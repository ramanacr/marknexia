# Intentional target-only decisions

These decisions are deliberately outside the frozen .NET baseline and must not alter a v1 expected result. Status: **proposed for feasibility review; not yet approved for implementation.** The Task 8 go/no-go decision is the approval authority.

* **Package manifest validation — proposed.** The Rust package manifest may replace the current installer’s `.dll` and `.pri` payload-file checks with equivalent Rust artifact and resource-manifest validation, while retaining archive traversal, required-payload, size-limit, and checksum protections. Rationale: file names are implementation-specific, while those protections are compatibility and security contracts.
* **Settings backup and migration — proposed.** The Rust settings store may use atomic backup and migration semantics. The .NET baseline records the current fail-soft load, normalization, recent-item bounds, and atomic temporary-file replacement behavior; a target migration must preserve those user-visible outcomes. Rationale: recovery mechanics can improve without changing settings outcomes the user observes.
* **Sanitizer differences (SAN-1 to SAN-11) — proposed.** See `sanitizer-ammonia-differences.md`: CSS normalization, stricter URL policy with .NET-style remote-image blanking, inline-only SVG, 128 MiB limits with nesting/work/reparent/attribute budgets that fail closed, chunked parallel cleaning with byte-identical output, a Marknexia-owned linear serializer (needs `--cfg ammonia_unstable`), white-space-only input, the `<pre>`/`<textarea>` leading line feed, and attacker-controlled attributes the bridge must treat as untrusted.
* **Markdown source diagnostics — proposed.** See `markdown-source-diagnostics.md`: an additive, non-serialized diagnostics field; HTML output and fixtures are unchanged.
* **Markdown resource limits — proposed.** See `markdown-resource-limits.md`: container and inline nesting flattened beyond depth 32, and a rendered-body output cap (default 128 MiB) that fails the parse.
* **Rendering differences (REND-1 to REND-7) — proposed.** See `rendering-differences.md`: the .NET size limits (50 MiB source, 128 MiB page) with the sanitizer's budgets, C#-only highlighting, LF page bytes, single-pass remote-image blanking, math depth clamp, text-handling approximations, and API shape (including cooperative cancellation).
* **Navigation probe order and case folding (NAV-1 to NAV-4) — proposed.** See `navigation-probe-order.md`:
  * A relative escape is rejected before the current-file probe.
  * `..` above the drive root is rejected rather than clamped.
  * A drive-root current file is rejected.
  * Containment folds ASCII case only, so non-ASCII case variants are false blocks.

  The file also sets native-adapter requirements: replace the virtual file lookup, and check containment on the final path of the opened handle. The positive navigation fixtures match .NET exactly.
