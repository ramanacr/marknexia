#![cfg(windows)]

use marknexia_webview::host::{ShellAccelerator, scroll_script};

const VK_TAB: u16 = 0x09;
const VK_W: u16 = 0x57;
const VK_F6: u16 = 0x75;
const VK_C: u16 = 0x43;
const VK_F: u16 = 0x46;
const VK_F3: u16 = 0x72;
const VK_ESCAPE: u16 = 0x1B;

#[test]
fn only_shell_owned_chords_leave_the_document() {
    assert!(ShellAccelerator::from_key(VK_TAB, true, false, false).is_some());
    assert!(ShellAccelerator::from_key(VK_TAB, true, true, false).is_some());
    assert!(ShellAccelerator::from_key(VK_W, true, false, false).is_some());
    assert!(ShellAccelerator::from_key(VK_F6, false, false, false).is_some());
    assert!(ShellAccelerator::from_key(VK_F6, false, true, false).is_some());

    // The shell's find chords leave the page, so WebView2's own find bar
    // never opens.
    assert!(ShellAccelerator::from_key(VK_F, true, false, false).is_some());
    assert!(ShellAccelerator::from_key(VK_F3, false, false, false).is_some());
    assert!(ShellAccelerator::from_key(VK_F3, false, true, false).is_some());

    // Plain Tab moves focus inside the page; copy and Escape stay in WebView2.
    assert!(ShellAccelerator::from_key(VK_TAB, false, false, false).is_none());
    assert!(ShellAccelerator::from_key(VK_C, true, false, false).is_none());
    assert!(ShellAccelerator::from_key(VK_F, true, true, false).is_none());
    assert!(ShellAccelerator::from_key(VK_F3, true, false, false).is_none());
    assert!(ShellAccelerator::from_key(VK_ESCAPE, false, false, false).is_none());
    assert!(ShellAccelerator::from_key(VK_W, true, true, false).is_none());
    assert!(ShellAccelerator::from_key(VK_F6, true, false, false).is_none());
    assert!(ShellAccelerator::from_key(VK_TAB, true, false, true).is_none());
}

#[test]
fn dotnet_command_accelerators_leave_the_document() {
    const VK_LEFT: u16 = 0x25;
    const VK_RIGHT: u16 = 0x27;
    const VK_O: u16 = 0x4F;
    const VK_R: u16 = 0x52;
    const VK_T: u16 = 0x54;
    const VK_F5: u16 = 0x74;
    for (key, ctrl, shift, alt) in [
        (VK_O, true, false, false),
        (VK_O, true, true, false),
        (VK_T, true, false, false),
        (VK_R, true, false, false),
        (VK_F5, false, false, false),
        (VK_LEFT, false, false, true),
        (VK_RIGHT, false, false, true),
    ] {
        let chord = ShellAccelerator::from_key(key, ctrl, shift, alt).expect("command chord");
        assert_eq!((chord.ctrl, chord.shift, chord.alt), (ctrl, shift, alt));
    }
    // Plain arrows scroll the page; Ctrl+Shift+R and Shift+F5 stay in WebView2.
    assert!(ShellAccelerator::from_key(VK_LEFT, false, false, false).is_none());
    assert!(ShellAccelerator::from_key(VK_R, true, true, false).is_none());
    assert!(ShellAccelerator::from_key(VK_F5, false, true, false).is_none());
    assert!(ShellAccelerator::from_key(VK_LEFT, true, false, true).is_none());
}

#[test]
fn scroll_script_json_encodes_the_anchor() {
    assert_eq!(scroll_script(None), "window.marknexiaBridge?.scrollToTop()");
    assert_eq!(
        scroll_script(Some("")),
        "window.marknexiaBridge?.scrollToTop()"
    );
    assert_eq!(
        scroll_script(Some("intro")),
        r#"window.marknexiaBridge?.scrollToAnchor("intro")"#
    );
    assert_eq!(
        scroll_script(Some("a\");alert(1);//")),
        r#"window.marknexiaBridge?.scrollToAnchor("a\");alert(1);//")"#
    );
}
