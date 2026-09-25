use marknexia_win32::{
    app::{AppState, FocusSurface},
    keyboard::ShellCommand,
};

#[test]
fn shell_commands_update_tabs_and_cycle_a_logical_focus_order() {
    let mut app = AppState::new();
    let first = app.open_tab("First.md").unwrap();
    let second = app.open_tab("Second.md").unwrap();

    assert_eq!(app.active_tab(), Some(second));
    assert!(app.apply_command(ShellCommand::PreviousTab));
    assert_eq!(app.active_tab(), Some(first));
    assert!(app.apply_command(ShellCommand::CloseTab));
    assert_eq!(app.active_tab(), Some(second));

    assert_eq!(app.focused_surface(), FocusSurface::CommandBar);
    for expected in [
        FocusSurface::TabStrip,
        FocusSurface::Repository,
        FocusSurface::Document,
        FocusSurface::FindBar,
        FocusSurface::Status,
        FocusSurface::CommandBar,
    ] {
        assert!(app.apply_command(ShellCommand::CycleFocus));
        assert_eq!(app.focused_surface(), expected);
    }
}

#[test]
fn focus_visibility_tracks_keyboard_and_pointer_input() {
    let mut app = AppState::new();
    assert!(!app.focus_is_visible());

    app.note_keyboard_input();
    assert!(app.focus_is_visible());

    app.note_pointer_input();
    assert!(!app.focus_is_visible());
}
