//! Activation: the nonce-bound health check, health failure and timeout,
//! crash, interrupted switches, rollback and recovery. The launched
//! "process" and the clock are fakes; payload folders hold text files.

mod support;

use std::{path::Path, time::Duration};

use marknexia_update::{
    activation::{
        self, ActivateError, ActiveRecord, HEALTH_ARGUMENT, HealthFailure, Probes, Recovery, Route,
        activate, health_nonce_from_arguments, read_active, read_transaction, recover, route,
        write_health_ack,
    },
    layout::Layout,
    stage::StagedPayload,
    version::Version,
};
use support::*;

const ALL_OK: Probes = Probes {
    shell_visible: true,
    web_view_ready: true,
    offline_render: true,
};

struct Setup {
    _temp: TempDir,
    layout: Layout,
}

fn setup(label: &str) -> Setup {
    let temp = TempDir::new(label);
    let layout = Layout::new(temp.path().join("Updates"));
    std::fs::create_dir_all(layout.versions()).unwrap();
    Setup {
        _temp: temp,
        layout,
    }
}

fn payload(layout: &Layout, version: &str) -> StagedPayload {
    let id = format!("{version}-x64");
    let directory = layout.payload(&id);
    std::fs::create_dir_all(&directory).unwrap();
    let shell = directory.join("shell.txt");
    std::fs::write(&shell, b"TEST shell placeholder").unwrap();
    StagedPayload {
        version: Version::parse(version).unwrap(),
        id,
        directory,
        shell,
    }
}

fn make_active(layout: &Layout, version: &str) -> ActiveRecord {
    let record = ActiveRecord::new(&format!("{version}-x64"), &Version::parse(version).unwrap());
    std::fs::write(layout.active_file(), serde_json::to_vec(&record).unwrap()).unwrap();
    record
}

/// A launcher whose child acknowledges like the new shell would.
fn acknowledging(pid: u32, probes: Probes) -> FakeLauncher {
    let launcher = FakeLauncher::new(pid);
    *launcher.on_launch.borrow_mut() = Some(Box::new(move |shell: &Path, nonce, pid| {
        write_health_ack(shell, &nonce.to_hex(), pid, probes).expect("ack");
    }));
    launcher
}

fn clock(step_ms: u64) -> impl Fn() -> Box<dyn activation::Clock> {
    move || Box::new(FakeClock::new(Duration::from_millis(step_ms))) as Box<dyn activation::Clock>
}

#[test]
fn a_healthy_new_version_is_committed_and_keeps_the_rollback_target() {
    let s = setup("activate-ok");
    let old = payload(&s.layout, "1.1.0");
    let older = payload(&s.layout, "1.0.0");
    let previous = make_active(&s.layout, "1.1.0");
    let new = payload(&s.layout, "1.2.0");
    let launcher = acknowledging(4242, ALL_OK);
    activate(&s.layout, &new, &nonce(7), &launcher, &clock(250)).expect("committed");
    let active = read_active(&s.layout).unwrap().unwrap();
    assert_eq!(active.payload, "1.2.0-x64");
    assert_eq!(read_transaction(&s.layout).unwrap(), None);
    // The previous payload stays for rollback; older ones are removed.
    assert!(old.directory.is_dir());
    assert!(!older.directory.exists());
    assert_ne!(previous, active);
    // The shell started with the nonce argument contract.
    let launched = launcher.launched.borrow();
    assert_eq!(launched[0].0, new.shell);
    assert_eq!(launched[0].1, nonce(7).to_hex());
    assert!(!launcher.terminated.get());
}

#[test]
fn a_failed_probe_rolls_back_to_the_previous_version() {
    for probes in [
        Probes {
            shell_visible: false,
            ..ALL_OK
        },
        Probes {
            web_view_ready: false,
            ..ALL_OK
        },
        Probes {
            offline_render: false,
            ..ALL_OK
        },
    ] {
        let s = setup("activate-probe");
        payload(&s.layout, "1.1.0");
        let previous = make_active(&s.layout, "1.1.0");
        let new = payload(&s.layout, "1.2.0");
        let launcher = acknowledging(100, probes);
        assert_eq!(
            activate(&s.layout, &new, &nonce(1), &launcher, &clock(250)),
            Err(ActivateError::RolledBack(HealthFailure::ProbeFailed))
        );
        assert_eq!(read_active(&s.layout).unwrap(), Some(previous));
        assert!(!new.directory.exists(), "the failed payload is removed");
        assert!(launcher.terminated.get());
        assert_eq!(read_transaction(&s.layout).unwrap(), None);
        let failure: serde_json::Value =
            serde_json::from_slice(&std::fs::read(s.layout.failure_file()).unwrap()).unwrap();
        assert_eq!(failure["version"], "1.2.0");
    }
}

