//! `marknexia-rust-preview-setup.exe`: the per-user setup of
//! Marknexia (Rust preview). See `marknexia_setup` and `cli::USAGE`.
//!
//! Built and run only on CI runners (it embeds a real executable).

#![windows_subsystem = "windows"]

mod native;

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use marknexia_setup::cli::{self, Command, Mode, exit};
use marknexia_setup::identity::PRODUCT_NAME;
use marknexia_setup::install::{self, Associations, Host};
use marknexia_setup::layout::InstallLayout;
use marknexia_setup::names::FileNames;
use marknexia_setup::payload::{self, VerifiedPayload};
use marknexia_setup::registry;

use native::Icon;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const TITLE: &str = "Marknexia (Rust preview) Setup";

/// Appends lines to `--log <file>`; silent otherwise.
struct Log(Option<PathBuf>);

impl Log {
    fn line(&self, text: &str) {
        if let Some(path) = &self.0
            && let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path)
        {
            let _ = writeln!(file, "{text}");
        }
    }
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let silent_requested = arguments
        .iter()
        .any(|argument| argument.eq_ignore_ascii_case("--silent"));
    let command = match cli::parse(arguments) {
        Ok(command) => command,
        Err(error) => {
            if !silent_requested {
                native::message(TITLE, &format!("{error}\n\n{}", cli::USAGE), Icon::Error);
            }
            return exit::USAGE;
        }
    };
    let log = Log(command.log.clone());
    log.line(&format!(
        "{PRODUCT_NAME} setup {VERSION}: {:?}",
        command.mode
    ));
    let code = match &command.mode {
        Mode::Help => {
            if !command.silent {
                native::message(TITLE, cli::USAGE, Icon::Information);
            }
            exit::OK
        }
        Mode::Verify { report } => verify(report.as_deref(), &log),
        Mode::Install { associations } => run_install(&command, *associations, &log),
        Mode::Uninstall => run_uninstall(&command, &log),
    };
    log.line(&format!("exit code {code}"));
    code
}

fn fail(command: &Command, log: &Log, text: &str) -> i32 {
    log.line(&format!("error: {text}"));
    if !command.silent {
        native::message(TITLE, text, Icon::Error);
    }
    exit::FAILED
}

fn verify(report: Option<&Path>, log: &Log) -> i32 {
    match VerifiedPayload::verify(payload::embedded(), &FileNames::release()) {
        Ok(verified) => {
            let json = verified.report(VERSION);
            for file in verified.entries() {
                log.line(&format!(
                    "verified {} ({} bytes)",
                    file.name,
                    file.bytes.len()
                ));
            }
            if let Some(path) = report {
                let text = serde_json::to_string_pretty(&json).unwrap_or_default() + "\n";
                if let Err(error) = std::fs::write(path, text) {
                    log.line(&format!("error: the report could not be written: {error}"));
                    return exit::FAILED;
                }
            }
            exit::OK
        }
        Err(error) => {
            log.line(&format!("error: {error}"));
            exit::FAILED
        }
    }
}

fn layout() -> Result<InstallLayout, String> {
    let (local, programs) = native::user_folders()
        .map_err(|error| format!("The user's folders could not be found: {error}"))?;
    InstallLayout::new(&local, &programs).map_err(|error| error.to_string())
}

fn token() -> String {
    use std::hash::BuildHasher;
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let value = std::collections::hash_map::RandomState::new().hash_one((seed, std::process::id()));
    format!("{value:016x}")
}

