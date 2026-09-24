#![forbid(unsafe_code)]

fn main() {
    if let Err(error) = marknexia_win32::window::run() {
        eprintln!("Marknexia Rust shell failed: {error:?}");
        std::process::exit(1);
    }
}
