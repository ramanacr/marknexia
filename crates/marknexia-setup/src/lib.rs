#![forbid(unsafe_code)]

//! The per-user setup of Marknexia (Rust preview) (production plan
//! Phase 7, `compat/decisions/rust-installers.md`).
//!
//! Headless: the registry and the shell are behind
//! [`registry::RegistryBackend`] and [`install::ShellIntegration`]; the
//! executable (`src/main.rs`) provides the native `HKCU`, shortcut and
//! message-box adapters. Tests use [`registry::MemoryRegistry`], temp
//! folders and text payloads only: no test writes the real registry or an
//! executable.

pub mod cli;
pub mod container;
pub mod identity;
pub mod install;
pub mod layout;
pub mod payload;
pub mod registry;
pub mod settings_probe;
pub mod update_state;

#[cfg(test)]
mod install_tests;
#[cfg(test)]
pub(crate) mod test_support;
