# Marknexia compatibility fixtures

`fixtures/v1` freezes observable .NET behavior for the Rust feasibility work. Each JSON case conforms to `schema/marknexia-parity-v1.schema.json`, is UTF-8 without BOM, uses LF endings, and has a terminal newline.

The exporter invokes the existing .NET parser, sanitizer, renderer, navigation resolver, settings service, and update-release validation seam. Fixture cases describe the current oracle; a target-only improvement belongs in `decisions/`, not in an expected result.

Run `dotnet run --project tools/Marknexia.ParityExporter -- --output compat/fixtures/v1 --verify` to regenerate and validate the corpus.
