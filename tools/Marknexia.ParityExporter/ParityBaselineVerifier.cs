using System.Text;
using System.Text.Json;
using Json.Schema;

namespace Marknexia.ParityExporter;

public static class ParityBaselineVerifier
{
    public static async Task VerifyAsync(string repositoryRoot, string baselineRoot, CancellationToken cancellationToken)
    {
        string fullBaseline = Path.GetFullPath(baselineRoot);
        ParityFixtureValidator.Validate(fullBaseline, repositoryRoot);
        string revision = GetFrozenRevision(fullBaseline);
        string temporary = Path.Combine(Path.GetTempPath(), "marknexia-parity-verify-" + Guid.NewGuid().ToString("N"));
        try
        {
            await new ParityExporter(repositoryRoot, revision).ExportAsync(temporary, cancellationToken);
            ParityFixtureValidator.Validate(temporary, repositoryRoot);
            if (!DirectoryDigest(fullBaseline).Equals(DirectoryDigest(temporary), StringComparison.Ordinal))
                throw new InvalidDataException("The committed parity baseline differs from a fresh oracle export.");
        }
        finally
        {
            try { if (Directory.Exists(temporary)) Directory.Delete(temporary, recursive: true); }
            catch { }
        }
    }

    public static string GetFrozenRevision(string fixtureRoot)
    {
        string[] revisions = Directory.EnumerateFiles(fixtureRoot, "*.case.json", SearchOption.AllDirectories)
            .Select(path => JsonDocument.Parse(File.ReadAllText(path)))
            .Select(document =>
            {
                using (document)
                {
                    return document.RootElement.GetProperty("sourceRevision").GetString();
                }
            })
            .Where(revision => !string.IsNullOrWhiteSpace(revision))
            .Select(revision => revision!)
            .Distinct(StringComparer.Ordinal)
            .ToArray();
        return revisions.Length == 1 && !string.IsNullOrWhiteSpace(revisions[0])
            ? revisions[0]!
            : throw new InvalidDataException("Parity fixtures must declare one non-empty frozen source revision.");
    }

    internal static string DirectoryDigest(string directory)
    {
        using var hash = System.Security.Cryptography.IncrementalHash.CreateHash(System.Security.Cryptography.HashAlgorithmName.SHA256);
        foreach (string file in Directory.EnumerateFiles(directory, "*", SearchOption.AllDirectories)
                     .OrderBy(path => Path.GetRelativePath(directory, path), StringComparer.Ordinal))
        {
            hash.AppendData(Encoding.UTF8.GetBytes(Path.GetRelativePath(directory, file).Replace('\\', '/') + "\n"));
            hash.AppendData(File.ReadAllBytes(file));
        }
        return Convert.ToHexString(hash.GetHashAndReset());
    }
}

public static class ParityFixtureValidator
{
    private static readonly object SchemaLock = new();
    private static JsonSchema? _v1Schema;

    public static void Validate(string fixtureRoot, string repositoryRoot)
    {
        string manifestPath = Path.Combine(fixtureRoot, "manifest.json");
        if (!File.Exists(manifestPath)) throw new InvalidDataException("Parity fixture manifest is missing.");
        using JsonDocument manifest = JsonDocument.Parse(File.ReadAllText(manifestPath));
        JsonElement root = manifest.RootElement;
        if (root.ValueKind != JsonValueKind.Object
            || !root.TryGetProperty("schemaVersion", out JsonElement schemaVersion)
            || schemaVersion.GetString() != ParityCase.SchemaVersion)
            throw new InvalidDataException("Parity fixture manifest has an unsupported schema version.");
        if (!root.TryGetProperty("cases", out JsonElement declaredCases) || declaredCases.ValueKind != JsonValueKind.Array
            || !root.TryGetProperty("caseCount", out JsonElement declaredCount) || !declaredCount.TryGetInt32(out int count))
            throw new InvalidDataException("Parity fixture manifest is missing required fields.");
        string[] declared = declaredCases.EnumerateArray().Select(item => item.GetString() ?? string.Empty).Order(StringComparer.Ordinal).ToArray();
        string[] actual = Directory.EnumerateFiles(fixtureRoot, "*.case.json", SearchOption.AllDirectories)
            .Select(path => Path.GetRelativePath(fixtureRoot, path).Replace('\\', '/')).Order(StringComparer.Ordinal).ToArray();
        if (count == 0 || count != actual.Length || !declared.SequenceEqual(actual, StringComparer.Ordinal))
            throw new InvalidDataException("Parity fixture manifest does not exactly describe the generated case files.");
        foreach (string area in new[] { "markdown", "headings", "navigation", "sanitizer", "rendering", "settings", "update-archives" })
            if (!actual.Any(path => path.StartsWith(area + "/", StringComparison.Ordinal)))
                throw new InvalidDataException($"Parity fixture corpus is missing the required '{area}' area.");
        JsonSchema schema = GetV1Schema(repositoryRoot);
        foreach (string file in Directory.EnumerateFiles(fixtureRoot, "*.case.json", SearchOption.AllDirectories))
        {
            using JsonDocument instance = JsonDocument.Parse(File.ReadAllText(file));
            if (!schema.Evaluate(instance.RootElement).IsValid)
                throw new InvalidDataException($"Parity fixture does not validate against the v1 schema: {file}");
        }
    }

    private static JsonSchema GetV1Schema(string repositoryRoot)
    {
        lock (SchemaLock)
        {
            return _v1Schema ??= JsonSchema.FromText(File.ReadAllText(Path.Combine(repositoryRoot, "compat", "schema", "marknexia-parity-v1.schema.json")));
        }
    }
}
