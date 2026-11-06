# Settings, recent items, startup resolution and search state

Status: **proposed — requires Task 8 approval**

Scope: `crates/marknexia-core` (`settings`, `recent`, `startup`, `search_state`, `text`) and `crates/marknexia-files` (`settings_store`, `atomic_write`, `startup_probe`). The .NET sources are `src/Marknexia.Infrastructure/SettingsService.cs`, `src/Marknexia.Core/StartupFileResolver.cs`, `src/Marknexia.Core/DocumentSearchState.cs` and `MainWindow.RefreshRecentFiles`.

## Parity that holds

* **Fixtures:** both `compat/fixtures/v1/settings` cases (`malformed-settings`, `recent-item-bounds`) match exactly, in memory and through the file store.
* **Format read:** `%LOCALAPPDATA%\Marknexia\settings.json`, read with the `System.Text.Json` defaults: PascalCase names matched case-sensitively, `Theme` as an integer, unknown properties ignored, duplicate properties last-wins, no comments or trailing commas, depth 64, and BOM-selected text encoding with U+FFFD replacement (`File.ReadAllText`).
* **Defaults and normalization:** the same defaults; undefined `Theme` becomes `System`; `RepositoryRoot` trimmed or `null`; `SidebarWidth` clamped to 220–520; recent files capped at 15 and folders at 10, trimmed, de-duplicated ignoring case, newest first.
* **Recent additions:** the literal .NET order of operations (remove the raw path ignoring case, insert it first, truncate, normalize). A white-space path changes nothing and is not saved. Missing files stay in the list; the sidebar shows the first five that exist.
* **Startup resolution:** the same candidate order, the direct-candidate-first rule, the Windows argv splitting rules, the four extensions, `GetFullPath` (`GetFullPathNameW`) and "is a file" checks.
* **Search state:** the same request-id rules, wrap-around and non-overlapping case-insensitive counting.

## Observable differences

| ID | Condition | .NET oracle | Rust target |
| --- | --- | --- | --- |
| SET-1 | The settings file cannot be used: invalid JSON, a wrong-typed known property, a non-object document (including `null`), or nesting deeper than 64 | Loads defaults silently. The next save overwrites the file, so the old content is lost. | Renames the file to `settings.json.corrupt-<id>`, loads defaults, and returns `SettingsNotice::Corrupt` with the preserved path. If the rename fails, it returns `CorruptNotPreserved` and the store is read-only. |
| SET-1a | The file exists but cannot be read (for example, access denied, or the path is a directory) | Loads defaults; the next save tries to overwrite it. | Loads defaults, returns `SettingsNotice::Unreadable`, and the store is read-only until `reload` succeeds. |
| SET-2 | Schema versioning | No version property. | Writes `"SchemaVersion": 2` first, then the .NET properties unchanged. A file without the property is version 1. Before the first write over a version-1 file, the store copies it to `settings.json.v1.bak` (atomically). Loading alone writes nothing. A version above 2 is read leniently (a wrong-typed property keeps its default), returns `SettingsNotice::NewerSchema`, and the store is read-only. An invalid `SchemaVersion` (not an integer of at least 1) is SET-1. |
| SET-3 | A stored recent path or `RepositoryRoot` is not an absolute drive path (`C:\` or `C:/`) or UNC path (`\\server\share`), has a control character, uses a `\\?\` or `\\.\` prefix, or exceeds 32,767 UTF-16 units | Kept. A bad root is cleared later, when the window fails to canonicalize it. | Dropped during normalization. The lexical check proves nothing about existence; the platform path gate still applies. |
| SET-4 | The file is larger than 8 MiB | Read in full. | Treated as SET-1. The largest valid file (25 paths of 32,767 UTF-16 units, JSON-escaped) is below this limit. |
| SET-5 | A save fails | Swallowed; the in-memory settings change anyway. | The in-memory settings change anyway, and the error (`ReadOnly`, `Backup` or `Write`) is returned to the caller. The temporary file is flushed (`sync_all`) before the rename. |
| SET-6 | Case-insensitive comparison of recent paths, file extensions and search text | `OrdinalIgnoreCase`: simple uppercase per UTF-16 unit. | Full uppercase per scalar when it is one character, else the character itself. The results are the same except for characters whose simple and full mappings differ, for example U+1F80–U+1FFC (Greek with iota subscript). |
| SET-7 | `SidebarMode` outside 0–1, or `SidebarWidth` NaN set through the API | Mode stored as is (the window clamps it when it reads it). NaN stays NaN and serializes to an unreadable file. | Mode clamped to 0–1 when normalized. NaN becomes 280. The window outcome for the mode is unchanged. |
| SET-8 | A `file:` URI candidate at startup | `Uri.LocalPath` for any absolute file URI. | Supports `file:///C:/…`, `file://C:/…`, `file://localhost/C:/…`, the legacy `C|` drive form, and `file://server/share/…` (UNC). The fragment is removed and the path percent-decoded as UTF-8. Other forms, and invalid UTF-8 after decoding, are not a match. |
| SET-8a | A plain Windows path candidate that contains `%xx` | `Uri.TryCreate` treats it as an implicit file URI, and `LocalPath` may percent-decode it. Not verified. | The path is used literally. |
| SET-8b | A candidate that is a dangling symbolic link to a `.md` file | `File.Exists` can report it as existing; opening it fails later. | `Path::is_file` follows the link, so it is not a match. |
| SET-9 | API shape of the search state | `int` counts; an inconsistent rendered position throws `ArgumentOutOfRangeException`. | `usize` counts (a negative count cannot be expressed); an inconsistent position returns `SearchPositionError` and leaves the state unchanged. |

## Rollback

The Rust edition writes the .NET property names and value types, so the .NET edition reads a schema-2 file and ignores `SchemaVersion`. When the .NET edition saves, it drops `SchemaVersion`. The Rust edition then reads that file as version 1 again and refreshes `settings.json.v1.bak` before its next write. Neither edition coordinates writes between running processes; the last save wins, as with two .NET instances today.

## Open item for a later phase

A packaged (MSIX) .NET install writes `settings.json` through file-system virtualization to the package's `LocalCache\Local\Marknexia` folder. `default_settings_path` returns only the unvirtualized `%LOCALAPPDATA%` location. Import from the packaged location belongs to the packaging and side-by-side work (design §16).
