# Task 1 report: frozen .NET parity baseline

## Implementation summary

Added a deterministic .NET parity exporter and a v1 fixture corpus that captures the existing parser, heading/anchor extraction, navigation resolution, sanitizer, renderer, settings, and update-release validation behavior. The exporter writes ordinally ordered JSON with UTF-8 (no BOM), LF endings, repository-relative virtual-file paths, and a terminal newline. The committed v1 manifest contains 41 cases.

The renderer produces per-render isolation values (a virtual document host and CSP nonce). `ParityNormalizer` replaces only those documented volatile values plus line endings; all rendered elements, attributes, content, meaningful whitespace, URL shape, and sanitizer results remain unchanged.

The parity test exports twice, hashes sorted relative paths and bytes, and validates every `*.case.json` with JsonSchema.Net 9.4.0 against the v1 JSON Schema. The schema dependency is centrally pinned and test-only.

## Files changed

* `Directory.Packages.props` — centrally pins `JsonSchema.Net` 9.4.0.
* `Marknexia.slnx` — includes `Marknexia.Parity.Tests`.
* `compat/schema/marknexia-parity-v1.schema.json` — fixture-envelope schema.
* `compat/fixtures/v1/**` — 41 generated baseline cases and manifest.
* `compat/README.md` and `compat/decisions/README.md` — fixture rules, normalizer scope, and target-only decisions.
* `tools/Marknexia.ParityExporter/**` — console exporter, case envelope, normalizer, and oracle adapters.
* `tests/Marknexia.Parity.Tests/**` — round-trip repeatability and real JSON Schema validation.

## TDD evidence

### RED

1. After restoring the new test project, the prescribed focused run failed with `CS0234`: `Marknexia.ParityExporter` did not exist. This established the missing-exporter baseline.
2. Once the first implementation existed, the focused test failed because two exports had different directory digests. Comparing outputs showed the difference was the renderer-generated `document-<guid>.marknexia.viewer` host and nonce values. The test therefore proved it could detect non-repeatable artifacts.
3. After deterministic normalization, the test failed on schema validation because an exported case name contained `%`, outside the schema name pattern. The exporter now canonicalizes `%` to `percent-` in a fixture filename while retaining the original source-file value.

### GREEN

`rtk proxy dotnet test tests/Marknexia.Parity.Tests/Marknexia.Parity.Tests.csproj --no-build --no-restore --logger "console;verbosity=minimal"`

Result: `Passed: 1, Failed: 0`. It proves byte-identical independent exports and JsonSchema.Net validation of every case.

## Verification

* `rtk proxy dotnet restore tests/Marknexia.Parity.Tests/Marknexia.Parity.Tests.csproj` — passed after elevation to read the user NuGet configuration.
* `rtk proxy dotnet run --project tools/Marknexia.ParityExporter --no-restore -- --output compat/fixtures/v1 --verify` — passed; regenerated the v1 corpus.
* `rtk dotnet test Marknexia.slnx --configuration Release --no-restore` — passed under elevation (the non-elevated attempt was blocked by the known Windows SDK ACL at `%LOCALAPPDATA%\\Microsoft SDKs`).
* `rtk proxy dotnet test Marknexia.slnx --configuration Release --no-build --no-restore --logger "console;verbosity=minimal"` — passed: 140 tests total (the prior 139-product-test baseline plus the new parity test), 0 failures.
* `rtk git diff --check` — passed with no whitespace errors.

## Self-review

* No Rust crate, workflow, installer, app UI, or product behavior was modified.
* Expected values are obtained through the existing .NET behavioral seams, not reimplemented validators.
* The only normalization is explicitly documented transport volatility. It does not suppress sanitizer, URL, HTML structure, or user-content differences.
* Update/archive entries use the public `UpdateService.ParseLatestReleaseJson` release-validation seam; the existing product suite continues to exercise the deeper staged-download checksum, archive traversal, payload, expansion, and cleanup protections.

## Concerns

* The exporter deliberately records update/archive baseline outcomes through the public release-validation seam rather than duplicating private staging/extraction code. Deeper archive checks remain covered by the existing infrastructure tests; a future public, side-effect-free validation seam would allow those exact archive outcomes to become standalone exported cases without widening product APIs.
* All source revisions in this first frozen corpus identify the behavioral-oracle HEAD `2931037abd86d02fca258d61c9171cf451d482e2`, captured before committing the exporter itself.
