//! Install, upgrade and uninstall against temp folders, a
//! [`MemoryRegistry`] and [`FakeShell`]. Payload files and the "setup"
//! are text stand-ins: no test writes executable bytes or the real
//! registry.

use std::fs;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::identity::PROG_ID;
use crate::install::{Associations, Host, SetupError, install, uninstall};
use crate::layout::InstallLayout;
use crate::names::FileNames;
use crate::payload::VerifiedPayload;
use crate::payload::tests::{sample_container, sample_files};
use crate::registry::{self, MemoryRegistry, RegValue, RegistryBackend};
use crate::test_support::{FakeShell, TempDir};

/// Non-executable stand-in names (`FileNames::stand_ins`): no test writes a
/// file named `*.exe` or `*.dll`.
const SHELL_STAND_IN: &str = "shell.bin.txt";
const SETUP_STAND_IN: &str = "setup.bin.txt";

struct Fixture {
    _dir: TempDir,
    layout: InstallLayout,
    local: PathBuf,
    temp: PathBuf,
    setup_source: PathBuf,
    registry: MemoryRegistry,
    shell: FakeShell,
    counter: u32,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let dir = TempDir::new(label);
        let local = dir.path().join("Local");
        let start = dir.path().join("Start Menu").join("Programs");
        let temp = dir.path().join("Temp");
        fs::create_dir_all(&local).expect("local");
        fs::create_dir_all(&temp).expect("temp");
        let setup_source = dir.path().join("Downloads").join("setup-stand-in.txt");
        fs::create_dir_all(setup_source.parent().expect("parent")).expect("downloads");
        fs::write(&setup_source, "setup stand-in (text)").expect("setup");
        Self {
            layout: InstallLayout::with_names(&local, &start, FileNames::stand_ins())
                .expect("layout"),
            _dir: dir,
            local,
            temp,
            setup_source,
            registry: MemoryRegistry::new(),
            shell: FakeShell::default(),
            counter: 0,
        }
    }

    fn run<T>(
        &mut self,
        setup_path: Option<PathBuf>,
        action: impl FnOnce(&InstallLayout, &mut Host<'_>) -> T,
    ) -> T {
        let setup_path = setup_path.unwrap_or_else(|| self.setup_source.clone());
        let counter = &mut self.counter;
        let mut token = || {
            *counter += 1;
            format!("t{counter:04}")
        };
        let mut host = Host {
            registry: &mut self.registry,
            shell: &mut self.shell,
            setup_path: &setup_path,
            temp_dir: &self.temp,
            token: &mut token,
        };
        action(&self.layout, &mut host)
    }

    fn install(
        &mut self,
        associations: Associations,
    ) -> Result<crate::install::InstallReport, SetupError> {
        let bytes = sample_container();
        let payload = VerifiedPayload::verify(&bytes, &FileNames::stand_ins()).expect("payload");
        self.run(None, |layout, host| {
            install(layout, &payload, host, "0.1.0", associations)
        })
    }

    fn uninstall_from_installed_copy(
        &mut self,
    ) -> Result<crate::install::UninstallReport, SetupError> {
        let installed = self.layout.setup_exe();
        self.run(Some(installed), uninstall)
    }

    fn programs_entries(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.layout.programs())
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

fn names_in(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .expect("list")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn expected_install_names() -> Vec<String> {
    let mut names: Vec<String> = FileNames::stand_ins().payload().to_vec();
    names.push(SETUP_STAND_IN.to_owned());
    names.sort();
    names
}

#[test]
fn a_first_install_writes_the_payload_entry_and_shortcut_only() {
    let mut fixture = Fixture::new("first-install");
    let report = fixture.install(Associations::Keep).expect("install");
    assert!(!report.upgraded);
    assert!(!report.associated);

    let install_dir = fixture.layout.install_dir().to_path_buf();
    assert_eq!(names_in(&install_dir), expected_install_names());
    for (name, bytes) in sample_files() {
        assert_eq!(
            fs::read(install_dir.join(&name)).expect("read"),
            bytes,
            "{name}"
        );
    }
    assert_eq!(
        fs::read(install_dir.join(SETUP_STAND_IN)).expect("setup"),
        b"setup stand-in (text)"
    );
    assert_eq!(fixture.programs_entries(), ["Marknexia Rust Preview"]);

    let entry = registry::uninstall_key();
    assert_eq!(
        fixture
            .registry
            .read_string(&entry, Some("DisplayVersion"))
            .unwrap()
            .as_deref(),
        Some("0.1.0")
    );
    assert!(matches!(
        fixture.registry.value(&entry, Some("EstimatedSize")),
        Some(RegValue::Dword(1))
    ));
    assert!(
        !registry::is_associated(&fixture.registry).unwrap(),
        "associations are opt-in"
    );
    assert_eq!(fixture.shell.association_notifications, 0);
    assert_eq!(fixture.shell.shortcuts.len(), 1);
    let (link, target, working_dir) = &fixture.shell.shortcuts[0];
    assert_eq!(link, fixture.layout.shortcut());
    assert_eq!(target, &install_dir.join(SHELL_STAND_IN));
    assert_eq!(working_dir, &install_dir);
}

#[test]
fn install_never_writes_settings_or_updater_state() {
    let mut fixture = Fixture::new("shared-state");
    let marknexia = fixture.local.join("Marknexia");
    let updates = marknexia.join("Updates");
    fs::create_dir_all(updates.join("versions").join("0.0.9-x64")).expect("versions");
    fs::write(updates.join("active.json"), "{\"payload\":\"0.0.9-x64\"}").expect("active");
    fs::write(
        marknexia.join("settings.json"),
        br#"{"AllowRemoteAssets":true}"#,
    )
    .expect("settings");

    let report = fixture.install(Associations::Keep).expect("install");
    assert_eq!(
        report.settings,
        crate::settings_probe::SettingsProbe::Compatible { schema_version: 1 }
    );
    assert_eq!(names_in(&marknexia), ["Updates", "settings.json"]);
    assert_eq!(names_in(&updates), ["active.json", "versions"]);
    assert_eq!(
        fs::read_to_string(updates.join("active.json")).expect("active"),
        "{\"payload\":\"0.0.9-x64\"}"
    );
    assert_eq!(
        fs::read(marknexia.join("settings.json")).expect("settings"),
        br#"{"AllowRemoteAssets":true}"#
    );
}

#[test]
fn associations_are_added_only_on_request_and_kept_on_upgrade() {
    let mut fixture = Fixture::new("associate");
    fixture.install(Associations::Enable).expect("install");
    assert!(registry::is_associated(&fixture.registry).unwrap());
    assert_eq!(fixture.shell.association_notifications, 1);
    assert_eq!(
        fixture
            .registry
            .value(r"Software\Classes\.md\OpenWithProgids", Some(PROG_ID)),
        Some(&RegValue::None)
    );
    assert_eq!(
        fixture
            .registry
            .read_string(r"Software\Classes\.md", None)
            .unwrap(),
        None
    );

    let report = fixture.install(Associations::Keep).expect("upgrade");
    assert!(report.upgraded);
    assert!(report.associated, "an earlier opt-in is kept");
    assert_eq!(
        fixture.programs_entries(),
        ["Marknexia Rust Preview"],
        "no staging or backup is left"
    );

    let report = fixture.install(Associations::Disable).expect("opt out");
    assert!(!report.associated);
    assert!(!registry::is_associated(&fixture.registry).unwrap());
    assert!(
        fixture
            .registry
            .key_exists(&registry::uninstall_key())
            .unwrap()
    );
}

#[test]
fn a_running_shell_blocks_install_before_anything_is_written() {
    let mut fixture = Fixture::new("in-use-install");
    fixture.install(Associations::Keep).expect("install");
    let registry_before = fixture.registry.clone();
    let shell = fixture.layout.shell_exe();
    let _lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&shell)
        .expect("lock");
    let error = fixture.install(Associations::Enable).unwrap_err();
    assert!(matches!(
        error,
        SetupError::InUse {
            uninstall: false,
            ..
        }
    ));
    assert_eq!(
        error.to_string(),
        "Close Marknexia (Rust preview) before installing or upgrading it."
    );
    assert_eq!(fixture.programs_entries(), ["Marknexia Rust Preview"]);
    assert_eq!(fixture.registry, registry_before);
}

#[test]
fn a_failed_registration_rolls_back_a_first_install() {
    let mut fixture = Fixture::new("rollback-first");
    fixture.shell.fail_shortcut = true;
    let error = fixture.install(Associations::Enable).unwrap_err();
    assert!(matches!(error, SetupError::Shortcut(_)));
    assert!(fixture.programs_entries().is_empty());
    assert!(
        !fixture
            .registry
            .key_exists(&registry::uninstall_key())
            .unwrap()
    );
    assert!(!registry::is_associated(&fixture.registry).unwrap());
}

#[test]
fn a_failed_registration_restores_the_previous_installation() {
    let mut fixture = Fixture::new("rollback-upgrade");
    fixture.install(Associations::Keep).expect("install");
    let marker = fixture.layout.install_dir().join("README.txt");
    fs::write(&marker, "previous version").expect("marker");
    fixture.shell.fail_shortcut = true;
    assert!(fixture.install(Associations::Keep).is_err());
    assert_eq!(
        fs::read_to_string(&marker).expect("restored"),
        "previous version"
    );
    assert_eq!(fixture.programs_entries(), ["Marknexia Rust Preview"]);
    assert!(
        fixture
            .registry
            .key_exists(&registry::uninstall_key())
            .unwrap()
    );
}

#[test]
fn leftovers_of_an_interrupted_setup_are_removed() {
    let mut fixture = Fixture::new("leftovers");
    let programs = fixture.layout.programs().to_path_buf();
    for name in [
        "Marknexia Rust Preview.staging-old1",
        "Marknexia Rust Preview.backup-old2",
        "Marknexia",
        "Marknexia.staging-dotnet",
    ] {
        fs::create_dir_all(programs.join(name)).expect("folder");
        fs::write(programs.join(name).join("x.txt"), "x").expect("file");
    }
    let old_copy = fixture
        .temp
        .join("marknexia-rust-preview-uninstall-t0001.txt");
    fs::write(&old_copy, "old uninstall copy stand-in").expect("copy");
    fixture.install(Associations::Keep).expect("install");
    assert_eq!(
        fixture.programs_entries(),
        [
            "Marknexia",
            "Marknexia Rust Preview",
            "Marknexia.staging-dotnet"
        ],
        "the .NET edition's folders are not touched"
    );
    assert!(!old_copy.exists());
}

#[test]
fn uninstall_removes_the_installation_and_keeps_shared_and_unknown_data() {
    let mut fixture = Fixture::new("uninstall");
    let dotnet_dir = fixture.layout.programs().join("Marknexia");
    fs::create_dir_all(&dotnet_dir).expect("dotnet");
    fs::write(dotnet_dir.join("Marknexia.App.exe.txt"), "dotnet stand-in").expect("dotnet file");
    let marknexia = fixture.local.join("Marknexia");
    fs::create_dir_all(&marknexia).expect("marknexia");
    fs::write(marknexia.join("settings.json"), "{}").expect("settings");

    fixture.install(Associations::Enable).expect("install");
    let updates = marknexia.join("Updates");
    fs::create_dir_all(updates.join("versions").join("0.2.0-x64")).expect("versions");
    fs::write(
        updates
            .join("versions")
            .join("0.2.0-x64")
            .join("README.txt"),
        "r",
    )
    .expect("payload");
    fs::write(updates.join("active.json"), "{}").expect("active");
    fs::create_dir_all(updates.join("pending-dotnet")).expect("dotnet pending");

    let report = fixture.uninstall_from_installed_copy().expect("uninstall");
    assert!(report.install_dir_removed);
    let moved = report
        .setup_moved_to
        .expect("the running setup moves itself");
    assert!(moved.starts_with(&fixture.temp));
    assert_eq!(
        fs::read(&moved).expect("moved copy"),
        b"setup stand-in (text)"
    );
    assert!(!fixture.layout.install_dir().exists());
    assert!(!fixture.layout.shortcut().exists());
    assert!(
        !fixture
            .registry
            .key_exists(&registry::uninstall_key())
            .unwrap()
    );
    assert!(!registry::is_associated(&fixture.registry).unwrap());
    assert!(!updates.join("active.json").exists());
    assert!(!updates.join("versions").exists());
    assert!(
        updates.join("pending-dotnet").is_dir(),
        ".NET updater state stays"
    );
    assert!(marknexia.join("settings.json").is_file(), "settings stay");
    assert!(
        dotnet_dir.join("Marknexia.App.exe.txt").is_file(),
        ".NET installation stays"
    );

    // A second run finds nothing and still succeeds.
    let again = fixture.run(None, uninstall).expect("again");
    assert!(again.install_dir_removed);
    assert!(again.removed.is_empty());
}

#[test]
fn uninstall_keeps_files_it_did_not_install() {
    let mut fixture = Fixture::new("uninstall-unknown");
    fixture.install(Associations::Keep).expect("install");
    let note = fixture.layout.install_dir().join("my-notes.md");
    fs::write(&note, "user data").expect("note");
    let report = fixture.run(None, uninstall).expect("uninstall");
    assert!(!report.install_dir_removed);
    assert_eq!(report.kept, std::slice::from_ref(&note));
    assert_eq!(names_in(fixture.layout.install_dir()), ["my-notes.md"]);
    assert!(
        !fixture
            .registry
            .key_exists(&registry::uninstall_key())
            .unwrap()
    );
}

#[test]
fn a_running_staged_update_blocks_uninstall_before_anything_is_removed() {
    let mut fixture = Fixture::new("uninstall-in-use");
    fixture.install(Associations::Enable).expect("install");
    let shell = fixture
        .local
        .join("Marknexia")
        .join("Updates")
        .join("versions")
        .join("0.2.0-x64")
        .join(SHELL_STAND_IN);
    fs::create_dir_all(shell.parent().expect("parent")).expect("payload");
    fs::write(&shell, "staged shell stand-in").expect("shell");
    let registry_before = fixture.registry.clone();
    let _lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&shell)
        .expect("lock");
    let error = fixture.uninstall_from_installed_copy().unwrap_err();
    assert_eq!(
        error.to_string(),
        "Close Marknexia (Rust preview) before uninstalling it."
    );
    assert_eq!(
        names_in(fixture.layout.install_dir()),
        expected_install_names()
    );
    assert_eq!(fixture.registry, registry_before);
    assert!(fixture.layout.shortcut().exists());
}
