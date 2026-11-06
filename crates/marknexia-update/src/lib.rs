#![forbid(unsafe_code)]

//! The portable updater (`update-manifest-v1`, production plan Phase 6).
//!
//! Headless: networking, Authenticode, owner-only folders and process
//! start are behind traits ([`stage::Fetcher`], [`stage::StagePlatform`],
//! [`activation::Launcher`]); the shell provides the native adapters.
//! Store/MSIX installations never use this crate: the shell disables
//! "Check for updates" for a packaged process.

pub mod activation;
pub mod archive;
pub mod check;
pub mod config;
pub mod hex;
pub mod keys;
pub mod layout;
pub mod manifest;
pub mod payload;
pub mod stage;
pub mod version;