#[test]
fn no_acknowledgement_within_sixty_seconds_rolls_back() {
    let s = setup("activate-timeout");
    payload(&s.layout, "1.1.0");
    let previous = make_active(&s.layout, "1.1.0");
    let new = payload(&s.layout, "1.2.0");
    let launcher = FakeLauncher::new(55);
    let ticks = std::rc::Rc::new(std::cell::Cell::new(0_u32));
    let counted = ticks.clone();
    let make_clock = move || {
        let clock = FakeClock::new(Duration::from_millis(250));
        let counted = counted.clone();
        *clock.on_pause.borrow_mut() = Some(Box::new(move |_| counted.set(counted.get() + 1)));
        Box::new(clock) as Box<dyn activation::Clock>
    };
    assert_eq!(
        activate(&s.layout, &new, &nonce(2), &launcher, &make_clock),
        Err(ActivateError::RolledBack(HealthFailure::TimedOut))
    );
    // 60 s at 250 ms per poll.
    assert_eq!(ticks.get(), 240);
    assert_eq!(read_active(&s.layout).unwrap(), Some(previous));
    assert!(launcher.terminated.get());
}

#[test]
fn a_late_acknowledgement_after_the_deadline_is_not_accepted() {
    let s = setup("activate-late");
    payload(&s.layout, "1.1.0");
    let previous = make_active(&s.layout, "1.1.0");
    let new = payload(&s.layout, "1.2.0");
    let launcher = FakeLauncher::new(56);
    let layout = s.layout.clone();
    let shell = new.shell.clone();
    let make_clock = move || {
        let clock = FakeClock::new(Duration::from_secs(1));
        let shell = shell.clone();
        let _ = &layout;
        *clock.on_pause.borrow_mut() = Some(Box::new(move |now| {
            if now == Duration::from_secs(61) {
                let _ = write_health_ack(&shell, &nonce(3).to_hex(), 56, ALL_OK);
            }
        }));
        Box::new(clock) as Box<dyn activation::Clock>
    };
    assert_eq!(
        activate(&s.layout, &new, &nonce(3), &launcher, &make_clock),
        Err(ActivateError::RolledBack(HealthFailure::TimedOut))
    );
    assert_eq!(read_active(&s.layout).unwrap(), Some(previous));
}

#[test]
fn a_crash_before_acknowledging_rolls_back() {
    let s = setup("activate-crash");
    payload(&s.layout, "1.1.0");
    let previous = make_active(&s.layout, "1.1.0");
    let new = payload(&s.layout, "1.2.0");
    let launcher = FakeLauncher::new(77);
    launcher.exit.set(Some(Some(0xC000_0005_u32 as i32)));
    assert_eq!(
        activate(&s.layout, &new, &nonce(4), &launcher, &clock(250)),
        Err(ActivateError::RolledBack(HealthFailure::Exited(Some(
            0xC000_0005_u32 as i32
        ))))
    );
    assert_eq!(read_active(&s.layout).unwrap(), Some(previous));
    // An acknowledgement from a process that then exited is not enough.
    let s = setup("activate-ack-exit");
    payload(&s.layout, "1.1.0");
    let previous = make_active(&s.layout, "1.1.0");
    let new = payload(&s.layout, "1.2.0");
    let launcher = acknowledging(78, ALL_OK);
    launcher.exit.set(Some(Some(3)));
    assert_eq!(
        activate(&s.layout, &new, &nonce(5), &launcher, &clock(250)),
        Err(ActivateError::RolledBack(HealthFailure::Exited(Some(3))))
    );
    assert_eq!(read_active(&s.layout).unwrap(), Some(previous));
}

#[test]
fn a_launch_failure_rolls_back_and_a_first_update_restores_no_pointer() {
    let s = setup("activate-launch");
    let new = payload(&s.layout, "1.2.0");
    let mut launcher = FakeLauncher::new(1);
    launcher.fail = true;
    assert_eq!(
        activate(&s.layout, &new, &nonce(6), &launcher, &clock(250)),
        Err(ActivateError::RolledBack(HealthFailure::LaunchFailed(
            std::io::ErrorKind::PermissionDenied
        )))
    );
    // There was no pointer before: the unpacked installation runs again.
    assert!(!s.layout.active_file().exists());
    assert!(!new.directory.exists());
}

