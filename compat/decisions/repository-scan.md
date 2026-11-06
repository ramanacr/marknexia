# Repository scan and tree differences

Status: **proposed — requires Task 8 approval**

Scope: `crates/marknexia-files/src/repository.rs` and `GatedPath` in `crates/marknexia-files/src/path.rs`. The .NET baseline is `src/Marknexia.Files/RepositoryService.cs` and `src/Marknexia.Files/RepositoryTreeBuilder.cs`.

## What is the same as .NET

The tests in `crates/marknexia-files/tests/repository_scan.rs` check these rules on temporary folders:

* The extension sets: `.md`, `.markdown`, `.mdown`, `.mkdn`, `.png`, `.jpg`, `.jpeg`, `.gif`, `.svg`, `.webp`, `.ico`, `.bmp`, `.pdf`. The match ignores case. The extension is found like `Path.GetExtension`, so a file named `.md` is listed and `a.md.` is not.
* The ignored folders. The tree ignores 13 names, and workspace enumeration ignores 7 names. Both lists are the .NET lists, and the match ignores case.
* Hidden entries. The tree always skips them. Workspace enumeration skips them unless `show_hidden` is set. A dot folder such as `.github` is not hidden on Windows.
* Reparse points. Junctions, symbolic links, mount points and other reparse points are never listed and never entered, as in .NET. A junction that leads out of the root or back into the root is skipped. A root that is itself a junction is scanned, as in .NET.
* The tree order. Folders come first. Then names sort by `StringComparer.OrdinalIgnoreCase` (uppercase forms, so `_` sorts after letters). The sort is stable, so two names that are equal when case is ignored keep the listing order.
* Empty folders. A folder without supported files is left out of the tree. The root is always present.
* A missing root, or a root that is a file, gives an empty root node.
* An access-denied folder gives an empty listing. Any other read error fails the whole scan. In .NET this error is an `IOException`, and the window shows "The repository tree could not be loaded."
* Root detection checks `.git` (a folder or a file), `*.sln` and `*.slnx`, from the file's folder upwards. `create_context` sets the repository kind from `.git`.
* UNC share roots (`\\server\share\...` and `\\?\UNC\server\share\...`) are allowed (owner decision).

## Observable differences

| ID | Case | .NET | Rust |
| --- | --- | --- | --- |
| RS-1 | Size of the scan | No limits. The recursive walk can overflow the stack on a very deep tree, which ends the process. | `ScanLimits` sets the limits. `max_depth` is 64 folder levels by default, with a hard ceiling of 256. `max_files` is 100,000 files. `max_entries` is 1,000,000 directory entries read. The walk is iterative. When a limit is reached, the scan returns the partial tree, and `LimitsHit` shows which limit was reached. The shell must tell the user that the tree is incomplete. |
| RS-2 | Root forms | `CanonicalizePath` accepts relative paths (resolved against the current folder), `file://` URIs and device paths. An unusable root gives an empty node. | The root must pass `GatedPath`: an absolute drive path or a UNC share path. The gate refuses `.` and `..` components, `\\.\` and other `\\?\` device forms, the `pipe`, `mailslot`, `IPC$` and `DavWWWRoot` shares, and reserved names. `build_repository_tree` and `enumerate_workspace_files` return `ScanError::InvalidRoot`, and `detect_repository_root` and `create_context` return `None`. The shell converts `file://` arguments before it calls the scan. |
| RS-3 | Entry names that the document gate refuses | Listed. | Skipped. These are reserved device names (`CON.md`, `nul.png`), names that end with `.` or a space, and names that are not valid UTF-16 (an unpaired surrogate). Such names exist only when another program creates them with `\\?\` paths, and the Rust document open refuses them anyway. |
| RS-4 | Non-ASCII case folding for sort and match | The .NET ordinal casing table. | Simple uppercase mapping from the Rust standard library's Unicode tables. A character with a multi-character uppercase form (`ß`) stays as it is. U+0131 (dotless i) stays as it is, as in .NET. ASCII names give the same result. Names with case pairs that are in one Unicode version and not in the other can sort differently. |
| RS-5 | Access denied during root detection | `Directory.EnumerateFiles` throws `UnauthorizedAccessException`, and `DetectRepositoryRoot` fails. | That folder has no solution marker, and the search continues upwards. The `*.sln` check reads at most 1,000,000 entries in each folder. |
| RS-6 | Access denied part-way through a workspace folder | The files added before the exception stay in the result. | The folder contributes no entries. Windows reports access denied when the listing starts, so this case is not expected in practice. |

Cancellation is not a difference. .NET throws `OperationCanceledException`, and the Rust scan returns `ScanError::Cancelled`. In both, the partial tree is dropped (design spec §10, "Cancelled scan: Discard partial results"). Rust checks the `cancel` callback before each directory entry and before each folder.

## Residual risk (same as .NET)

The reparse-point check uses the attributes from the directory listing. Then the scan opens the subfolder by path. If another process replaces a listed folder with a junction between these two steps, Windows follows the junction. .NET has the same race. The process that does this needs write access inside the repository, and the scan returns only names and paths. When a listed document is opened, `read_source` checks the final path of the opened handle with the same gate. That check does not check repository containment. To close the race, a later native adapter must list each folder through a handle opened with `FILE_FLAG_OPEN_REPARSE_POINT`, for example with `GetFileInformationByHandleEx`. This needs `unsafe` code, so it belongs in `marknexia-win32`, not in `marknexia-files`.

## Unverified assumption

The tree root's display name for a UNC share root (`\\server\share`) is the whole root text in Rust. This copies .NET `DirectoryInfo.Name` for a root. A .NET run has not checked this value.
