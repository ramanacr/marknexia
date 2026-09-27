# Navigation probe order and drive-root traversal

Status: **proposed — requires Task 8 approval**

Scope: `crates/marknexia-navigation` and `marknexia-files` path containment, for local destinations resolved against an absolute (canonical) repository root.

## Parity that holds

The four `navigation/positive-*` fixtures are exported from the real .NET `NavigationResolver` and `PathCanonicalizer`, running against an in-memory file set. Rust matches them exactly:

* **Case:** the target keeps the caller's case, as `Path.GetFullPath` does. Only the separator changes, from `\` to `/`. For example, `C:\REPO\docs\api.md` stays `C:/REPO/docs/api.md`. Containment is still checked on case-folded components (see "Case folding for containment").
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

None of these conditions is in the v1 corpus. Each one is asserted in `crates/marknexia-navigation/tests/security_properties.rs` (`nav_1_*`, `nav_2_*`, `nav_3_*`).

## Case folding for containment

This section describes no observable difference. It records how containment folds case.

Containment compares case-folded components. The fold (`fold_char` in `crates/marknexia-files/src/path.rs`) merges only simple 1:1 case pairs in the Basic Multilingual Plane. Look-alikes that NTFS and .NET `OrdinalIgnoreCase` keep distinct also stay distinct in Rust:

* KELVIN SIGN U+212A and `k`
* OHM SIGN U+2126 and `ω`
* ANGSTROM SIGN U+212B and `å`
* `İ` and `i`

For example, with root `C:/key`, both `C:/` + U+212A + `ey/secret.md` and `../` + U+212A + `ey/secret.md` are rejected before any probe, as .NET `IsWithinRoot` rejects them.

Rust keeps some characters distinct that .NET folds together, which can only fail closed:

* final sigma `ς` and `σ`
* dotless `ı` and `I`
* characters outside the Basic Multilingual Plane

One residual risk is shared with .NET. A case pair added in a newer Unicode version than a volume's NTFS upcase table is merged by the fold but not by that volume.

The tests are in `crates/marknexia-files/tests/path_properties.rs` and `crates/marknexia-navigation/tests/security_properties.rs`.

## Rationale

The Rust sandbox checks canonical containment before any probe the destination could influence. All three differences fail closed:

* **NAV-1** only delays a probe of the trusted current file. It also refuses a fallback base that .NET reaches only when the current document no longer exists.
* **NAV-2** keeps `CanonicalPath` strict. A path that names a directory above the drive root is a traversal attempt, not a path to normalize.
* **NAV-3** treats a drive root as not being a document, so it cannot anchor a relative link.

## Approval needed

Task 8 must accept NAV-1 to NAV-3, or require exact .NET ordering, clamping and drive-root bases. Either way, no v1 fixture changes.
