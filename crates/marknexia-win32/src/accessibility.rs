//! UI Automation semantics shared by the native providers and static tests.

use crate::tabs::{TabId, TabStore};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessibleRole {
    TabControl,
    TabItem,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccessibleElement {
    name: String,
    role: AccessibleRole,
    tab_id: Option<TabId>,
    selected: bool,
}

impl AccessibleElement {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn role(&self) -> AccessibleRole {
        self.role
    }

    #[must_use]
    pub const fn is_selected(&self) -> bool {
        self.selected
    }
}

pub struct AccessibilityTree {
    root: AccessibleElement,
    children: Vec<AccessibleElement>,
    connected: bool,
}

impl AccessibilityTree {
    #[must_use]
    pub fn from_tabs(tabs: &TabStore) -> Self {
        Self {
            root: AccessibleElement {
                name: "Document tabs".into(),
                role: AccessibleRole::TabControl,
                tab_id: None,
                selected: false,
            },
            children: tabs
                .tabs()
                .iter()
                .map(|tab| AccessibleElement {
                    name: tab.title().into(),
                    role: AccessibleRole::TabItem,
                    tab_id: Some(tab.id()),
                    selected: tabs.active_id() == Some(tab.id()),
                })
                .collect(),
            connected: true,
        }
    }

    #[must_use]
    pub fn root(&self) -> &AccessibleElement {
        &self.root
    }

    #[must_use]
    pub fn children(&self) -> &[AccessibleElement] {
        if self.connected { &self.children } else { &[] }
    }

    #[must_use]
    pub fn selected_tab(&self) -> Option<TabId> {
        self.connected.then(|| {
            self.children
                .iter()
                .find(|child| child.selected)
                .and_then(|child| child.tab_id)
        })?
    }

    pub fn select(&self, tabs: &mut TabStore, tab_id: TabId) -> bool {
        self.connected && tabs.select(tab_id)
    }

    pub fn disconnect(&mut self) {
        self.connected = false;
        self.children.clear();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AutomationSmokeContract {
    pub root_control_type: &'static str,
    pub child_control_type: &'static str,
    pub requires_selection_pattern: bool,
    pub requires_active_view_change: bool,
    pub rejects_default_hwnd_proxy: bool,
}

pub const AUTOMATION_SMOKE_CONTRACT: AutomationSmokeContract = AutomationSmokeContract {
    root_control_type: "TabControl",
    child_control_type: "TabItem",
    requires_selection_pattern: true,
    requires_active_view_change: true,
    rejects_default_hwnd_proxy: true,
};

/// Equal-width tab slots shared by painting, hit testing, and UIA bounds so
/// every surface agrees on where a tab is. The last slot absorbs remainders.
#[must_use]
pub fn tab_slot(total_width: i32, count: usize, index: usize) -> Option<(i32, i32)> {
    if count == 0 || index >= count {
        return None;
    }
    let total = total_width.max(0);
    let width = total / count as i32;
    let left = width * index as i32;
    let right = if index + 1 == count {
        total
    } else {
        left + width
    };
    Some((left, right))
}

#[cfg(windows)]
#[allow(unsafe_code)] // Server-side UIA vtables and SAFEARRAY ownership stay here.
#[allow(non_snake_case)] // COM method names follow the UIA ABI.
#[allow(non_upper_case_globals)] // windows-rs names UIA enum constants in PascalCase.
pub mod native {
    use std::{
        cell::{Cell, RefCell},
        ffi::c_void,
        ptr,
        rc::{Rc, Weak},
    };

    use windows::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        System::{
            Com::SAFEARRAY,
            Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement},
            Variant::{VARIANT, VT_I4, VT_UNKNOWN},
        },
        UI::{
            Accessibility::{
                IRawElementProviderFragment, IRawElementProviderFragmentRoot,
                IRawElementProviderSimple, ISelectionItemProvider, ISelectionItemProvider_Impl,
                ISelectionProvider, ISelectionProvider_Impl, NavigateDirection,
                NavigateDirection_FirstChild, NavigateDirection_LastChild,
                NavigateDirection_NextSibling, NavigateDirection_Parent,
                NavigateDirection_PreviousSibling, ProviderOptions,
                ProviderOptions_ServerSideProvider, StructureChangeType_ChildrenInvalidated,
                UIA_AutomationFocusChangedEventId, UIA_ControlTypePropertyId,
                UIA_E_ELEMENTNOTAVAILABLE, UIA_HasKeyboardFocusPropertyId,
                UIA_IsControlElementPropertyId, UIA_IsEnabledPropertyId,
                UIA_IsKeyboardFocusablePropertyId, UIA_NamePropertyId, UIA_PATTERN_ID,
                UIA_PROPERTY_ID, UIA_SelectionItem_ElementSelectedEventId,
                UIA_SelectionItemPatternId, UIA_SelectionPatternId, UIA_TabControlTypeId,
                UIA_TabItemControlTypeId, UiaAppendRuntimeId, UiaClientsAreListening,
                UiaHostProviderFromHwnd, UiaRaiseAutomationEvent, UiaRaiseStructureChangedEvent,
                UiaRect, UiaReturnRawElementProvider, UiaRootObjectId,
            },
            Input::KeyboardAndMouse::{GetFocus, SetFocus},
            WindowsAndMessaging::GetWindowRect,
        },
    };
    // `#[implement]` and `#[interface]` expand to `::windows_core` paths, so the
    // crate depends on windows-core directly.
    use windows_core::{BOOL, Error, HRESULT, IUnknown, Interface, Result, implement};

    use super::tab_slot;
    use crate::{app::AppState, layout::PixelRect, tabs::TabId};

    const S_OK: HRESULT = HRESULT(0);
    const E_POINTER: HRESULT = HRESULT(0x8000_4003_u32 as i32);
    const E_FAIL: HRESULT = HRESULT(0x8000_4005_u32 as i32);
    const ELEMENT_NOT_AVAILABLE: HRESULT = HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32);
    /// `UIA_E_INVALIDOPERATION`: the request is valid but cannot run now.
    const INVALID_OPERATION: HRESULT = HRESULT(0x8013_1509_u32 as i32);

    use abi::{
        IRawElementProviderFragmentAbi, IRawElementProviderFragmentAbi_Impl,
        IRawElementProviderFragmentRootAbi, IRawElementProviderFragmentRootAbi_Impl,
        IRawElementProviderSimpleAbi, IRawElementProviderSimpleAbi_Impl,
    };

    mod abi {
        // The generated caller-side methods are never invoked from Rust.
        #![allow(dead_code)]

        use std::ffi::c_void;

        use windows::Win32::{
            System::{Com::SAFEARRAY, Variant::VARIANT},
            UI::Accessibility::{
                NavigateDirection, ProviderOptions, UIA_PATTERN_ID, UIA_PROPERTY_ID, UiaRect,
            },
        };
        use windows_core::{HRESULT, IUnknown, IUnknown_Vtbl, interface};

        // windows-rs 0.62.2 models the nullable UIA interface returns below as
        // non-null `Result<Interface>` values, and its generated error path returns
        // the error HRESULT without writing the out pointer. These ABI-exact local
        // declarations reuse the SDK IIDs and vtable order so every interface out
        // pointer is initialized before any HRESULT is returned, and successful
        // null is expressible without UB. As in UIAutomationCore.h, all three
        // interfaces derive directly from IUnknown; Fragment and FragmentRoot do
        // NOT extend IRawElementProviderSimple, so their slots start at 3.
        #[interface("d6dd68d1-86fd-4332-8666-9abedea2d24c")]
        pub unsafe trait IRawElementProviderSimpleAbi: IUnknown {
            unsafe fn ProviderOptions(&self, result: *mut ProviderOptions) -> HRESULT;
            unsafe fn GetPatternProvider(
                &self,
                pattern: UIA_PATTERN_ID,
                result: *mut *mut c_void,
            ) -> HRESULT;
            unsafe fn GetPropertyValue(
                &self,
                property: UIA_PROPERTY_ID,
                result: *mut VARIANT,
            ) -> HRESULT;
            unsafe fn HostRawElementProvider(&self, result: *mut *mut c_void) -> HRESULT;
        }

        #[interface("f7063da8-8359-439c-9297-bbc5299a7d87")]
        pub unsafe trait IRawElementProviderFragmentAbi: IUnknown {
            unsafe fn Navigate(
                &self,
                direction: NavigateDirection,
                result: *mut *mut c_void,
            ) -> HRESULT;
            unsafe fn GetRuntimeId(&self, result: *mut *mut SAFEARRAY) -> HRESULT;
            unsafe fn BoundingRectangle(&self, result: *mut UiaRect) -> HRESULT;
            unsafe fn GetEmbeddedFragmentRoots(&self, result: *mut *mut SAFEARRAY) -> HRESULT;
            unsafe fn SetFocus(&self) -> HRESULT;
            unsafe fn FragmentRoot(&self, result: *mut *mut c_void) -> HRESULT;
        }

        #[interface("620ce2a5-ab8f-40a9-86cb-de3c75599b58")]
        pub unsafe trait IRawElementProviderFragmentRootAbi: IUnknown {
            unsafe fn ElementProviderFromPoint(
                &self,
                x: f64,
                y: f64,
                result: *mut *mut c_void,
            ) -> HRESULT;
            unsafe fn GetFocus(&self, result: *mut *mut c_void) -> HRESULT;
        }
    }

    pub fn is_uia_root_request(lparam: LPARAM) -> bool {
        lparam.0 == UiaRootObjectId as isize
    }

    /// Synchronous shell selection. Returns true only after portable state,
    /// the active WebView controller, and the painted tab strip all agree.
    pub type SelectTab = dyn Fn(TabId) -> bool;

    struct TabSnapshot {
        id: TabId,
        name: String,
        selected: bool,
    }

    struct ProviderState {
        hwnd: HWND,
        app: Weak<RefCell<AppState>>,
        select: Weak<SelectTab>,
        bounds: Cell<PixelRect>,
        connected: Cell<bool>,
    }

    impl ProviderState {
        fn tabs(&self) -> Result<Vec<TabSnapshot>> {
            if !self.connected.get() {
                return Err(ELEMENT_NOT_AVAILABLE.into());
            }
            let app = self
                .app
                .upgrade()
                .ok_or_else(|| Error::from(ELEMENT_NOT_AVAILABLE))?;
            // A panic here would unwind across the COM boundary, so a
            // conflicting borrow is reported as a retryable failure instead.
            let app = app.try_borrow().map_err(|_| Error::from(E_FAIL))?;
            let active = app.active_tab();
            Ok(app
                .tabs()
                .tabs()
                .iter()
                .map(|tab| TabSnapshot {
                    id: tab.id(),
                    name: tab.title().to_owned(),
                    selected: active == Some(tab.id()),
                })
                .collect())
        }

        fn host_rect(&self) -> Result<RECT> {
            let mut rect = RECT::default();
            unsafe { GetWindowRect(self.hwnd, &mut rect) }?;
            Ok(rect)
        }

        fn tab_rect(&self, tabs: &[TabSnapshot], id: TabId) -> Result<UiaRect> {
            let index = tabs
                .iter()
                .position(|tab| tab.id == id)
                .ok_or_else(|| Error::from(ELEMENT_NOT_AVAILABLE))?;
            let host = self.host_rect()?;
            let (left, right) = tab_slot(host.right - host.left, tabs.len(), index)
                .ok_or_else(|| Error::from(ELEMENT_NOT_AVAILABLE))?;
            Ok(UiaRect {
                left: f64::from(host.left + left),
                top: f64::from(host.top),
                width: f64::from(right - left),
                height: f64::from((host.bottom - host.top).max(0)),
            })
        }

        fn has_focus(&self) -> bool {
            let focused = unsafe { GetFocus() };
            focused == self.hwnd
        }
    }

    #[derive(Clone)]
    pub struct NativeAccessibility {
        state: Rc<ProviderState>,
    }

    impl NativeAccessibility {
        pub fn new(hwnd: HWND, app: Weak<RefCell<AppState>>, select: Weak<SelectTab>) -> Self {
            Self {
                state: Rc::new(ProviderState {
                    hwnd,
                    app,
                    select,
                    bounds: Cell::new(PixelRect::default()),
                    connected: Cell::new(true),
                }),
            }
        }

        /// Raise `ElementSelected` for the newly active tab. Called by the
        /// shell after a completed selection from any input source.
        pub fn notify_selected(&self, id: TabId) {
            if !self.state.connected.get() || !unsafe { UiaClientsAreListening() }.as_bool() {
                return;
            }
            let Ok(provider) = child_simple(&self.state, id) else {
                return;
            };
            let _ = unsafe {
                UiaRaiseAutomationEvent(&provider, UIA_SelectionItem_ElementSelectedEventId)
            };
            // Keyboard focus follows selection while the strip is focused.
            if self.state.has_focus() {
                let _ = unsafe {
                    UiaRaiseAutomationEvent(&provider, UIA_AutomationFocusChangedEventId)
                };
            }
        }

        /// Tell UIA clients the tab children changed (tab opened or closed).
        pub fn notify_children_changed(&self) {
            if !self.state.connected.get() || !unsafe { UiaClientsAreListening() }.as_bool() {
                return;
            }
            let Ok(root) = root_simple(&self.state) else {
                return;
            };
            // The HWND-hosted root has no runtime ID of its own.
            let _ = unsafe {
                UiaRaiseStructureChangedEvent(
                    &root,
                    StructureChangeType_ChildrenInvalidated,
                    ptr::null_mut(),
                    0,
                )
            };
        }

        /// Stop serving live data and release UIA's references to the root
        /// provider, as required before the tab-strip HWND is destroyed.
        pub fn disconnect(&mut self) {
            self.state.connected.set(false);
            let _ = unsafe {
                UiaReturnRawElementProvider(
                    self.state.hwnd,
                    WPARAM(0),
                    LPARAM(0),
                    None::<&IRawElementProviderSimple>,
                )
            };
        }

        pub fn update_tab_strip_bounds(&self, bounds: PixelRect) {
            self.state.bounds.set(bounds);
        }

        pub fn tab_strip_width(&self) -> u32 {
            self.state.bounds.get().width
        }

        /// # Safety
        /// Call only from the tab strip's `WM_GETOBJECT` handler with the
        /// message's own `wparam` and `lparam`.
        pub unsafe fn return_provider(&self, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            if !self.state.connected.get() {
                return LRESULT(0);
            }
            let Ok(provider) = root_simple(&self.state) else {
                return LRESULT(0);
            };
            unsafe { UiaReturnRawElementProvider(self.state.hwnd, wparam, lparam, &provider) }
        }
    }

    /// Zero the interface out pointer first, then publish an owned reference
    /// only on success. Every nullable UIA return goes through here.
    unsafe fn write_interface<T: Interface>(
        result: *mut *mut c_void,
        value: Result<Option<T>>,
    ) -> HRESULT {
        if result.is_null() {
            return E_POINTER;
        }
        unsafe { result.write(ptr::null_mut()) };
        match value {
            Ok(Some(value)) => {
                unsafe { result.write(value.into_raw()) };
                S_OK
            }
            Ok(None) => S_OK,
            Err(error) => error.code(),
        }
    }

    /// Out VARIANTs are caller-owned uninitialized storage, so they are always
    /// initialized (to VT_EMPTY on failure) without dropping prior contents.
    unsafe fn write_variant(result: *mut VARIANT, value: Result<VARIANT>) -> HRESULT {
        if result.is_null() {
            return E_POINTER;
        }
        match value {
            Ok(value) => {
                unsafe { result.write(value) };
                S_OK
            }
            Err(error) => {
                unsafe { result.write(VARIANT::default()) };
                error.code()
            }
        }
    }

    unsafe fn write_rect(result: *mut UiaRect, value: Result<UiaRect>) -> HRESULT {
        if result.is_null() {
            return E_POINTER;
        }
        let (rect, code) = match value {
            Ok(rect) => (rect, S_OK),
            Err(error) => (UiaRect::default(), error.code()),
        };
        unsafe { result.write(rect) };
        code
    }

    /// Ownership of a non-null array transfers to the caller on success.
    unsafe fn write_safearray(
        result: *mut *mut SAFEARRAY,
        value: Result<*mut SAFEARRAY>,
    ) -> HRESULT {
        if result.is_null() {
            return E_POINTER;
        }
        unsafe { result.write(ptr::null_mut()) };
        match value {
            Ok(value) => {
                unsafe { result.write(value) };
                S_OK
            }
            Err(error) => error.code(),
        }
    }

    /// Builds a one-dimensional SAFEARRAY. On any element failure the array is
    /// destroyed here, so no partially filled array escapes.
    fn build_safearray<T>(
        kind: windows::Win32::System::Variant::VARENUM,
        values: &[T],
        element: impl Fn(&T) -> *const c_void,
    ) -> Result<*mut SAFEARRAY> {
        let count = u32::try_from(values.len()).map_err(|_| Error::from(E_FAIL))?;
        let array = unsafe { SafeArrayCreateVector(kind, 0, count) };
        if array.is_null() {
            return Err(E_FAIL.into());
        }
        for (index, value) in values.iter().enumerate() {
            let index = i32::try_from(index).map_err(|_| Error::from(E_FAIL))?;
            if let Err(error) = unsafe { SafeArrayPutElement(array, &index, element(value)) } {
                let _ = unsafe { SafeArrayDestroy(array) };
                return Err(error);
            }
        }
        Ok(array)
    }

    fn root_object(state: &Rc<ProviderState>) -> RootProvider {
        RootProvider {
            state: Rc::downgrade(state),
        }
    }

    fn tab_object(state: &Rc<ProviderState>, id: TabId) -> TabProvider {
        TabProvider {
            state: Rc::downgrade(state),
            id,
        }
    }

    // The local ABI interfaces share the SDK IIDs, so QueryInterface on our
    // own object always succeeds; the result is still propagated, not unwrapped.
    fn root_simple(state: &Rc<ProviderState>) -> Result<IRawElementProviderSimple> {
        let provider: IRawElementProviderSimpleAbi = root_object(state).into();
        provider.cast()
    }

    fn root_fragment(state: &Rc<ProviderState>) -> Result<IRawElementProviderFragment> {
        let provider: IRawElementProviderFragmentAbi = root_object(state).into();
        provider.cast()
    }

    fn root_fragment_root(state: &Rc<ProviderState>) -> Result<IRawElementProviderFragmentRoot> {
        let provider: IRawElementProviderFragmentRootAbi = root_object(state).into();
        provider.cast()
    }

    fn child_fragment(state: &Rc<ProviderState>, id: TabId) -> Result<IRawElementProviderFragment> {
        let provider: IRawElementProviderFragmentAbi = tab_object(state, id).into();
        provider.cast()
    }

    fn child_simple(state: &Rc<ProviderState>, id: TabId) -> Result<IRawElementProviderSimple> {
        let provider: IRawElementProviderSimpleAbi = tab_object(state, id).into();
        provider.cast()
    }

    // Not agile: providers hold Rc/RefCell state and must stay on the STA.
    #[implement(
        IRawElementProviderSimpleAbi,
        IRawElementProviderFragmentAbi,
        IRawElementProviderFragmentRootAbi,
        ISelectionProvider,
        Agile = false
    )]
    struct RootProvider {
        state: Weak<ProviderState>,
    }

    impl RootProvider_Impl {
        fn live(&self) -> Result<Rc<ProviderState>> {
            self.state
                .upgrade()
                .filter(|state| state.connected.get())
                .ok_or_else(|| ELEMENT_NOT_AVAILABLE.into())
        }
    }

    impl IRawElementProviderSimpleAbi_Impl for RootProvider_Impl {
        unsafe fn ProviderOptions(&self, result: *mut ProviderOptions) -> HRESULT {
            if result.is_null() {
                return E_POINTER;
            }
            unsafe { result.write(ProviderOptions_ServerSideProvider) };
            S_OK
        }

        unsafe fn GetPatternProvider(
            &self,
            pattern: UIA_PATTERN_ID,
            result: *mut *mut c_void,
        ) -> HRESULT {
            let value = self.live().and_then(|state| {
                if pattern == UIA_SelectionPatternId {
                    let provider: ISelectionProvider = root_object(&state).into();
                    Ok(Some(provider.cast::<IUnknown>()?))
                } else {
                    Ok(None)
                }
            });
            unsafe { write_interface(result, value) }
        }

        unsafe fn GetPropertyValue(
            &self,
            property: UIA_PROPERTY_ID,
            result: *mut VARIANT,
        ) -> HRESULT {
            let value = self.live().map(|state| {
                if property == UIA_NamePropertyId {
                    VARIANT::from("Document tabs")
                } else if property == UIA_ControlTypePropertyId {
                    VARIANT::from(UIA_TabControlTypeId.0)
                } else if property == UIA_IsControlElementPropertyId
                    || property == UIA_IsEnabledPropertyId
                    || property == UIA_IsKeyboardFocusablePropertyId
                {
                    VARIANT::from(true)
                } else if property == UIA_HasKeyboardFocusPropertyId {
                    // With a selected tab, focus belongs to that child.
                    let focused = state.has_focus()
                        && state
                            .tabs()
                            .is_ok_and(|tabs| !tabs.iter().any(|tab| tab.selected));
                    VARIANT::from(focused)
                } else {
                    VARIANT::default()
                }
            });
            unsafe { write_variant(result, value) }
        }

        unsafe fn HostRawElementProvider(&self, result: *mut *mut c_void) -> HRESULT {
            let value = self
                .live()
                .and_then(|state| unsafe { UiaHostProviderFromHwnd(state.hwnd) }.map(Some));
            unsafe { write_interface(result, value) }
        }
    }

    impl IRawElementProviderFragmentAbi_Impl for RootProvider_Impl {
        unsafe fn Navigate(
            &self,
            direction: NavigateDirection,
            result: *mut *mut c_void,
        ) -> HRESULT {
            let value = self.live().and_then(|state| {
                let tabs = state.tabs()?;
                let target = match direction {
                    NavigateDirection_FirstChild => tabs.first(),
                    NavigateDirection_LastChild => tabs.last(),
                    // The HWND host provider supplies parent and siblings.
                    _ => None,
                };
                target.map(|tab| child_fragment(&state, tab.id)).transpose()
            });
            unsafe { write_interface(result, value) }
        }

        unsafe fn GetRuntimeId(&self, result: *mut *mut SAFEARRAY) -> HRESULT {
            // An HWND-hosted fragment root returns NULL; UIA derives its
            // runtime ID from the host HWND.
            let value = self.live().map(|_| ptr::null_mut());
            unsafe { write_safearray(result, value) }
        }

        unsafe fn BoundingRectangle(&self, result: *mut UiaRect) -> HRESULT {
            // An HWND-hosted fragment root may return an empty rectangle; the
            // host provider supplies the HWND bounds.
            let value = self.live().map(|_| UiaRect::default());
            unsafe { write_rect(result, value) }
        }

        unsafe fn GetEmbeddedFragmentRoots(&self, result: *mut *mut SAFEARRAY) -> HRESULT {
            let value = self.live().map(|_| ptr::null_mut());
            unsafe { write_safearray(result, value) }
        }

        unsafe fn SetFocus(&self) -> HRESULT {
            match self.live() {
                Ok(state) => match unsafe { SetFocus(Some(state.hwnd)) } {
                    Ok(_) => S_OK,
                    Err(error) => error.code(),
                },
                Err(error) => error.code(),
            }
        }

        unsafe fn FragmentRoot(&self, result: *mut *mut c_void) -> HRESULT {
            let value = self
                .live()
                .and_then(|state| root_fragment_root(&state).map(Some));
            unsafe { write_interface(result, value) }
        }
    }

    impl IRawElementProviderFragmentRootAbi_Impl for RootProvider_Impl {
        unsafe fn ElementProviderFromPoint(
            &self,
            x: f64,
            y: f64,
            result: *mut *mut c_void,
        ) -> HRESULT {
            let value = self.live().and_then(|state| {
                let tabs = state.tabs()?;
                for tab in &tabs {
                    let r = state.tab_rect(&tabs, tab.id)?;
                    if x >= r.left && x < r.left + r.width && y >= r.top && y < r.top + r.height {
                        return child_fragment(&state, tab.id).map(Some);
                    }
                }
                root_fragment(&state).map(Some)
            });
            unsafe { write_interface(result, value) }
        }

        unsafe fn GetFocus(&self, result: *mut *mut c_void) -> HRESULT {
            // Focus inside this fragment is the selected tab while the tab
            // strip owns keyboard focus; otherwise successful null.
            let value = self.live().and_then(|state| {
                if !state.has_focus() {
                    return Ok(None);
                }
                let tabs = state.tabs()?;
                tabs.iter()
                    .find(|tab| tab.selected)
                    .map(|tab| child_fragment(&state, tab.id))
                    .transpose()
            });
            unsafe { write_interface(result, value) }
        }
    }

    impl ISelectionProvider_Impl for RootProvider_Impl {
        fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
            let state = self.live()?;
            let selected = state
                .tabs()?
                .into_iter()
                .filter(|tab| tab.selected)
                .map(|tab| child_simple(&state, tab.id))
                .collect::<Result<Vec<_>>>()?;
            // UIA expects IRawElementProviderSimple pointers stored as
            // VT_UNKNOWN. SafeArrayPutElement AddRefs each element; the local
            // references are released when `selected` drops. An empty
            // selection is an empty array, never NULL.
            build_safearray(VT_UNKNOWN, &selected, |provider| provider.as_raw())
        }

        fn CanSelectMultiple(&self) -> Result<BOOL> {
            self.live().map(|_| false.into())
        }

        fn IsSelectionRequired(&self) -> Result<BOOL> {
            self.live().map(|_| true.into())
        }
    }

    #[implement(
        IRawElementProviderSimpleAbi,
        IRawElementProviderFragmentAbi,
        ISelectionItemProvider,
        Agile = false
    )]
    struct TabProvider {
        state: Weak<ProviderState>,
        id: TabId,
    }

    impl TabProvider_Impl {
        /// Resolves the provider by immutable `TabId`; a closed tab or a
        /// disconnected shell reports `UIA_E_ELEMENTNOTAVAILABLE`.
        fn live(&self) -> Result<(Rc<ProviderState>, Vec<TabSnapshot>)> {
            let state = self
                .state
                .upgrade()
                .filter(|state| state.connected.get())
                .ok_or_else(|| Error::from(ELEMENT_NOT_AVAILABLE))?;
            let tabs = state.tabs()?;
            if tabs.iter().any(|tab| tab.id == self.id) {
                Ok((state, tabs))
            } else {
                Err(ELEMENT_NOT_AVAILABLE.into())
            }
        }

        fn index_in(&self, tabs: &[TabSnapshot]) -> Result<usize> {
            tabs.iter()
                .position(|tab| tab.id == self.id)
                .ok_or_else(|| ELEMENT_NOT_AVAILABLE.into())
        }

        fn select_now(&self) -> Result<()> {
            let (state, tabs) = self.live()?;
            if tabs.iter().any(|tab| tab.id == self.id && tab.selected) {
                return Ok(());
            }
            let select = state
                .select
                .upgrade()
                .ok_or_else(|| Error::from(ELEMENT_NOT_AVAILABLE))?;
            // No provider borrow is held here: `tabs` is an owned snapshot.
            if select(self.id) {
                Ok(())
            } else {
                Err(INVALID_OPERATION.into())
            }
        }
    }

    impl IRawElementProviderSimpleAbi_Impl for TabProvider_Impl {
        unsafe fn ProviderOptions(&self, result: *mut ProviderOptions) -> HRESULT {
            if result.is_null() {
                return E_POINTER;
            }
            unsafe { result.write(ProviderOptions_ServerSideProvider) };
            S_OK
        }

        unsafe fn GetPatternProvider(
            &self,
            pattern: UIA_PATTERN_ID,
            result: *mut *mut c_void,
        ) -> HRESULT {
            let value = self.live().and_then(|(state, _)| {
                if pattern == UIA_SelectionItemPatternId {
                    let provider: ISelectionItemProvider = tab_object(&state, self.id).into();
                    Ok(Some(provider.cast::<IUnknown>()?))
                } else {
                    Ok(None)
                }
            });
            unsafe { write_interface(result, value) }
        }

        unsafe fn GetPropertyValue(
            &self,
            property: UIA_PROPERTY_ID,
            result: *mut VARIANT,
        ) -> HRESULT {
            let value = self.live().and_then(|(state, tabs)| {
                let tab = &tabs[self.index_in(&tabs)?];
                Ok(if property == UIA_NamePropertyId {
                    VARIANT::from(tab.name.as_str())
                } else if property == UIA_ControlTypePropertyId {
                    VARIANT::from(UIA_TabItemControlTypeId.0)
                } else if property == UIA_IsControlElementPropertyId
                    || property == UIA_IsEnabledPropertyId
                    || property == UIA_IsKeyboardFocusablePropertyId
                {
                    VARIANT::from(true)
                } else if property == UIA_HasKeyboardFocusPropertyId {
                    VARIANT::from(tab.selected && state.has_focus())
                } else {
                    VARIANT::default()
                })
            });
            unsafe { write_variant(result, value) }
        }

        unsafe fn HostRawElementProvider(&self, result: *mut *mut c_void) -> HRESULT {
            // Non-HWND fragment children have no host provider.
            let value = self.live().map(|_| None::<IUnknown>);
            unsafe { write_interface(result, value) }
        }
    }

    impl IRawElementProviderFragmentAbi_Impl for TabProvider_Impl {
        unsafe fn Navigate(
            &self,
            direction: NavigateDirection,
            result: *mut *mut c_void,
        ) -> HRESULT {
            let value = self.live().and_then(|(state, tabs)| {
                let index = self.index_in(&tabs)?;
                match direction {
                    NavigateDirection_Parent => root_fragment(&state).map(Some),
                    NavigateDirection_NextSibling => tabs
                        .get(index + 1)
                        .map(|tab| child_fragment(&state, tab.id))
                        .transpose(),
                    NavigateDirection_PreviousSibling => index
                        .checked_sub(1)
                        .and_then(|previous| tabs.get(previous))
                        .map(|tab| child_fragment(&state, tab.id))
                        .transpose(),
                    _ => Ok(None),
                }
            });
            unsafe { write_interface(result, value) }
        }

        unsafe fn GetRuntimeId(&self, result: *mut *mut SAFEARRAY) -> HRESULT {
            // Stable per tab: UiaAppendRuntimeId prefixes the host HWND's
            // runtime ID, followed by the immutable 64-bit TabId halves.
            let value = self.live().and_then(|_| {
                let raw = self.id.get();
                let values = [
                    UiaAppendRuntimeId as i32,
                    (raw & 0xffff_ffff) as u32 as i32,
                    (raw >> 32) as u32 as i32,
                ];
                build_safearray(VT_I4, &values, |value| ptr::from_ref(value).cast())
            });
            unsafe { write_safearray(result, value) }
        }

        unsafe fn BoundingRectangle(&self, result: *mut UiaRect) -> HRESULT {
            let value = self
                .live()
                .and_then(|(state, tabs)| state.tab_rect(&tabs, self.id));
            unsafe { write_rect(result, value) }
        }

        unsafe fn GetEmbeddedFragmentRoots(&self, result: *mut *mut SAFEARRAY) -> HRESULT {
            let value = self.live().map(|_| ptr::null_mut());
            unsafe { write_safearray(result, value) }
        }

        unsafe fn SetFocus(&self) -> HRESULT {
            // Tab focus follows selection in this strip: select, then give the
            // strip keyboard focus.
            let outcome = self.select_now().and_then(|()| {
                let (state, _) = self.live()?;
                unsafe { SetFocus(Some(state.hwnd)) }.map(|_| ())
            });
            match outcome {
                Ok(()) => S_OK,
                Err(error) => error.code(),
            }
        }

        unsafe fn FragmentRoot(&self, result: *mut *mut c_void) -> HRESULT {
            let value = self
                .live()
                .and_then(|(state, _)| root_fragment_root(&state).map(Some));
            unsafe { write_interface(result, value) }
        }
    }

    impl ISelectionItemProvider_Impl for TabProvider_Impl {
        fn Select(&self) -> Result<()> {
            self.select_now()
        }

        fn AddToSelection(&self) -> Result<()> {
            // Single selection: adding is only valid for the selected tab.
            let (_, tabs) = self.live()?;
            if tabs.iter().any(|tab| tab.id == self.id && tab.selected) {
                Ok(())
            } else {
                Err(INVALID_OPERATION.into())
            }
        }

        fn RemoveFromSelection(&self) -> Result<()> {
            // Selection is required, so the selected tab cannot be removed;
            // removing an unselected tab is a no-op.
            let (_, tabs) = self.live()?;
            if tabs.iter().any(|tab| tab.id == self.id && tab.selected) {
                Err(INVALID_OPERATION.into())
            } else {
                Ok(())
            }
        }

        fn IsSelected(&self) -> Result<BOOL> {
            let (_, tabs) = self.live()?;
            Ok(tabs
                .iter()
                .any(|tab| tab.id == self.id && tab.selected)
                .into())
        }

        fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
            let (state, _) = self.live()?;
            root_simple(&state)
        }
    }
}