#[test]
fn the_acknowledgement_is_bound_to_the_nonce_and_the_launched_process() {
    let s = setup("activate-nonce");
    payload(&s.layout, "1.1.0");
    let previous = make_active(&s.layout, "1.1.0");
    let new = payload(&s.layout, "1.2.0");
    // The child writes the ack under another pid (another process).
    let launcher = FakeLauncher::new(900);
    *launcher.on_launch.borrow_mut() = Some(Box::new(|shell: &Path, nonce, _| {
        write_health_ack(shell, &nonce.to_hex(), 901, ALL_OK).unwrap();
    }));
    assert_eq!(
        activate(&s.layout, &new, &nonce(8), &launcher, &clock(1000)),
        Err(ActivateError::RolledBack(HealthFailure::TimedOut)),
        "an acknowledgement for another process is ignored"
    );
    assert_eq!(read_active(&s.layout).unwrap(), Some(previous));

    // A wrong nonce cannot be acknowledged at all.
    let s = setup("activate-nonce-2");
    let new = payload(&s.layout, "1.2.0");
    let launcher = FakeLauncher::new(902);
    let wrote = std::rc::Rc::new(std::cell::Cell::new(None));
    let seen = wrote.clone();
    *launcher.on_launch.borrow_mut() = Some(Box::new(move |shell: &Path, _, pid| {
        seen.set(Some(write_health_ack(
            shell,
            &nonce(9).to_hex(),
            pid,
            ALL_OK,
        )));
    }));
    assert!(activate(&s.layout, &new, &nonce(10), &launcher, &clock(1000)).is_err());
    assert_eq!(wrote.get(), Some(Err(activation::AckError::WrongNonce)));
}

#[test]
fn an_acknowledgement_needs_a_transaction_for_its_own_payload() {
    let s = setup("ack-scope");
    let new = payload(&s.layout, "1.2.0");
    // No transaction: a stray `--post-update-health` writes nothing.
    assert_eq!(
        write_health_ack(&new.shell, &nonce(1).to_hex(), 1, ALL_OK),
        Err(activation::AckError::NotAnUpdate)
    );
    // An executable outside `versions` is never an update.
    assert_eq!(
        write_health_ack(
            &s.layout.root().join("shell.txt"),
            &nonce(1).to_hex(),
            1,
            ALL_OK
        ),
        Err(activation::AckError::NotAnUpdate)
    );
    // A transaction for another payload.
    let other = payload(&s.layout, "1.3.0");
    activation::begin(&s.layout, &other, &nonce(1), u32::MAX).unwrap();
    assert_eq!(
        write_health_ack(&new.shell, &nonce(1).to_hex(), 1, ALL_OK),
        Err(activation::AckError::NotAnUpdate)
    );
    assert!(!s.layout.health().exists());
}

#[test]
fn an_interrupted_switch_is_rolled_back_on_the_next_start() {
    let s = setup("recover");
    payload(&s.layout, "1.1.0");
    let previous = make_active(&s.layout, "1.1.0");
    let new = payload(&s.layout, "1.2.0");
    // The coordinator switched the pointer, then died.
    let dead_coordinator = 4_000_000;
    activation::begin(&s.layout, &new, &nonce(1), dead_coordinator).unwrap();
    assert_eq!(
        read_active(&s.layout).unwrap().unwrap().payload,
        "1.2.0-x64"
    );
    // While the coordinator still runs, nothing changes.
    assert_eq!(recover(&s.layout, &|_| true), Recovery::InProgress);
    assert_eq!(
        read_active(&s.layout).unwrap().unwrap().payload,
        "1.2.0-x64"
    );
    assert_eq!(
        recover(&s.layout, &|_| false),
        Recovery::RolledBack("1.2.0".to_owned())
    );
    assert_eq!(read_active(&s.layout).unwrap(), Some(previous));
    assert_eq!(read_transaction(&s.layout).unwrap(), None);
    assert!(!new.directory.exists());
    assert_eq!(recover(&s.layout, &|_| false), Recovery::Nothing);
}

