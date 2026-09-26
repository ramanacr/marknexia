# Navigation probe order and drive-root traversal

Status: **proposed — requires Task 8 approval**

Scope: `crates/marknexia-navigation`, local destinations resolved against an absolute (canonical) repository root.

## Parity that holds

The four `navigation/positive-*` fixtures are exported from the real .NET `NavigationResolver` and `PathCanonicalizer`, running against an in-memory file set. Rust matches them exactly:

* The target keeps the caller's case, as `Path.GetFullPath` does. Only the separator changes, from `\` to `/`. For example, `C:\REPO\docs\api.md` stays `C:/REPO/docs/api.md`. Containment is still checked on the case-folded canonical path.
* A relative destination probes the current file first, then the target: 2 probes. If the current file is missing, the current path itself is the base directory, as in .NET.
* Repository-root and absolute destinations probe only the target: 1 probe.
* A missing target's diagnostic uses the .NET prefix for its branch (`Target file not found: `, `Repository-relative file not found: `, `Absolute target file not found: `), followed by the Windows spelling of the path.

## Observable differences

| ID | Condition | .NET oracle | Rust target |
| --- | --- | --- | --- |
| NAV-1 | A relative destination whose directory-based target escapes the repository root, with the sandbox enforced | Probes the current file, then blocks: 1 probe. If the current file is missing, it retries with the current path as the base, and that target may be allowed. | Blocks before any probe: 0 probes. There is no retry from the current path. |
| NAV-2 | `..` climbs above the drive root, for example `../../repo/docs/api.md` from `C:/repo/README.md`, or `/../../repo/docs/api.md` | `Path.GetFullPath` clamps at `C:\`, so the result can land back inside the root and resolve. | Blocks before any probe with the branch's traversal diagnostic. |

Neither condition is in the v1 corpus. Both are asserted in `crates/marknexia-navigation/tests/security_properties.rs`: `nav_1_*` and `nav_2_*`.

## Rationale

The Rust sandbox checks canonical containment before any probe that the destination could influence. NAV-1 only moves a probe of the trusted current file later, and refuses a fallback base that .NET reaches only when the current document no longer exists. NAV-2 keeps `CanonicalPath` strict: a path that names a directory above the drive root is a traversal attempt, not a path to normalize. Both differences fail closed.

## Approval needed

Task 8 must accept both differences, or require exact .NET ordering and clamping. Either way, no v1 fixture changes.
