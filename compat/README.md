# Marknexia compatibility fixtures

`fixtures/v1` freezes observable .NET behavior for the Rust feasibility work. Each JSON case conforms to `schema/marknexia-parity-v1.schema.json`, is UTF-8 without BOM, uses LF endings, and has a terminal newline.

The exporter invokes the existing .NET parser, sanitizer, renderer, navigation resolver, settings service, and update-release validation seam. Fixture cases describe the current oracle; a target-only improvement belongs in `decisions/`, not in an expected result.

Run `dotnet run --project tools/Marknexia.ParityExporter -- --output compat/fixtures/v1 --verify` to validate the corpus without modifying it. Omit `--verify` only when deliberately regenerating a reviewed baseline: the exporter first builds in a temporary sibling directory and only replaces an owned fixture corpus after successful export.

`--verify` validates every case against the v1 JSON Schema, requires a nonempty manifest whose paths exactly match the case files and all required behavior areas, then byte-compares a temporary replay with the committed corpus. Fixtures declare one resolved Git revision. The replay provenance allowlist is `src/`, `test-fixtures/`, `Directory.Packages.props`, `Directory.Build.props`, `Directory.Build.targets`, `global.json`, `NuGet.config`, and `packages.lock.json`; tracked or untracked changes in those inputs reject a replay rather than relabeling changed oracle behavior.