#[test]
fn a_transaction_written_before_the_pointer_switch_also_recovers() {
    let s = setup("recover-early");
    payload(&s.layout, "1.1.0");
    let previous = make_active(&s.layout, "1.1.0");
    let new = payload(&s.layout, "1.2.0");
    activation::begin(&s.layout, &new, &nonce(1), 4_000_001).unwrap();
    // Simulate a crash between the two writes: the pointer is still old.
    std::fs::write(
        s.layout.active_file(),
        serde_json::to_vec(&previous).unwrap(),
    )
    .unwrap();
    assert_eq!(
        recover(&s.layout, &|_| false),
        Recovery::RolledBack("1.2.0".to_owned())
    );
    assert_eq!(read_active(&s.layout).unwrap(), Some(previous));
}

#[test]
fn a_damaged_transaction_or_pointer_falls_back_to_the_unpacked_installation() {
    let s = setup("recover-damaged");
    make_active(&s.layout, "1.1.0");
    std::fs::write(s.layout.transaction_file(), b"{ not json").unwrap();
    assert_eq!(recover(&s.layout, &|_| false), Recovery::PointerReset);
    assert!(!s.layout.active_file().exists());
    assert!(!s.layout.transaction_file().exists());
    // A pointer that escapes `versions` is invalid and never followed.
    std::fs::write(
        s.layout.active_file(),
        br#"{"schema":"marknexia-active-v1","payload":"..\\..\\evil","version":"9.0.0"}"#,
    )
    .unwrap();
    assert!(read_active(&s.layout).is_err());
    assert_eq!(
        route(
            &s.layout,
            Path::new(r"C:\portable\marknexia-win32.exe"),
            &Version::parse("1.0.0").unwrap(),
            "shell.txt"
        ),
        Route::RunHere
    );
}

#[test]
fn a_normal_start_forwards_only_to_a_newer_existing_active_payload() {
    let s = setup("route");
    let running = Path::new(r"C:\portable\marknexia-win32.exe");
    let v100 = Version::parse("1.0.0").unwrap();
    assert_eq!(
        route(&s.layout, running, &v100, "shell.txt"),
        Route::RunHere
    );
    let new = payload(&s.layout, "1.2.0");
    make_active(&s.layout, "1.2.0");
    assert_eq!(
        route(&s.layout, running, &v100, "shell.txt"),
        Route::Forward(new.shell.clone())
    );
    // The active payload itself, or a newer running version, runs here.
    assert_eq!(
        route(
            &s.layout,
            &new.shell,
            &Version::parse("1.2.0").unwrap(),
            "shell.txt"
        ),
        Route::RunHere
    );
    assert_eq!(
        route(
            &s.layout,
            running,
            &Version::parse("1.3.0").unwrap(),
            "shell.txt"
        ),
        Route::RunHere
    );
    // A missing shell never strands the user.
    std::fs::remove_file(&new.shell).unwrap();
    assert_eq!(
        route(&s.layout, running, &v100, "shell.txt"),
        Route::RunHere
    );
}

#[test]
fn health_mode_needs_exactly_the_flag_and_a_valid_nonce() {
    use std::ffi::OsString;
    let hex = nonce(0xab).to_hex();
    let ok = [OsString::from(HEALTH_ARGUMENT), OsString::from(&hex)];
    assert_eq!(health_nonce_from_arguments(&ok), Some(hex.clone()));
    for args in [
        vec![OsString::from(HEALTH_ARGUMENT)],
        vec![OsString::from(HEALTH_ARGUMENT), OsString::from("abc")],
        vec![
            OsString::from(HEALTH_ARGUMENT),
            OsString::from(&hex),
            OsString::from("doc.md"),
        ],
        vec![OsString::from("doc.md"), OsString::from(&hex)],
        vec![
            OsString::from(HEALTH_ARGUMENT),
            OsString::from(hex.to_uppercase()),
        ],
    ] {
        assert_eq!(health_nonce_from_arguments(&args), None, "{args:?}");
    }
}

#[test]
fn a_second_activation_waits_for_the_first() {
    let s = setup("busy");
    let new = payload(&s.layout, "1.2.0");
    activation::begin(&s.layout, &new, &nonce(1), u32::MAX).unwrap();
    let launcher = acknowledging(1, ALL_OK);
    assert_eq!(
        activate(&s.layout, &new, &nonce(2), &launcher, &clock(250)),
        Err(ActivateError::Begin(activation::ActivationError::Busy))
    );
    assert!(launcher.launched.borrow().is_empty());
}
