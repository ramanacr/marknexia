# Shell chrome: toolbar, menu, sidebar, diagnostics bar

Status: **proposed — requires Task 8 approval**

Scope: `crates/marknexia-win32/src/chrome_model.rs` and `crates/marknexia-win32/src/window/{chrome,sidebar,toolbar,diagnostics,dialogs,settings_bridge}.rs` (Phase 2 tasks 1, 2 and 6). The .NET sources are `src/Marknexia.App/MainWindow.xaml`, `MainWindow.xaml.cs` and `MainWindow.Support.cs`.

## Parity that holds

* **Accelerators** (`RegisterKeyboardAccelerators`): Ctrl+O and Ctrl+T open a file, Ctrl+Shift+O opens a repository, F5 and Ctrl+R reload, Alt+Left and Alt+Right go back and forward, Ctrl+W closes the tab, Ctrl+Tab and Ctrl+Shift+Tab change tabs. They work from the shell controls and from inside the document (the WebView2 accelerator hand-off). Ctrl+F and Esc belong to the find-in-page lane.
* **Settings** (`SettingsService`): the theme, `IsSidebarOpen`, `SidebarWidth` (220–520 DIPs), `SidebarMode` (0 Outline, 1 Repository), `AllowRemoteAssets`, `RepositoryRoot`, recent files and recent folders are loaded at startup and saved after each change, at the same moments as .NET. Opening a repository shows the pane without saving `IsSidebarOpen`, as .NET does.
* **Sidebar**: the outline lists the rendered document's headings, indented 14 DIPs per level; a click or Enter jumps to the heading's anchor through `window.marknexiaBridge.scrollToAnchor` and records history only when the location changes. The repository tree uses `marknexia_files::repository::build_repository_tree` on a worker thread with a cancel flag; a newer scan cancels the older one, the root is expanded, and a click on a file opens it. Without a startup document, the saved root is restored; a missing root is cleared and saved. The first opened document without a repository sets the root from `detect_repository_root` and saves it. Opening a repository adds a recent folder, selects the Repository view, and opens `README.md` when it exists.
* **Reload, theme, remote assets**: reload re-reads and re-renders the active document and, after the load, scrolls back to its location; a failed reload keeps the previous document and shows "Unable to reload document: …". A theme change and the remote-assets toggle save the setting and reload the active document; the remote-assets toggle shows the .NET messages.
* **Diagnostics bar** (`ShowDiagnostic`): one message, Info, Warning or Error, above the document, replaced by a newer message, closed by its Close button. The .NET texts are kept: "File not found: …", "Folder not found: …", "Unable to open document: …", "The repository tree could not be loaded.", the remote-assets messages.

## Observable differences

| ID | Condition | .NET oracle | Rust target |
| --- | --- | --- | --- |
| CHROME-1 | Recent files | A "RECENT FILES" list (up to five existing files) on the start page, shown while no document is open. | The same list as a third sidebar view, "Recent files", with the selector tabs "Outline", "Repository", "Recent files". The third view is not saved: `SidebarMode` stays 0 or 1 (SET-7). |
| CHROME-2 | Toolbar and menus | Brand icon and tagline, a Find button, tooltips, a compact layout below 720 px (pane as overlay), and Help items "Troubleshooting & diagnostics", "Software components (SBOM)" and "Check for updates". | Not present yet. Find belongs to the find-in-page lane; the Help items wait for Phases 6 and 7; the compact layout and tooltips are not ported. The menu bar is the native Win32 menu, which stays light in the dark theme. The open button keeps the existing name "Open Markdown" (.NET: "Open Markdown file"). |
| CHROME-3 | Row buttons | Each outline row has "Copy heading anchor"; each repository row has "Reveal in File Explorer". | Not present. |
| CHROME-4 | Back and Forward | `ReplaceActiveDocumentAsync`: re-reads and re-renders the file, then scrolls to the entry's anchor. | History entries come from outline jumps in the same document, so the entry is shown by scrolling the loaded document; the file is not re-read. Link navigation, which can change the document, is not wired yet. |
| CHROME-5 | About and Help | `ContentDialog` with the logo and a Close button. | A system message box with an OK button and the same text. The help text keeps the .NET lines for find (Ctrl+F, Enter, Esc). |
| CHROME-6 | `ShowDiagnostic` side effects | Appends `[time] [Diagnostic:severity] message` to `%LOCALAPPDATA%\Marknexia\crash.log`. | Nothing is written. The bar's accessible name starts with the severity ("Warning: …"). Rust adds messages for settings load notices (SET-1, SET-1a, SET-2), failed settings saves, a failed folder dialog, and a repository tree shortened by scan limits (RS-1), which also shows the heading "Repository (incomplete)". |
| CHROME-7 | Remote images enabled | Remote images load. | The setting is saved, the sanitizer keeps `https` images (`RemoteImagePolicy::AllowHttps`) and the page policy allows `http:` and `https:` images, but the WebView2 resource broker still answers every network request with a denial, so the images show the "Image unavailable" fallback. Allowing them needs a broker decision. |
| CHROME-8 | Sidebar resize | Pointer drag on the 6-DIP handle. | The same handle, plus keyboard: focus it with Tab, then Left and Right (16 DIPs), Home (narrowest), End (widest). Each key saves the width. |
| CHROME-9 | Opening an open file | Every open path selects the existing tab. | The recent list, the tree and a repository README select the existing tab. The Open dialog and the command line keep the earlier shell behaviour: a new tab, with the multi-select `*.md;*.markdown;*.txt` dialog (.NET: one file, `.md .markdown .mdown .mkdn`). |
| CHROME-10 | Settings location | Always `%LOCALAPPDATA%\Marknexia\settings.json`. | `MARKNEXIA_SETTINGS_PATH`, when it is an absolute path, replaces it. Only the native tests and `Collect-RustDesktopPerf.ps1` set it. |

## Approval needed

Approve or reject each row. CHROME-2, CHROME-3, CHROME-5 and CHROME-7 are gaps to close before promotion unless approved as permanent.
