using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Json.Schema;

namespace Marknexia.Parity.Tests;

internal static class ParityTestSupport
{
    public static string CreateTempDirectory()
    {
        string path = Path.Combine(Path.GetTempPath(), "marknexia-parity-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(path);
        return path;
    }

    public static void DeleteTempDirectory(string path)
    {
        if (Directory.Exists(path)) Directory.Delete(path, recursive: true);
    }

    public static string FindRepositoryRoot()
    {
        for (DirectoryInfo? current = new(AppContext.BaseDirectory); current is not null; current = current.Parent)
        {
            if (File.Exists(Path.Combine(current.FullName, "Marknexia.slnx"))) return current.FullName;
        }

        throw new DirectoryNotFoundException("Could not find the Marknexia repository root.");
    }

    public static string DirectoryDigest(string directory)
    {
        using IncrementalHash hash = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        foreach (string file in Directory.EnumerateFiles(directory, "*", SearchOption.AllDirectories)
                     .OrderBy(path => Path.GetRelativePath(directory, path), StringComparer.Ordinal))
        {
            string relativePath = Path.GetRelativePath(directory, file).Replace('\\', '/');
            hash.AppendData(Encoding.UTF8.GetBytes(relativePath + "\n"));
            hash.AppendData(File.ReadAllBytes(file));
        }

        return Convert.ToHexString(hash.GetHashAndReset());
    }

    public static bool ValidateEveryCaseAgainstSchema(string fixtureRoot)
    {
        string schemaPath = Path.Combine(FindRepositoryRoot(), "compat", "schema", "marknexia-parity-v1.schema.json");
        JsonSchema schema = JsonSchema.FromText(File.ReadAllText(schemaPath));

        return Directory.EnumerateFiles(fixtureRoot, "*.case.json", SearchOption.AllDirectories)
            .All(path =>
            {
                using JsonDocument instance = JsonDocument.Parse(File.ReadAllText(path));
                return schema.Evaluate(instance.RootElement).IsValid;
            });
    }
}
