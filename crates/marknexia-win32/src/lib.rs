#![deny(unsafe_code)]

pub mod accessibility;
pub mod app;
pub mod keyboard;
pub mod layout;
pub mod tabs;
pub mod theme;
#[allow(unsafe_code)] // Raw HWND ownership and message dispatch stay in this adapter.
pub mod window;
