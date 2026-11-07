//! Command line.
//!
//! ```text
//! marknexia-rust-preview-setup.exe [--install] [--silent] [--associate | --no-associate] [--log <file>]
//! marknexia-rust-preview-setup.exe --uninstall [--silent] [--log <file>]
//! marknexia-rust-preview-setup.exe --verify [--report <file>] [--log <file>]
//! ```
//!
//! Without `--silent`, setup asks before it changes anything and asks
//! separately about the "Open with" associations (default: no). With
//! `--silent` it asks nothing; associations are added only with
//! `--associate`. `--verify` checks the embedded payload, writes nothing
//! except the optional report, and installs nothing.

use std::fmt;
use std::path::PathBuf;

use crate::install::Associations;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Install { associations: Associations },
    Uninstall,
    Verify { report: Option<PathBuf> },
    Help,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub mode: Mode,
    pub silent: bool,
    pub log: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageError(pub String);

impl fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

pub const USAGE: &str = "Marknexia (Rust preview) setup\n\
  --install [--silent] [--associate | --no-associate] [--log <file>]\n\
  --uninstall [--silent] [--log <file>]\n\
  --verify [--report <file>] [--log <file>]\n\
  --help";

/// Exit codes.
pub mod exit {
    pub const OK: i32 = 0;
    pub const FAILED: i32 = 1;
    pub const USAGE: i32 = 2;
    pub const CANCELLED: i32 = 3;
}

fn absolute(option: &str, value: Option<String>) -> Result<PathBuf, UsageError> {
    let value = value.ok_or_else(|| UsageError(format!("Option {option} requires a value.")))?;
    let path = PathBuf::from(value.trim());
    if !path.is_absolute() {
        return Err(UsageError(format!(
            "Option {option} requires an absolute path."
        )));
    }
    Ok(path)
}

/// Parses the arguments after the program name.
///
/// # Errors
/// An unknown option, a missing or relative path, or conflicting options.
pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Command, UsageError> {
    let mut arguments = arguments.into_iter();
    let mut mode: Option<&'static str> = None;
    let mut silent = false;
    let mut log = None;
    let mut report = None;
    let mut associations = Associations::Keep;
    fn set_mode(new: &'static str, mode: &mut Option<&'static str>) -> Result<(), UsageError> {
        match mode {
            Some(old) if *old != new => Err(UsageError(format!(
                "--{old} and --{new} cannot be combined."
            ))),
            _ => {
                *mode = Some(new);
                Ok(())
            }
        }
    }
    while let Some(argument) = arguments.next() {
        match argument.trim().to_ascii_lowercase().as_str() {
            "--install" => set_mode("install", &mut mode)?,
            "--uninstall" => set_mode("uninstall", &mut mode)?,
            "--verify" => set_mode("verify", &mut mode)?,
            "--help" | "-h" | "/?" => set_mode("help", &mut mode)?,
            "--silent" => silent = true,
            "--associate" => {
                if associations == Associations::Disable {
                    return Err(UsageError(
                        "--associate and --no-associate cannot be combined.".to_owned(),
                    ));
                }
                associations = Associations::Enable;
            }
            "--no-associate" => {
                if associations == Associations::Enable {
                    return Err(UsageError(
                        "--associate and --no-associate cannot be combined.".to_owned(),
                    ));
                }
                associations = Associations::Disable;
            }
            "--log" => log = Some(absolute("--log", arguments.next())?),
            "--report" => report = Some(absolute("--report", arguments.next())?),
            _ => {
                return Err(UsageError(format!(
                    "Unknown setup option '{}'.",
                    argument.trim()
                )));
            }
        }
    }
    let mode = mode.unwrap_or("install");
    if mode != "verify" && report.is_some() {
        return Err(UsageError("--report applies only to --verify.".to_owned()));
    }
    let mode = match mode {
        "uninstall" => Mode::Uninstall,
        "verify" => Mode::Verify { report },
        "help" => Mode::Help,
        _ => Mode::Install { associations },
    };
    if associations != Associations::Keep && !matches!(mode, Mode::Install { .. }) {
        return Err(UsageError(
            "--associate and --no-associate apply only to --install.".to_owned(),
        ));
    }
    Ok(Command { mode, silent, log })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn no_arguments_is_an_interactive_install_that_keeps_associations() {
        assert_eq!(
            parse(Vec::new()).unwrap(),
            Command {
                mode: Mode::Install {
                    associations: Associations::Keep
                },
                silent: false,
                log: None
            }
        );
    }

    #[test]
    fn silent_install_and_uninstall() {
        let command = parse(args(
            r"--install --silent --associate --log C:\ci\setup.log",
        ))
        .unwrap();
        assert_eq!(
            command.mode,
            Mode::Install {
                associations: Associations::Enable
            }
        );
        assert!(command.silent);
        assert_eq!(command.log, Some(PathBuf::from(r"C:\ci\setup.log")));
        let command = parse(args("--uninstall --silent")).unwrap();
        assert_eq!(command.mode, Mode::Uninstall);
        assert!(command.silent);
        assert_eq!(
            parse(args("--NO-ASSOCIATE")).unwrap().mode,
            Mode::Install {
                associations: Associations::Disable
            }
        );
    }

    #[test]
    fn verify_takes_an_absolute_report_path() {
        assert_eq!(
            parse(args(r"--verify --report C:\ci\payload.json"))
                .unwrap()
                .mode,
            Mode::Verify {
                report: Some(PathBuf::from(r"C:\ci\payload.json"))
            }
        );
        assert!(parse(args("--verify --report payload.json")).is_err());
        assert!(parse(args("--verify --report")).is_err());
    }

    #[test]
    fn conflicting_or_unknown_options_are_refused() {
        for text in [
            "--install --uninstall",
            "--associate --no-associate",
            "--uninstall --associate",
            "--install --report C:\\x.json",
            "--dir C:\\elsewhere",
            "/silent",
            "--log relative.log",
        ] {
            assert!(parse(args(text)).is_err(), "{text}");
        }
        assert_eq!(parse(args("--help")).unwrap().mode, Mode::Help);
    }
}
