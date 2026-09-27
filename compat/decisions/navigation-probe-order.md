# Navigation probe order, drive-root traversal and case folding

Status: **proposed — requires Task 8 approval**

Scope: `crates/marknexia-navigation` and `marknexia-files` path containment, for local destinations resolved against an absolute (canonical) repository root.

## Parity that holds

The four `navigation/positive-*` fixtures are exported from the real .NET `NavigationResolver` and `PathCanonicalizer`, running against an in-memory file set. Rust matches them exactly:

* **Case:** the target keeps the caller's case, as `Path.GetFullPath` does. Only the separator changes, from `\` to `/`. For example, `C:\REPO\docs\api.md` stays `C:/REPO/docs/api.md`. Containment is still checked on case-folded components (ASCII only; see NAV-4).
* **Relative destinations:** the current file is probed first, then the target, for 2 probes. If the current file is missing, the current path itself is the base directory, as in .NET.
* **Repository-root and absolute destinations:** only the target is probed, for 1 probe.
* **Missing targets:** the diagnostic uses the .NET prefix for its branch (`Target file not found: `, `Repository-relative file not found: `, `Absolute target file not found: `), followed by the Windows spelling of the path.

## Observable differences

| ID | Condition | .NET oracle | Rust target |
| --- | --- | --- | --- |
| NAV-1 | A relative destination whose directory-based target escapes the repository root, with the sandbox enforced | Probes the current file first (1 probe). If the file exists, the base is its directory; if not, the base is the current path itself (`NavigationResolver.cs` line 175). Containment is checked only after that. So when the current file is missing, a destination that escapes from its directory can still resolve against the current path. | Checks the directory-based target before any probe and blocks it: 0 probes. Rust uses the current path as the base only after that check has passed and the probe reports the current file missing. |
| NAV-2 | `..` climbs above the drive root | `Path.GetFullPath` clamps at `C:\`, so the result can land back inside the root and resolve. | Blocks before any probe. The diagnostic depends on the destination form (see the next table). |
| NAV-3 | The current file is a drive root, `C:/` | Uses `C:\` as the base. It is not a file, so the current path is the base. | Blocks before any probe with `Access blocked: Current file is invalid; cannot enforce repository sandbox.` |

NAV-2 diagnostics by destination form:

| Destination form | Example (current file `C:/repo/README.md`) | Rust diagnostic |
| --- | --- | --- |
| Relative | `../../repo/docs/api.md` | `Access blocked: Relative path escapes repository root sandbox.` |
| Repository root | `/../../repo/docs/api.md` | `Access blocked: Repository root traversal outside sandbox boundary.` |
| Absolute or `file:///` | `C:/../repo/docs/api.md` | `Access blocked: Network file paths and invalid local paths are not supported.` The path cannot be canonicalized, so it is not a valid local path. |

None of these conditions is in the v1 corpus. Each one is asserted in `crates/marknexia-navigation/tests/security_properties.rs` (`nav_1_*`, `nav_2_*`, `nav_3_*`). NAV-4 is described in its own section below.

## NAV-4: case folding for containment

Containment compares case-folded components (`CanonicalPath` in `crates/marknexia-files/src/path.rs`). Only ASCII `A`-`Z` fold to `a`-`z`. Every non-ASCII character must match exactly. The fold does not depend on any Unicode or NTFS `$UpCase` table version, so it cannot merge two names that a volume keeps distinct.

| Pair | .NET `IsWithinRoot` / NTFS | Rust |
| --- | --- | --- |
| `key` and `KEY` (ASCII) | Equal | Equal: resolves |
| KELVIN SIGN U+212A and `k`; OHM SIGN U+2126 and `ω`; ANGSTROM SIGN U+212B and `å`; `İ` U+0130 and `i` | Distinct | Distinct: blocked, as in .NET |
| `σ` and `Σ`; `é` and `É`; `д` and `Д` | Equal | Distinct: blocked, a **false block** |
| Georgian Mkhedruli U+10D0-10FF and Mtavruli U+1C90-1CBF (Unicode 11); Cherokee U+13A0-13F5 and U+AB70-ABBF, U+13F8-13FD (Unicode 8); Latin Extended-D U+A7C0 and above (Unicode 12-16); Glagolitic U+2C2F / U+2C5F (Unicode 14) | Equal in current .NET tables. Distinct on volumes whose `$UpCase` table predates the pair | Distinct: blocked, a false block that is never an escape |

Look-alikes and case variants are blocked before any probe. For example:

* with root `C:/key`, the link `C:/` + U+212A + `ey/secret.md` is blocked;
* with root `C:/src/` + U+10D0, both `C:/src/` + U+1C90 + `/secret.md` and `../` + U+1C90 + `/secret.md` are blocked.

A link that spells the non-ASCII part of the root exactly still resolves, as does any ASCII case variant.

The rationale is that merging a pair the volume keeps distinct lets a sibling directory pass containment, and the display target then names that sibling. With ASCII-only folding, the worst case is a document with a non-ASCII case variant in its link that fails to open.

Tests: `crates/marknexia-files/tests/path_properties.rs` covers `contains` and `contains_opened_target` in both directions. `crates/marknexia-navigation/tests/security_properties.rs` covers links in absolute, `file:///`, relative and repository-root form, each blocked with 0 probes.

## Requirements for a native file adapter

These requirements are not implemented yet. They must hold before any production adapter ships.

* **Replace the virtual file lookup.** `VirtualFileSystem` in `crates/marknexia-files/src/virtual_fs.rs` is a test and probe-counting model only. Its lookup folds with full Unicode `to_lowercase` and merges names that NTFS keeps distinct. A production adapter must not reuse it. Gate the type to tests, or replace it with a probe trait whose native implementation asks the file system.
* **Confirm containment on the opened handle.** Lexical containment is necessary but not sufficient, because reparse points, junctions, 8.3 short names and per-directory case sensitivity can all redirect a path. After opening the target, the adapter must:
  1. obtain `GetFinalPathNameByHandle` for the opened target, and for the repository root opened as a directory handle;
  2. compare the two final paths component by component with an ordinal comparison, folding ASCII case only (the NAV-4 fold);
  3. authorize the target only if the root's final path is a prefix of it, through `RepositoryScope::contains_opened_target` with both lexical and final paths.

  Otherwise it must close the handle and block.

## Rationale

The Rust sandbox checks canonical containment before any probe the destination could influence. All four differences fail closed:

* **NAV-1** only delays a probe of the trusted current file. It also refuses a fallback base that .NET reaches only when the current document no longer exists.
* **NAV-2** keeps `CanonicalPath` strict. A path that names a directory above the drive root is a traversal attempt, not a path to normalize.
* **NAV-3** treats a drive root as not being a document, so it cannot anchor a relative link.
* **NAV-4** trades false blocks on non-ASCII case variants for independence from Unicode and `$UpCase` table versions.

## Approval needed

Task 8 must accept NAV-1 to NAV-4, or require exact .NET ordering, clamping, drive-root bases and Unicode case folding. Either way, no v1 fixture changes.
