//! The Rust preview's identity. Every name differs from the .NET edition
//! (`src/Marknexia.Setup/Program.cs`), so both install side by side and
//! neither setup touches the other's files, keys or shortcuts.

/// Shown in Apps and Features, the Start menu and the setup's messages.
pub const PRODUCT_NAME: &str = "Marknexia (Rust preview)";
/// `%LOCALAPPDATA%\Programs\<this>` (.NET: `Marknexia`).
pub const INSTALL_FOLDER_NAME: &str = "Marknexia Rust Preview";
/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\<this>` (.NET: `Marknexia`).
pub const UNINSTALL_KEY_NAME: &str = "MarknexiaRustPreview";
/// The opt-in ProgID (.NET: `Marknexia.MarkdownFile`).
pub const PROG_ID: &str = "Marknexia.RustPreview.MarkdownFile";
/// The Start menu shortcut, directly in the user's Programs folder (.NET:
/// `Programs\Marknexia\Marknexia.lnk`).
pub const SHORTCUT_FILE_NAME: &str = "Marknexia (Rust preview).lnk";
/// The shell executable (.NET: `Marknexia.App.exe`).
pub const SHELL_FILE_NAME: &str = "marknexia-win32.exe";
/// The installed copy of this setup, used for uninstall (.NET: `MarknexiaSetup.exe`).
pub const SETUP_FILE_NAME: &str = "marknexia-rust-preview-setup.exe";
pub const PUBLISHER: &str = "Marknexia";
/// The Default Apps capabilities key (opt-in associations only).
pub const CAPABILITIES_KEY: &str = r"Software\Marknexia\RustPreview\Capabilities";
/// The value name under `HKCU\Software\RegisteredApplications`.
pub const REGISTERED_APPLICATION_NAME: &str = "MarknexiaRustPreview";
/// The extensions the .NET edition registers; the preview only adds itself
/// to their "Open with" lists, and only on request.
pub const MARKDOWN_EXTENSIONS: [&str; 4] = [".md", ".markdown", ".mdown", ".mkdn"];

/// The .NET edition's names, kept to prove that the preview never uses them.
pub mod dotnet {
    pub const INSTALL_FOLDER_NAME: &str = "Marknexia";
    pub const UNINSTALL_KEY_NAME: &str = "Marknexia";
    pub const PROG_ID: &str = "Marknexia.MarkdownFile";
    pub const APPLICATION_KEY: &str = r"Software\Classes\Applications\Marknexia.App.exe";
    pub const START_MENU_FOLDER: &str = "Marknexia";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_differs_from_the_dotnet_edition() {
        assert!(!INSTALL_FOLDER_NAME.eq_ignore_ascii_case(dotnet::INSTALL_FOLDER_NAME));
        assert!(!UNINSTALL_KEY_NAME.eq_ignore_ascii_case(dotnet::UNINSTALL_KEY_NAME));
        assert!(!PROG_ID.eq_ignore_ascii_case(dotnet::PROG_ID));
        assert!(!SHORTCUT_FILE_NAME.eq_ignore_ascii_case(dotnet::START_MENU_FOLDER));
        assert!(!SHELL_FILE_NAME.eq_ignore_ascii_case("Marknexia.App.exe"));
        assert!(!SETUP_FILE_NAME.eq_ignore_ascii_case("MarknexiaSetup.exe"));
    }
}
