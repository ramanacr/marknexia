using System.Diagnostics;
using System.IO.Compression;
using System.Net;
using System.Net.Http.Headers;
using System.Security.Cryptography;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Text.RegularExpressions;

namespace Marknexia.Infrastructure;

public sealed record UpdateAsset(string Name, Uri DownloadUri, long SizeBytes);

public sealed record StagedUpdate(string Version, string StageDirectory, string PayloadDirectory);

public sealed record UpdateCheckResult(
    bool IsUpdateAvailable,
    string CurrentVersion,
    string? LatestVersion,
    Uri? ReleaseUri,
    string? Error = null)
{
    public IReadOnlyList<UpdateAsset> Assets { get; init; } = Array.Empty<UpdateAsset>();

    public UpdateAsset? PortablePackage => GetPortablePackage(UpdateService.CurrentArchitectureAssetToken);

    public UpdateAsset? PortableChecksum => GetPortableChecksum(UpdateService.CurrentArchitectureAssetToken);

    public UpdateAsset? GetPortablePackage(string architectureToken)
    {
        if (LatestVersion is null || string.IsNullOrWhiteSpace(architectureToken)) return null;
        return Assets.FirstOrDefault(asset => asset.Name.Equals(
            $"Marknexia-v{LatestVersion}-win-{architectureToken}.zip",
            StringComparison.OrdinalIgnoreCase));
    }

    public UpdateAsset? GetPortableChecksum(string architectureToken)
    {
        UpdateAsset? package = GetPortablePackage(architectureToken);
        return package is null
            ? null
            : Assets.FirstOrDefault(asset => asset.Name.Equals(package.Name + ".sha256", StringComparison.OrdinalIgnoreCase));
    }
}

public sealed class UpdateService
{
    public const string ReleaseApiEndpoint = "https://api.github.com/repos/ramanacr/marknexia/releases/latest";
    public const long MaxDownloadBytes = 1_000_000_000;

    public static string CurrentArchitectureAssetToken => RuntimeInformation.ProcessArchitecture switch
    {
        Architecture.X64 => "x64",
        Architecture.Arm64 => "arm64",
        _ => "unsupported"
    };

    private readonly HttpClient _client;

    public UpdateService(HttpClient? client = null)
    {
        _client = client ?? CreateClient();
    }

    public async Task<UpdateCheckResult> CheckAsync(string currentVersion, CancellationToken cancellationToken = default)
    {
        try
        {
            using HttpResponseMessage response = await GetTrustedResponseAsync(new Uri(ReleaseApiEndpoint), cancellationToken);
            response.EnsureSuccessStatusCode();
            string json = await response.Content.ReadAsStringAsync(cancellationToken);
            return ParseLatestReleaseJson(json, currentVersion);
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested) { throw; }
        catch (Exception ex) { return new UpdateCheckResult(false, currentVersion, null, null, ex.Message); }
    }

    public static UpdateCheckResult ParseLatestReleaseJson(string json, string currentVersion)
    {
        try
        {
            var release = JsonSerializer.Deserialize<GitHubRelease>(json);
            if (release == null || release.Draft || release.Prerelease || string.IsNullOrWhiteSpace(release.TagName))
                return new UpdateCheckResult(false, currentVersion, null, null, "No stable release is available.");

            string latestVersion = NormalizeVersion(release.TagName);
            IReadOnlyList<UpdateAsset> assets = ParseAssets(release.Assets);
            if (!Version.TryParse(NormalizeVersion(currentVersion), out Version? current)
                || !Version.TryParse(latestVersion, out Version? latest))
            {
                return new UpdateCheckResult(false, currentVersion, latestVersion, ParseUri(release.HtmlUrl), "Release version is not comparable.") { Assets = assets };
            }

            return new UpdateCheckResult(latest > current, currentVersion, latestVersion, ParseUri(release.HtmlUrl)) { Assets = assets };
        }
        catch (JsonException ex)
        {
            return new UpdateCheckResult(false, currentVersion, null, null, $"Invalid release metadata: {ex.Message}");
        }
    }

