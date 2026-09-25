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
        if self.connected {
            &self.children
        } else {
            &[]
        }
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

#[cfg(windows)]
#[allow(unsafe_code)] // Server-side UIA vtables and SAFEARRAY ownership stay here.
pub mod native {
    use std::{
        cell::{Cell, RefCell},
        ffi::c_void,
        ptr,
        rc::{Rc, Weak},
    };

    use windows::{
        core::{implement, Error, IUnknown, Interface, Result, HRESULT, VARIANT},
        Win32::{
            Foundation::{BOOL, HWND, LPARAM, LRESULT, RECT, WPARAM},
            System::{
                Com::SAFEARRAY,
                Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement},
                Variant::{VT_I4, VT_UNKNOWN},
            },
            UI::{
                Accessibility::{
                    IRawElementProviderFragment, IRawElementProviderFragmentRoot,
                    IRawElementProviderFragmentRoot_Impl, IRawElementProviderFragment_Impl,
                    IRawElementProviderSimple, IRawElementProviderSimple_Impl,
                    ISelectionItemProvider, ISelectionItemProvider_Impl, ISelectionProvider,
                    ISelectionProvider_Impl, NavigateDirection, NavigateDirection_FirstChild,
                    NavigateDirection_LastChild, NavigateDirection_NextSibling,
                    NavigateDirection_Parent, NavigateDirection_PreviousSibling, ProviderOptions,
                    ProviderOptions_ServerSideProvider, UIA_ControlTypePropertyId,
                    UIA_HasKeyboardFocusPropertyId, UIA_IsControlElementPropertyId,
                    UIA_IsEnabledPropertyId, UIA_IsKeyboardFocusablePropertyId, UIA_NamePropertyId,
                    UIA_SelectionItemPatternId, UIA_SelectionPatternId, UIA_TabControlTypeId,
                    UIA_TabItemControlTypeId, UiaAppendRuntimeId, UiaHostProviderFromHwnd, UiaRect,
                    UiaReturnRawElementProvider, UiaRootObjectId, UIA_E_ELEMENTNOTAVAILABLE,
                    UIA_PATTERN_ID, UIA_PROPERTY_ID,
                },
                WindowsAndMessaging::{GetFocus, GetWindowRect, SetFocus},
            },
        },
    };

    use crate::{app::AppState, layout::PixelRect, tabs::TabId};

    pub fn is_uia_root_request(lparam: LPARAM) -> bool {
        lparam.0 == UiaRootObjectId as isize
    }

    struct ProviderState {
        hwnd: HWND,
        app: Weak<RefCell<AppState>>,
        select: Weak<dyn Fn(TabId)>,
        bounds: Cell<PixelRect>,
        connected: Cell<bool>,
    }

    impl ProviderState {
        fn app(&self) -> Result<Rc<RefCell<AppState>>> {
            if !self.connected.get() {
                return Err(UIA_E_ELEMENTNOTAVAILABLE.into());
            }
            self.app
                .upgrade()
                .ok_or_else(|| UIA_E_ELEMENTNOTAVAILABLE.into())
        }

        fn tabs(&self) -> Result<Vec<(TabId, String, bool)>> {
            let app = self.app()?;
            let app = app.borrow();
            Ok(app
                .tabs()
                .tabs()
                .iter()
                .map(|tab| {
                    (
                        tab.id(),
                        tab.title().to_owned(),
                        app.active_tab() == Some(tab.id()),
                    )
                })
                .collect())
        }

        fn tab_index(&self, id: TabId) -> Result<usize> {
            self.tabs()?
                .iter()
                .position(|(candidate, _, _)| *candidate == id)
                .ok_or_else(|| UIA_E_ELEMENTNOTAVAILABLE.into())
        }

        fn tab_rect(&self, id: TabId) -> Result<UiaRect> {
            let tabs = self.tabs()?;
            let index = tabs
                .iter()
                .position(|(candidate, _, _)| *candidate == id)
                .ok_or_else(|| Error::from(UIA_E_ELEMENTNOTAVAILABLE))?;
            let mut host = RECT::default();
            unsafe { GetWindowRect(self.hwnd, &mut host) }?;
            if tabs.is_empty() || index >= tabs.len() {
                return Err(UIA_E_ELEMENTNOTAVAILABLE.into());
            }
            let width = (host.right - host.left).max(0) as f64 / tabs.len() as f64;
            Ok(UiaRect {
                left: host.left as f64 + width * index as f64,
                top: host.top as f64,
                width,
                height: (host.bottom - host.top).max(0) as f64,
            })
        }
    }

    #[derive(Clone)]
    pub struct NativeAccessibility {
        state: Rc<ProviderState>,
    }

    impl NativeAccessibility {
        pub fn new(hwnd: HWND, app: Weak<RefCell<AppState>>, select: Weak<dyn Fn(TabId)>) -> Self {
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

        pub fn refresh(&self) {}

        pub fn disconnect(&mut self) {
            self.state.connected.set(false);
        }

        pub fn update_tab_strip_bounds(&self, bounds: PixelRect) {
            self.state.bounds.set(bounds);
        }

        pub fn tab_strip_width(&self) -> u32 {
            self.state.bounds.get().width
        }

        pub unsafe fn return_provider(&self, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            let provider: IRawElementProviderSimple = RootProvider {
                state: Rc::downgrade(&self.state),
            }
            .into();
            unsafe { UiaReturnRawElementProvider(self.state.hwnd, wparam, lparam, &provider) }
        }
    }

    fn unavailable<T>() -> Result<T> {
        Err(UIA_E_ELEMENTNOTAVAILABLE.into())
    }

    // The generated implementation trait models nullable COM out interfaces
    // as Result<T>. Its vtable adapter initializes the out pointer to null;
    // returning S_OK through this sentinel therefore represents the native
    // contract's successful null without constructing an invalid Rust wrapper.
    fn successful_null<T>() -> Result<T> {
        Err(Error::from_hresult(HRESULT(0)))
    }

    fn root_simple(state: &Rc<ProviderState>) -> IRawElementProviderSimple {
        RootProvider {
            state: Rc::downgrade(state),
        }
        .into()
    }

    fn root_fragment(state: &Rc<ProviderState>) -> IRawElementProviderFragment {
        RootProvider {
            state: Rc::downgrade(state),
        }
        .into()
    }

    fn root_fragment_root(state: &Rc<ProviderState>) -> IRawElementProviderFragmentRoot {
        RootProvider {
            state: Rc::downgrade(state),
        }
        .into()
    }

    fn child_fragment(state: &Rc<ProviderState>, id: TabId) -> IRawElementProviderFragment {
        TabProvider {
            state: Rc::downgrade(state),
            id,
        }
        .into()
    }

    fn child_simple(state: &Rc<ProviderState>, id: TabId) -> IRawElementProviderSimple {
        TabProvider {
            state: Rc::downgrade(state),
            id,
        }
        .into()
    }

    #[implement(
        IRawElementProviderSimple,
        IRawElementProviderFragment,
        IRawElementProviderFragmentRoot,
        ISelectionProvider
    )]
    struct RootProvider {
        state: Weak<ProviderState>,
    }

    impl RootProvider_Impl {
        fn live(&self) -> Result<Rc<ProviderState>> {
            self.state
                .upgrade()
                .filter(|state| state.connected.get())
                .ok_or_else(|| UIA_E_ELEMENTNOTAVAILABLE.into())
        }
    }

    impl IRawElementProviderSimple_Impl for RootProvider_Impl {
        fn ProviderOptions(&self) -> Result<ProviderOptions> {
            Ok(ProviderOptions_ServerSideProvider)
        }
        fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID) -> Result<IUnknown> {
            let state = self.live()?;
            if pattern == UIA_SelectionPatternId {
                let provider: ISelectionProvider = RootProvider {
                    state: Rc::downgrade(&state),
                }
                .into();
                Ok(provider.into())
            } else {
                successful_null()
            }
        }
        fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
            let state = self.live()?;
            if property == UIA_NamePropertyId {
                Ok(VARIANT::from("Document tabs"))
            } else if property == UIA_ControlTypePropertyId {
                Ok(VARIANT::from(UIA_TabControlTypeId.0))
            } else if property == UIA_IsControlElementPropertyId
                || property == UIA_IsEnabledPropertyId
                || property == UIA_IsKeyboardFocusablePropertyId
            {
                Ok(VARIANT::from(true))
            } else if property == UIA_HasKeyboardFocusPropertyId {
                Ok(VARIANT::from(unsafe { GetFocus() } == state.hwnd))
            } else {
                Ok(VARIANT::default())
            }
        }
        fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
            let state = self.live()?;
            unsafe { UiaHostProviderFromHwnd(state.hwnd) }
        }
    }

    impl IRawElementProviderFragment_Impl for RootProvider_Impl {
        fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
            let state = self.live()?;
            let tabs = state.tabs()?;
            match direction {
                NavigateDirection_FirstChild if !tabs.is_empty() => {
                    Ok(child_fragment(&state, tabs[0].0))
                }
                NavigateDirection_LastChild if !tabs.is_empty() => {
                    Ok(child_fragment(&state, tabs[tabs.len() - 1].0))
                }
                _ => successful_null(),
            }
        }
        fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
            Ok(ptr::null_mut())
        }
        fn BoundingRectangle(&self) -> Result<UiaRect> {
            let state = self.live()?;
            let mut rect = RECT::default();
            unsafe { GetWindowRect(state.hwnd, &mut rect) }?;
            Ok(UiaRect {
                left: rect.left as f64,
                top: rect.top as f64,
                width: (rect.right - rect.left).max(0) as f64,
                height: (rect.bottom - rect.top).max(0) as f64,
            })
        }
        fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
            Ok(ptr::null_mut())
        }
        fn SetFocus(&self) -> Result<()> {
            let state = self.live()?;
            unsafe { SetFocus(Some(state.hwnd)) };
            Ok(())
        }
        fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
            let state = self.live()?;
            Ok(root_fragment_root(&state))
        }
    }

    impl IRawElementProviderFragmentRoot_Impl for RootProvider_Impl {
        fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
            let state = self.live()?;
            let tabs = state.tabs()?;
            for index in 0..tabs.len() {
                let r = state.tab_rect(tabs[index].0)?;
                if x >= r.left && x < r.left + r.width && y >= r.top && y < r.top + r.height {
                    return Ok(child_fragment(&state, tabs[index].0));
                }
            }
            Ok(root_fragment(&state))
        }
        fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
            let state = self.live()?;
            let tabs = state.tabs()?;
            let index = tabs
                .iter()
                .position(|(_, _, selected)| *selected)
                .unwrap_or(0);
            if tabs.is_empty() {
                Ok(root_fragment(&state))
            } else {
                Ok(child_fragment(&state, tabs[index].0))
            }
        }
    }

    impl ISelectionProvider_Impl for RootProvider_Impl {
        fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
            let state = self.live()?;
            let tabs = state.tabs()?;
            let Some(index) = tabs.iter().position(|(_, _, selected)| *selected) else {
                return Ok(ptr::null_mut());
            };
            let child: IUnknown = child_simple(&state, tabs[index].0).into();
            let array = unsafe { SafeArrayCreateVector(VT_UNKNOWN, 0, 1) };
            if array.is_null() {
                return unavailable();
            }
            if let Err(error) = unsafe { SafeArrayPutElement(array, &0, child.as_raw()) } {
                let _ = unsafe { SafeArrayDestroy(array) };
                return Err(error);
            }
            Ok(array)
        }
        fn CanSelectMultiple(&self) -> Result<BOOL> {
            Ok(false.into())
        }
        fn IsSelectionRequired(&self) -> Result<BOOL> {
            Ok(true.into())
        }
    }

    #[implement(
        IRawElementProviderSimple,
        IRawElementProviderFragment,
        ISelectionItemProvider
    )]
    struct TabProvider {
        state: Weak<ProviderState>,
        id: TabId,
    }

    impl TabProvider_Impl {
        fn live(&self) -> Result<(Rc<ProviderState>, Vec<(TabId, String, bool)>)> {
            let state = self
                .state
                .upgrade()
                .filter(|state| state.connected.get())
                .ok_or_else(|| UIA_E_ELEMENTNOTAVAILABLE.into())?;
            let tabs = state.tabs()?;
            if tabs.iter().any(|(id, _, _)| *id == self.id) {
                Ok((state, tabs))
            } else {
                unavailable()
            }
        }
    }

    impl IRawElementProviderSimple_Impl for TabProvider_Impl {
        fn ProviderOptions(&self) -> Result<ProviderOptions> {
            Ok(ProviderOptions_ServerSideProvider)
        }
        fn GetPatternProvider(&self, pattern: UIA_PATTERN_ID) -> Result<IUnknown> {
            let (state, _) = self.live()?;
            if pattern == UIA_SelectionItemPatternId {
                let provider: ISelectionItemProvider = TabProvider {
                    state: Rc::downgrade(&state),
                    id: self.id,
                }
                .into();
                Ok(provider.into())
            } else {
                successful_null()
            }
        }
        fn GetPropertyValue(&self, property: UIA_PROPERTY_ID) -> Result<VARIANT> {
            let (_, tabs) = self.live()?;
            let (_, name, selected) = tabs
                .iter()
                .find(|(id, _, _)| *id == self.id)
                .ok_or_else(|| Error::from(UIA_E_ELEMENTNOTAVAILABLE))?;
            if property == UIA_NamePropertyId {
                Ok(VARIANT::from(name.as_str()))
            } else if property == UIA_ControlTypePropertyId {
                Ok(VARIANT::from(UIA_TabItemControlTypeId.0))
            } else if property == UIA_IsControlElementPropertyId
                || property == UIA_IsEnabledPropertyId
                || property == UIA_IsKeyboardFocusablePropertyId
            {
                Ok(VARIANT::from(true))
            } else if property == UIA_HasKeyboardFocusPropertyId {
                Ok(VARIANT::from(*selected))
            } else {
                Ok(VARIANT::default())
            }
        }
        fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
            successful_null()
        }
    }

    impl IRawElementProviderFragment_Impl for TabProvider_Impl {
        fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
            let (state, tabs) = self.live()?;
            let index = state.tab_index(self.id)?;
            match direction {
                NavigateDirection_Parent => Ok(root_fragment(&state)),
                NavigateDirection_NextSibling if index + 1 < tabs.len() => {
                    Ok(child_fragment(&state, tabs[index + 1].0))
                }
                NavigateDirection_PreviousSibling if index > 0 => {
                    Ok(child_fragment(&state, tabs[index - 1].0))
                }
                _ => successful_null(),
            }
        }
        fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
            let (state, _) = self.live()?;
            let values = [
                UiaAppendRuntimeId,
                self.id.get() as i32,
                (self.id.get() >> 32) as i32,
            ];
            let array = unsafe { SafeArrayCreateVector(VT_I4, 0, values.len() as u32) };
            if array.is_null() {
                return unavailable();
            }
            for (index, value) in values.iter().enumerate() {
                if let Err(error) = unsafe {
                    SafeArrayPutElement(
                        array,
                        &(index as i32),
                        (value as *const i32).cast::<c_void>(),
                    )
                } {
                    let _ = unsafe { SafeArrayDestroy(array) };
                    return Err(error);
                }
            }
            drop(state);
            Ok(array)
        }
        fn BoundingRectangle(&self) -> Result<UiaRect> {
            let (state, _) = self.live()?;
            state.tab_rect(self.id)
        }
        fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
            Ok(ptr::null_mut())
        }
        fn SetFocus(&self) -> Result<()> {
            self.Select()
        }
        fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
            let (state, _) = self.live()?;
            Ok(root_fragment_root(&state))
        }
    }

    impl ISelectionItemProvider_Impl for TabProvider_Impl {
        fn Select(&self) -> Result<()> {
            let (state, _) = self.live()?;
            let select = state
                .select
                .upgrade()
                .ok_or_else(|| Error::from(UIA_E_ELEMENTNOTAVAILABLE))?;
            select(self.id);
            unsafe { SetFocus(Some(state.hwnd)) };
            Ok(())
        }
        fn AddToSelection(&self) -> Result<()> {
            self.Select()
        }
        fn RemoveFromSelection(&self) -> Result<()> {
            unavailable()
        }
        fn IsSelected(&self) -> Result<BOOL> {
            let (_, tabs) = self.live()?;
            Ok(tabs
                .iter()
                .find(|(id, _, _)| *id == self.id)
                .is_some_and(|(_, _, selected)| *selected)
                .into())
        }
        fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
            let (state, _) = self.live()?;
            Ok(root_simple(&state))
        }
    }
}
