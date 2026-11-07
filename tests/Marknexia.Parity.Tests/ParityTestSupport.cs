using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Marknexia.ParityExporter;

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

    public static void CopyDirectory(string source, string destination)
    {
        foreach (string file in Directory.EnumerateFiles(source, "*", SearchOption.AllDirectories))
        {
            string target = Path.Combine(destination, Path.GetRelativePath(source, file));
            Directory.CreateDirectory(Path.GetDirectoryName(target)!);
            File.Copy(file, target, overwrite: true);
        }
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
        try
        {
            ParityFixtureValidator.Validate(fixtureRoot, FindRepositoryRoot());
            return true;
        }
        catch (InvalidDataException) { return false; }
    }

    public static int ReadManifestCaseCount(string fixtureRoot)
    {
        using JsonDocument manifest = JsonDocument.Parse(File.ReadAllText(Path.Combine(fixtureRoot, "manifest.json")));
        return manifest.RootElement.GetProperty("caseCount").GetInt32();
    }

    public static string[] AreaNames(string fixtureRoot) => Directory.EnumerateDirectories(fixtureRoot)
        .Select(Path.GetFileName).Where(name => !string.IsNullOrWhiteSpace(name)).Cast<string>().Order(StringComparer.Ordinal).ToArray();
}
