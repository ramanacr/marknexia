# Intentional target-only decisions

These decisions are deliberately outside the frozen .NET baseline and must not alter a v1 expected result. Status: **proposed for feasibility review; not yet approved for implementation.** The Task 8 go/no-go decision is the approval authority.

* **Package manifest validation — proposed.** The Rust package manifest may replace the current installer’s `.dll` and `.pri` payload-file checks with equivalent Rust artifact and resource-manifest validation, while retaining archive traversal, required-payload, size-limit, and checksum protections. Rationale: file names are implementation-specific, while those protections are compatibility and security contracts.
* **Settings backup and migration — proposed.** The Rust settings store may use atomic backup and migration semantics. The .NET baseline records the current fail-soft load, normalization, recent-item bounds, and atomic temporary-file replacement behavior; a target migration must preserve those user-visible outcomes. Rationale: recovery mechanics can improve without changing settings outcomes the user observes.
