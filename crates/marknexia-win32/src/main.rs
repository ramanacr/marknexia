//! The Rust shell is a GUI program: no console window opens with it
//! (release and debug). Nothing reads its stdout or stderr; harnesses use
//! named events and UI Automation.
#![windows_subsystem = "windows"]
#![forbid(unsafe_code)]

fn main() {
    if let Err(error) = marknexia_win32::window::run() {
        eprintln!("Marknexia Rust shell failed: {error:?}");
        std::process::exit(1);
    }
}
