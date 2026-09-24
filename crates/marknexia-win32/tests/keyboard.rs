use marknexia_win32::keyboard::{KeyChord, ShellCommand, route_key};

#[test]
fn tab_shortcuts_route_without_swallowing_system_keys() {
    assert_eq!(
        route_key(KeyChord::new(0x09, true, false, false)),
        Some(ShellCommand::NextTab)
    );
    assert_eq!(
        route_key(KeyChord::new(0x09, true, true, false)),
        Some(ShellCommand::PreviousTab)
    );
    assert_eq!(
        route_key(KeyChord::new(0x57, true, false, false)),
        Some(ShellCommand::CloseTab)
    );
    assert_eq!(
        route_key(KeyChord::new(0x75, false, false, false)),
        Some(ShellCommand::CycleFocus)
    );
    assert_eq!(route_key(KeyChord::new(0x73, false, false, true)), None); // Alt+F4 belongs to Windows.
    assert_eq!(route_key(KeyChord::new(0x09, false, false, false)), None); // Plain Tab stays in focus traversal.
}
