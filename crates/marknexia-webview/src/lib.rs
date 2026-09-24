#![deny(unsafe_code)]

//! Native WebView2 boundary.

pub mod broker;
#[cfg(windows)]
#[allow(unsafe_code)] // Only the native COM adapter may cross the FFI boundary.
pub mod environment;
#[cfg(windows)]
#[allow(unsafe_code)] // Controller ownership and calls stay in the native adapter.
pub mod host;
pub mod protocol;
pub mod recovery;
