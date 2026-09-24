use marknexia_webview::recovery::{RecoveryAction, RecoveryCoordinator};

#[test]
fn renderer_failure_recreates_only_its_tab_once() {
    let mut recovery = RecoveryCoordinator::new(Some(7));
    assert_eq!(
        recovery.record_renderer_failure(7),
        RecoveryAction::RecreateController { tab_id: 7 }
    );
    assert_eq!(recovery.record_renderer_failure(7), RecoveryAction::None);
    recovery.renderer_restored(7);
    assert_eq!(
        recovery.record_renderer_failure(7),
        RecoveryAction::RecreateController { tab_id: 7 }
    );
}

#[test]
fn browser_exit_deduplicates_teardown_and_environment_recreation() {
    let mut recovery = RecoveryCoordinator::new(Some(7));
    assert_eq!(
        recovery.record_browser_exit(),
        RecoveryAction::CloseAllControllers
    );
    assert_eq!(recovery.record_browser_exit(), RecoveryAction::None);
    assert_eq!(recovery.controllers_closed(), RecoveryAction::None);
    assert_eq!(
        recovery.browser_process_exited(),
        RecoveryAction::RecreateEnvironment
    );
    assert_eq!(recovery.browser_process_exited(), RecoveryAction::None);
    assert_eq!(recovery.controllers_closed(), RecoveryAction::None);
}

#[test]
fn browser_process_exit_event_can_arrive_before_controller_failure_notifications() {
    let mut recovery = RecoveryCoordinator::new(Some(7));
    assert_eq!(
        recovery.browser_process_exited(),
        RecoveryAction::CloseAllControllers
    );
    assert_eq!(recovery.record_browser_exit(), RecoveryAction::None);
    assert_eq!(
        recovery.controllers_closed(),
        RecoveryAction::RecreateEnvironment
    );
    assert_eq!(recovery.browser_process_exited(), RecoveryAction::None);
}

#[test]
fn second_browser_failure_waits_for_its_own_exit_event() {
    let mut recovery = RecoveryCoordinator::new(None);
    assert_eq!(
        recovery.record_browser_exit(),
        RecoveryAction::CloseAllControllers
    );
    assert_eq!(recovery.controllers_closed(), RecoveryAction::None);
    assert_eq!(
        recovery.browser_process_exited(),
        RecoveryAction::RecreateEnvironment
    );
    assert_eq!(recovery.environment_restored(), RecoveryAction::None);

    assert_eq!(
        recovery.record_browser_exit(),
        RecoveryAction::CloseAllControllers
    );
    assert_eq!(recovery.controllers_closed(), RecoveryAction::None);
    assert_eq!(
        recovery.browser_process_exited(),
        RecoveryAction::RecreateEnvironment
    );
}

#[test]
fn browser_recovery_restores_active_tab_identity_once() {
    let mut recovery = RecoveryCoordinator::new(Some(12));
    recovery.record_browser_exit();
    recovery.controllers_closed();
    recovery.browser_process_exited();
    assert_eq!(
        recovery.environment_restored(),
        RecoveryAction::RestoreActiveTab { tab_id: 12 }
    );
    assert_eq!(recovery.environment_restored(), RecoveryAction::None);
    recovery.active_tab_restored(12);
    assert_eq!(recovery.environment_restored(), RecoveryAction::None);
}

#[test]
fn active_tab_snapshot_is_frozen_during_browser_recovery() {
    let mut recovery = RecoveryCoordinator::new(Some(7));
    recovery.set_active_tab(Some(12));
    recovery.record_browser_exit();
    recovery.set_active_tab(Some(99));
    recovery.controllers_closed();
    recovery.browser_process_exited();
    assert_eq!(
        recovery.environment_restored(),
        RecoveryAction::RestoreActiveTab { tab_id: 12 }
    );
}

#[test]
fn browser_exit_supersedes_pending_renderer_recovery() {
    let mut recovery = RecoveryCoordinator::new(Some(7));
    recovery.record_renderer_failure(7);
    assert_eq!(
        recovery.record_browser_exit(),
        RecoveryAction::CloseAllControllers
    );
    assert_eq!(recovery.record_renderer_failure(7), RecoveryAction::None);
}

#[test]
fn close_during_callback_prevents_any_recovery_action() {
    let mut recovery = RecoveryCoordinator::new(Some(7));
    recovery.record_browser_exit();
    recovery.close();
    assert_eq!(recovery.controllers_closed(), RecoveryAction::None);
    assert_eq!(recovery.browser_process_exited(), RecoveryAction::None);
    assert_eq!(recovery.environment_restored(), RecoveryAction::None);
    assert_eq!(recovery.record_renderer_failure(7), RecoveryAction::None);
    assert_eq!(recovery.record_browser_exit(), RecoveryAction::None);
}
