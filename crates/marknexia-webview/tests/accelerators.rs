#![cfg(windows)]

use marknexia_webview::host::ShellAccelerator;

const VK_TAB: u16 = 0x09;
const VK_W: u16 = 0x57;
const VK_F6: u16 = 0x75;
const VK_C: u16 = 0x43;
const VK_F: u16 = 0x46;

#[test]
fn only_shell_owned_chords_leave_the_document() {
    assert!(ShellAccelerator::from_key(VK_TAB, true, false, false).is_some());
    assert!(ShellAccelerator::from_key(VK_TAB, true, true, false).is_some());
    assert!(ShellAccelerator::from_key(VK_W, true, false, false).is_some());
    assert!(ShellAccelerator::from_key(VK_F6, false, false, false).is_some());
    assert!(ShellAccelerator::from_key(VK_F6, false, true, false).is_some());

    // Plain Tab moves focus inside the page; copy and find stay in WebView2.
    assert!(ShellAccelerator::from_key(VK_TAB, false, false, false).is_none());
    assert!(ShellAccelerator::from_key(VK_C, true, false, false).is_none());
    assert!(ShellAccelerator::from_key(VK_F, true, false, false).is_none());
    assert!(ShellAccelerator::from_key(VK_W, true, true, false).is_none());
    assert!(ShellAccelerator::from_key(VK_F6, true, false, false).is_none());
    assert!(ShellAccelerator::from_key(VK_TAB, true, false, true).is_none());
}
