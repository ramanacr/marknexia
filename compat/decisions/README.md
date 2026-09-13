# Intentional target-only decisions

These decisions are deliberately outside the frozen .NET baseline and must not alter a v1 expected result.

* The Rust package manifest may replace the current installer’s `.dll` and `.pri` payload-file checks with equivalent Rust artifact and resource-manifest validation, while retaining archive traversal, required-payload, size-limit, and checksum protections.
* The Rust settings store may use atomic backup and migration semantics. The .NET baseline records the current fail-soft load, normalization, recent-item bounds, and atomic temporary-file replacement behavior; a target migration must preserve those user-visible outcomes.
