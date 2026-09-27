#![deny(unsafe_code)]

//! Native WebView2 boundary.

pub mod broker;
#[cfg(windows)]
#[allow(unsafe_code)] // WebView2 event registration and response streams stay private.
mod callbacks;
#[cfg(windows)]
#[allow(unsafe_code)] // Only the native COM adapter may cross the FFI boundary.
pub mod environment;
#[cfg(windows)]
#[allow(unsafe_code)] // Controller ownership and calls stay in the native adapter.
pub mod host;
pub mod policy;
pub mod probe;
pub mod protocol;
pub mod recovery;
#[cfg(windows)]
pub mod session;
#[cfg(windows)]
#[allow(unsafe_code)] // A read-only COM stream over a shared document buffer.
mod stream;