    public async Task<StagedUpdate> DownloadAndStageAsync(UpdateCheckResult update, string? updatesDirectory = null, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(update);
        string architectureToken = CurrentArchitectureAssetToken;
        if (architectureToken == "unsupported")
            throw new PlatformNotSupportedException("Self-update is supported only for x64 and ARM64 builds.");

        if (!update.IsUpdateAvailable || string.IsNullOrWhiteSpace(update.LatestVersion)
            || update.GetPortablePackage(architectureToken) is not UpdateAsset package
            || update.GetPortableChecksum(architectureToken) is not UpdateAsset checksum)
            throw new InvalidOperationException($"This release does not contain a verified {architectureToken} portable package.");
        if (package.SizeBytes > MaxDownloadBytes || checksum.SizeBytes > 1024 * 1024)
            throw new InvalidDataException("The update release assets are larger than the supported limits.");

        string root = Path.GetFullPath(updatesDirectory ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Marknexia", "Updates"));
        Directory.CreateDirectory(root);
        string stage = Path.Combine(root, $"pending-{Guid.NewGuid():N}");
        string payload = Path.Combine(stage, "payload");
        Directory.CreateDirectory(payload);
        try
        {
            string archivePath = Path.Combine(stage, "package.zip");
            string checksumPath = Path.Combine(stage, "package.sha256");
            await DownloadToFileAsync(package.DownloadUri, archivePath, MaxDownloadBytes, cancellationToken);
            await DownloadToFileAsync(checksum.DownloadUri, checksumPath, 1024 * 1024, cancellationToken);
            await VerifySha256Async(archivePath, checksumPath, cancellationToken);
            await ExtractArchiveSafelyAsync(archivePath, payload, cancellationToken);
            ValidatePortablePayload(payload);
            File.Delete(archivePath);
            File.Delete(checksumPath);
            return new StagedUpdate(update.LatestVersion, stage, payload);
        }
        catch { TryDeleteDirectory(stage); throw; }
    }

    public static Process StartUpdateAndRestart(StagedUpdate stagedUpdate, string targetDirectory, int ownerProcessId)
    {
        ArgumentNullException.ThrowIfNull(stagedUpdate);
        string target = Path.GetFullPath(targetDirectory);
        string stage = Path.GetFullPath(stagedUpdate.PayloadDirectory);
        if (!File.Exists(Path.Combine(stage, "Marknexia.App.exe")) || !Directory.Exists(target))
            throw new InvalidOperationException("The staged update or installation directory is missing.");

        string scriptPath = Path.Combine(Path.GetTempPath(), $"marknexia-update-{Guid.NewGuid():N}.ps1");
        string script = BuildUpdateScript(scriptPath, target, stage, ownerProcessId);
        File.WriteAllText(scriptPath, script, Encoding.UTF8);
        string encodedCommand = Convert.ToBase64String(Encoding.Unicode.GetBytes(script));
        var startInfo = new ProcessStartInfo { FileName = "powershell.exe", UseShellExecute = false, CreateNoWindow = true, WindowStyle = ProcessWindowStyle.Hidden };
        startInfo.ArgumentList.Add("-NoProfile");
        startInfo.ArgumentList.Add("-NonInteractive");
        startInfo.ArgumentList.Add("-ExecutionPolicy");
        startInfo.ArgumentList.Add("Bypass");
        startInfo.ArgumentList.Add("-EncodedCommand");
        startInfo.ArgumentList.Add(encodedCommand);
        try { return Process.Start(startInfo) ?? throw new InvalidOperationException("Could not start the update worker."); }
        catch { try { File.Delete(scriptPath); } catch { } throw; }
    }

    private static IReadOnlyList<UpdateAsset> ParseAssets(IReadOnlyList<GitHubAsset>? assets)
    {
        if (assets is null) return Array.Empty<UpdateAsset>();
        return assets.Where(asset => !string.IsNullOrWhiteSpace(asset.Name) && asset.Size >= 0
                && Uri.TryCreate(asset.DownloadUrl, UriKind.Absolute, out Uri? uri) && IsTrustedDownloadHost(uri))
            .Select(asset => new UpdateAsset(asset.Name!, new Uri(asset.DownloadUrl!), asset.Size)).ToArray();
    }

    private static bool IsTrustedDownloadHost(Uri uri) => uri.Scheme.Equals("https", StringComparison.OrdinalIgnoreCase)
        && uri.IsDefaultPort
        && (uri.Host.Equals("github.com", StringComparison.OrdinalIgnoreCase)
            || uri.Host.Equals("api.github.com", StringComparison.OrdinalIgnoreCase)
            || uri.Host.Equals("objects.githubusercontent.com", StringComparison.OrdinalIgnoreCase)
            || uri.Host.Equals("release-assets.githubusercontent.com", StringComparison.OrdinalIgnoreCase));

    private async Task DownloadToFileAsync(Uri uri, string destination, long maxBytes, CancellationToken cancellationToken)
    {
        using HttpResponseMessage response = await GetTrustedResponseAsync(uri, cancellationToken);
        response.EnsureSuccessStatusCode();
        if (response.Content.Headers.ContentLength is long length && length > maxBytes)
            throw new InvalidDataException("The update package is larger than the supported limit.");
        await using FileStream output = new(destination, FileMode.CreateNew, FileAccess.Write, FileShare.None, 64 * 1024, FileOptions.Asynchronous | FileOptions.SequentialScan);
        await using Stream input = await response.Content.ReadAsStreamAsync(cancellationToken);
        long total = 0;
        byte[] buffer = new byte[64 * 1024];
        int read;
        while ((read = await input.ReadAsync(buffer, cancellationToken)) > 0)
        {
            total += read;
            if (total > maxBytes) throw new InvalidDataException("The update package is larger than the supported limit.");
            await output.WriteAsync(buffer.AsMemory(0, read), cancellationToken);
        }
    }

    private static void ValidatePortablePayload(string payloadDirectory)
    {
        string[] requiredFiles =
        [
            "Marknexia.App.exe",
            "Marknexia.App.dll",
            "Marknexia.App.pri",
            "marknexia-sbom.spdx.json"
        ];

        foreach (string requiredFile in requiredFiles)
        {
            if (!File.Exists(Path.Combine(payloadDirectory, requiredFile)))
                throw new InvalidDataException($"The update package is missing required file '{requiredFile}'.");
        }
    }

    private async Task<HttpResponseMessage> GetTrustedResponseAsync(Uri uri, CancellationToken cancellationToken)
    {
        Uri current = uri;
        for (int redirect = 0; redirect < 4; redirect++)
        {
            if (!IsTrustedDownloadHost(current))
                throw new InvalidDataException("Update download host is not trusted.");

            using var request = new HttpRequestMessage(HttpMethod.Get, current);
            HttpResponseMessage response = await _client.SendAsync(
                request,
                HttpCompletionOption.ResponseHeadersRead,
                cancellationToken);
            if (response.StatusCode is not (HttpStatusCode.Moved
                or HttpStatusCode.Redirect
                or HttpStatusCode.SeeOther
                or HttpStatusCode.TemporaryRedirect
                or HttpStatusCode.PermanentRedirect))
            {
                return response;
            }

            Uri? location = response.Headers.Location;
            response.Dispose();
            if (location is null) throw new InvalidDataException("The update server returned an invalid redirect.");
            current = location.IsAbsoluteUri ? location : new Uri(current, location);
        }

        throw new InvalidDataException("The update server returned too many redirects.");
    }

    private static async Task VerifySha256Async(string archivePath, string checksumPath, CancellationToken cancellationToken)
    {
        string checksumText = await File.ReadAllTextAsync(checksumPath, cancellationToken);
        Match match = Regex.Match(checksumText, @"\b[0-9a-fA-F]{64}\b");
        if (!match.Success) throw new InvalidDataException("The update checksum file is invalid.");
        await using FileStream input = new(archivePath, FileMode.Open, FileAccess.Read, FileShare.Read, 64 * 1024, FileOptions.Asynchronous | FileOptions.SequentialScan);
        string actual = Convert.ToHexString(await SHA256.HashDataAsync(input, cancellationToken));
        if (!actual.Equals(match.Value, StringComparison.OrdinalIgnoreCase)) throw new InvalidDataException("The downloaded update checksum does not match.");
    }

    private static async Task ExtractArchiveSafelyAsync(string archivePath, string destination, CancellationToken cancellationToken)
    {
        await using FileStream input = new(archivePath, FileMode.Open, FileAccess.Read, FileShare.Read, 64 * 1024, FileOptions.Asynchronous | FileOptions.SequentialScan);
        using var archive = new ZipArchive(input, ZipArchiveMode.Read, leaveOpen: false);
        long total = 0;
        byte[] buffer = new byte[64 * 1024];
        foreach (ZipArchiveEntry entry in archive.Entries)
        {
            cancellationToken.ThrowIfCancellationRequested();
            string name = entry.FullName.Replace('\\', '/');
            if (string.IsNullOrWhiteSpace(name)) continue;
            if (name.StartsWith('/') || name.Contains(':', StringComparison.Ordinal)) throw new InvalidDataException("The update archive contains an invalid path.");
            string target = Path.GetFullPath(Path.Combine(destination, name.Replace('/', Path.DirectorySeparatorChar)));
            if (!IsWithinDirectory(target, destination)) throw new InvalidDataException("The update archive contains a path traversal entry.");
            if (name.EndsWith('/')) { Directory.CreateDirectory(target); continue; }
            if (entry.Length < 0 || entry.Length > MaxDownloadBytes || total > MaxDownloadBytes - entry.Length)
                throw new InvalidDataException("The update archive is larger than the supported limit.");
            Directory.CreateDirectory(Path.GetDirectoryName(target)!);
            await using Stream source = entry.Open();
            await using FileStream output = new(target, FileMode.CreateNew, FileAccess.Write, FileShare.None, 64 * 1024, FileOptions.Asynchronous | FileOptions.SequentialScan);
            long entryBytes = 0;
            int read;
            while ((read = await source.ReadAsync(buffer, cancellationToken)) > 0)
            {
                entryBytes += read;
                if (entryBytes > entry.Length || total > MaxDownloadBytes - entryBytes)
                    throw new InvalidDataException("The update archive is larger than the supported limit.");
                await output.WriteAsync(buffer.AsMemory(0, read), cancellationToken);
            }

            total += entryBytes;
        }
    }

    private static bool IsWithinDirectory(string candidate, string root)
    {
        string normalizedCandidate = Path.TrimEndingDirectorySeparator(Path.GetFullPath(candidate));
        string normalizedRoot = Path.TrimEndingDirectorySeparator(Path.GetFullPath(root));
        return normalizedCandidate.StartsWith(normalizedRoot + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase);
    }

    private static string BuildUpdateScript(string scriptPath, string target, string stage, int ownerProcessId)
    {
        string targetLiteral = EscapePowerShellLiteral(target);
        string stageLiteral = EscapePowerShellLiteral(stage);
        string scriptLiteral = EscapePowerShellLiteral(scriptPath);
        string rollbackName = $".marknexia-rollback-{Guid.NewGuid():N}";
        string script = """
            $ErrorActionPreference = 'Stop'
            $ownerPid = __OWNER_PID__
            $target = '__TARGET__'
            $stage = '__STAGE__'
            $scriptPath = '__SCRIPT_PATH__'
            $backup = Join-Path ([IO.Path]::GetDirectoryName($target)) '__ROLLBACK__'
            try {
                for ($i = 0; $i -lt 150; $i++) {
                    if (-not (Get-Process -Id $ownerPid -ErrorAction SilentlyContinue)) { break }
                    Start-Sleep -Milliseconds 200
                }
                if (Get-Process -Id $ownerPid -ErrorAction SilentlyContinue) { throw 'Marknexia did not exit in time.' }
                New-Item -ItemType Directory -Path $backup -Force | Out-Null
                Get-ChildItem -LiteralPath $target -Force | Move-Item -Destination $backup -Force
                Get-ChildItem -LiteralPath $stage -Force | Copy-Item -Destination $target -Recurse -Force
                if (-not (Test-Path -LiteralPath (Join-Path $target 'Marknexia.App.exe'))) { throw 'The update executable is missing after installation.' }
                Start-Process -FilePath (Join-Path $target 'Marknexia.App.exe')
                Remove-Item -LiteralPath $backup -Recurse -Force
                Remove-Item -LiteralPath $stage -Recurse -Force
            } catch {
                Get-ChildItem -LiteralPath $target -Force -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
                Get-ChildItem -LiteralPath $backup -Force -ErrorAction SilentlyContinue | Move-Item -Destination $target -Force -ErrorAction SilentlyContinue
                if (Test-Path -LiteralPath (Join-Path $target 'Marknexia.App.exe')) { Start-Process -FilePath (Join-Path $target 'Marknexia.App.exe') }
            } finally {
                Remove-Item -LiteralPath $scriptPath -Force -ErrorAction SilentlyContinue
            }
            """;
        return script
            .Replace("__OWNER_PID__", ownerProcessId.ToString(System.Globalization.CultureInfo.InvariantCulture), StringComparison.Ordinal)
            .Replace("__TARGET__", targetLiteral, StringComparison.Ordinal)
            .Replace("__STAGE__", stageLiteral, StringComparison.Ordinal)
            .Replace("__SCRIPT_PATH__", scriptLiteral, StringComparison.Ordinal)
            .Replace("__ROLLBACK__", rollbackName, StringComparison.Ordinal);
    }

    private static string EscapePowerShellLiteral(string value) => value.Replace("'", "''", StringComparison.Ordinal);
    private static void TryDeleteDirectory(string path) { try { if (Directory.Exists(path)) Directory.Delete(path, recursive: true); } catch { } }

    private static HttpClient CreateClient()
    {
        var client = new HttpClient(new HttpClientHandler { AllowAutoRedirect = false })
        {
            Timeout = TimeSpan.FromSeconds(30)
        };
        client.DefaultRequestHeaders.UserAgent.Add(new ProductInfoHeaderValue("Marknexia", "1.0"));
        client.DefaultRequestHeaders.Accept.Add(new MediaTypeWithQualityHeaderValue("application/vnd.github+json"));
        return client;
    }

    private static string NormalizeVersion(string value)
    {
        string normalized = value.Trim();
        while (normalized.StartsWith('v') || normalized.StartsWith('V')) normalized = normalized[1..];
        int dash = normalized.IndexOf('-');
        return dash >= 0 ? normalized[..dash] : normalized;
    }

    private static Uri? ParseUri(string? value) => Uri.TryCreate(value, UriKind.Absolute, out Uri? uri)
        && uri.Scheme.Equals("https", StringComparison.OrdinalIgnoreCase)
        && uri.IsDefaultPort
        && uri.Host.Equals("github.com", StringComparison.OrdinalIgnoreCase)
        && uri.AbsolutePath.StartsWith("/ramanacr/marknexia/", StringComparison.OrdinalIgnoreCase)
        ? uri : null;

    private sealed record GitHubRelease(
        [property: JsonPropertyName("tag_name")] string? TagName,
        [property: JsonPropertyName("html_url")] string? HtmlUrl,
        [property: JsonPropertyName("draft")] bool Draft,
        [property: JsonPropertyName("prerelease")] bool Prerelease,
        [property: JsonPropertyName("assets")] IReadOnlyList<GitHubAsset>? Assets);

    private sealed record GitHubAsset(
        [property: JsonPropertyName("name")] string? Name,
        [property: JsonPropertyName("browser_download_url")] string? DownloadUrl,
        [property: JsonPropertyName("size")] long Size);
}