fn run_install(command: &Command, associations: Associations, log: &Log) -> i32 {
    // The payload is checked before anything is asked or written.
    let verified = match VerifiedPayload::verify(payload::embedded(), &FileNames::release()) {
        Ok(verified) => verified,
        Err(error) => return fail(command, log, &error.to_string()),
    };
    log.line(&format!(
        "payload verified: {} files, {} bytes",
        verified.entries().len(),
        verified.total_bytes()
    ));
    let layout = match layout() {
        Ok(layout) => layout,
        Err(text) => return fail(command, log, &text),
    };
    let mut registry_backend = native::HkcuRegistry;
    let mut associations = associations;
    if !command.silent {
        let question = format!(
            "Install {PRODUCT_NAME} {VERSION} for the current user?\n\n\
             It installs to {} beside Marknexia and does not change the Marknexia installation.\n\n\
             This is an UNSIGNED PREVIEW for testing.",
            layout.install_dir().display()
        );
        if !native::ask(TITLE, &question, true) {
            log.line("cancelled by the user");
            return exit::CANCELLED;
        }
        if associations == Associations::Keep
            && !registry::is_associated(&registry_backend).unwrap_or(false)
        {
            let question = format!(
                "Add {PRODUCT_NAME} to \"Open with\" for Markdown files (.md, .markdown, .mdown, .mkdn)?\n\n\
                 It does not become the default app. You can choose it later in Windows Settings > Apps > Default apps."
            );
            if native::ask(TITLE, &question, false) {
                associations = Associations::Enable;
            }
        }
    }
    if let Err(error) = native::init_com() {
        return fail(
            command,
            log,
            &format!("Setup could not start Windows shell services: {error}"),
        );
    }
    let setup_path = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            return fail(
                command,
                log,
                &format!("The setup executable path could not be determined: {error}"),
            );
        }
    };
    let temp_dir = std::env::temp_dir();
    let mut shell = native::NativeShell;
    let mut next_token = token;
    let mut host = Host {
        registry: &mut registry_backend,
        shell: &mut shell,
        setup_path: &setup_path,
        temp_dir: &temp_dir,
        token: &mut next_token,
    };
    match install::install(&layout, &verified, &mut host, VERSION, associations) {
        Ok(report) => {
            log.line(&format!(
                "installed {} to {} (upgrade: {}, associations: {})",
                report.version,
                report.install_dir.display(),
                report.upgraded,
                report.associated
            ));
            log.line(&format!("settings: {}", report.settings));
            for warning in &report.warnings {
                log.line(&format!("warning: {warning}"));
            }
            let runtime = native::webview2_runtime_version();
            log.line(&format!(
                "WebView2 runtime: {}",
                runtime.as_deref().unwrap_or("not found")
            ));
            if !command.silent {
                let mut text = format!(
                    "{PRODUCT_NAME} {VERSION} was installed. It is available from the Start menu."
                );
                if runtime.is_none() {
                    text.push_str(
                        "\n\nMicrosoft Edge WebView2 Runtime was not found. Marknexia offers to install it when it starts.",
                    );
                }
                native::message(TITLE, &text, Icon::Information);
            }
            exit::OK
        }
        Err(error) => fail(command, log, &error.to_string()),
    }
}

fn run_uninstall(command: &Command, log: &Log) -> i32 {
    let layout = match layout() {
        Ok(layout) => layout,
        Err(text) => return fail(command, log, &text),
    };
    if !command.silent
        && !native::ask(
            TITLE,
            &format!(
                "Uninstall {PRODUCT_NAME}?\n\nMarknexia and the shared settings file are not changed."
            ),
            false,
        )
    {
        log.line("cancelled by the user");
        return exit::CANCELLED;
    }
    let setup_path = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            return fail(
                command,
                log,
                &format!("The setup executable path could not be determined: {error}"),
            );
        }
    };
    let temp_dir = std::env::temp_dir();
    let mut registry_backend = native::HkcuRegistry;
    let mut shell = native::NativeShell;
    let mut next_token = token;
    let mut host = Host {
        registry: &mut registry_backend,
        shell: &mut shell,
        setup_path: &setup_path,
        temp_dir: &temp_dir,
        token: &mut next_token,
    };
    match install::uninstall(&layout, &mut host) {
        Ok(report) => {
            for path in &report.removed {
                log.line(&format!("removed {}", path.display()));
            }
            for path in &report.update_state.removed {
                log.line(&format!("removed update state {}", path.display()));
            }
            for path in report.kept.iter().chain(&report.update_state.kept) {
                log.line(&format!("kept {}", path.display()));
            }
            if let Some(moved) = &report.setup_moved_to {
                log.line(&format!("the running setup moved to {}", moved.display()));
            }
            log.line(&format!(
                "install folder removed: {}",
                report.install_dir_removed
            ));
            if !command.silent {
                native::message(
                    TITLE,
                    &format!("{PRODUCT_NAME} was uninstalled."),
                    Icon::Information,
                );
            }
            exit::OK
        }
        Err(error) => fail(command, log, &error.to_string()),
    }
}
