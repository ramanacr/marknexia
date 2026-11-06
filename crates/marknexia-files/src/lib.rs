#![forbid(unsafe_code)]

//! Portable file-system compatibility surface.
pub mod atomic_write;
pub mod bounded_read;
pub mod path;
pub mod repository;
pub mod settings_store;
pub mod startup_probe;
pub mod virtual_fs;
