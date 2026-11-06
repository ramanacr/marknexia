#![deny(unsafe_code)]

pub mod accessibility;
pub mod app;
pub mod chrome_model;
pub mod documents;
pub mod dragdrop;
pub mod find;
pub mod keyboard;
pub mod layout;
pub mod single_instance;
pub mod tabs;
pub mod theme;
#[allow(unsafe_code)] // Raw HWND ownership and message dispatch stay in this adapter.
pub mod window;
