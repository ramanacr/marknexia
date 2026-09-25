#![cfg(windows)]

//! Controlled native UI Automation gate. The AV-safe source pass does not run
//! this ignored test; `Test-RustNativeShell.ps1` supplies the final executable.

use std::{
    process::Command,
    thread,
    time::{Duration, Instant},
};

use windows::{
    core::{w, Interface},
    Win32::{
        System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
        UI::{
            Accessibility::{
                CUIAutomation, IUIAutomation, IUIAutomationSelectionItemPattern,
                TreeScope_Children, UIA_SelectionItemPatternId, UIA_TabControlTypeId,
                UIA_TabItemControlTypeId,
            },
            WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId},
        },
    },
};

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE and a controlled interactive desktop"]
fn two_named_tab_items_expose_selection_and_change_active_tab() {
    let executable = std::env::var_os("MARKNEXIA_SHELL_EXE").expect("MARKNEXIA_SHELL_EXE");
    let mut child = Command::new(executable).spawn().expect("launch shell");
    let deadline = Instant::now() + Duration::from_secs(15);
    let hwnd = loop {
        if let Ok(hwnd) = unsafe { FindWindowW(w!("MarknexiaRustWindow"), None) } {
            let mut owner = 0;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
            if owner == child.id() {
                break hwnd;
            }
        }
        assert!(Instant::now() < deadline, "shell HWND did not appear");
        thread::sleep(Duration::from_millis(100));
    };

    let _apartment = windows::core::initialize_sta().expect("UIA STA");
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .expect("CUIAutomation");
    let root = unsafe { automation.ElementFromHandle(hwnd) }.expect("ElementFromHandle");
    let children = unsafe {
        root.FindAll(
            TreeScope_Children,
            &automation.CreateTrueCondition().unwrap(),
        )
    }
    .expect("root children");
    let mut tab_control = None;
    for index in 0..unsafe { children.Length() }.unwrap() {
        let element = unsafe { children.GetElement(index) }.unwrap();
        if unsafe { element.CurrentControlType() }.unwrap() == UIA_TabControlTypeId.0 {
            tab_control = Some(element);
            break;
        }
    }
    let tab_control =
        tab_control.expect("custom TabControl child; default HWND proxy is insufficient");
    let tabs = unsafe {
        tab_control.FindAll(
            TreeScope_Children,
            &automation.CreateTrueCondition().unwrap(),
        )
    }
    .unwrap();
    assert_eq!(unsafe { tabs.Length() }.unwrap(), 2);
    let first = unsafe { tabs.GetElement(0) }.unwrap();
    let second = unsafe { tabs.GetElement(1) }.unwrap();
    assert_eq!(
        unsafe { first.CurrentControlType() }.unwrap(),
        UIA_TabItemControlTypeId.0
    );
    assert!(!unsafe { first.CurrentName() }
        .unwrap()
        .to_string()
        .is_empty());
    assert!(!unsafe { second.CurrentName() }
        .unwrap()
        .to_string()
        .is_empty());
    let pattern: IUIAutomationSelectionItemPattern =
        unsafe { first.GetCurrentPattern(UIA_SelectionItemPatternId) }
            .unwrap()
            .cast()
            .unwrap();
    unsafe { pattern.Select() }.unwrap();
    assert!(unsafe { pattern.CurrentIsSelected() }.unwrap().as_bool());

    child.kill().expect("terminate controlled shell");
    child.wait().expect("reap controlled shell");
}
