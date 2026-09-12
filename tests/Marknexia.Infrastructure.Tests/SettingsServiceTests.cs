using FluentAssertions;
using Marknexia.Core;
using Marknexia.Infrastructure;
using System.Net;
using Xunit;

namespace Marknexia.Infrastructure.Tests;

public sealed class SettingsServiceTests : IDisposable
{
    private readonly string _directory = Path.Combine(Path.GetTempPath(), $"marknexia-settings-{Guid.NewGuid():N}");

    [Fact]
    public void Settings_RoundTrip_PreservesThemeAndRecentItems()
    {
        string path = Path.Combine(_directory, "settings.json");
        var service = new SettingsService(path);
        service.Current.Theme = AppTheme.Dark;
        service.Current.IsSidebarOpen = false;
        service.Current.SidebarWidth = 360;
        service.Current.SidebarWidth = 360;
        service.Current.SidebarMode = 1;
        service.Current.AllowRemoteAssets = true;
        service.Current.RepositoryRoot = "C:\\workspace";
        service.AddRecentFile("  C:\\docs\\README.md  ");
        service.AddRecentFolder("C:\\workspace");

        var reloaded = new SettingsService(path);

        reloaded.Current.Theme.Should().Be(AppTheme.Dark);
        reloaded.Current.IsSidebarOpen.Should().BeFalse();
        reloaded.Current.SidebarWidth.Should().Be(360);
        reloaded.Current.SidebarWidth.Should().Be(360);
        reloaded.Current.SidebarMode.Should().Be(1);
        reloaded.Current.AllowRemoteAssets.Should().BeTrue();
        reloaded.Current.RepositoryRoot.Should().Be("C:\\workspace");
        reloaded.Current.RecentFiles.Should().ContainSingle().Which.Should().Be("C:\\docs\\README.md");
        reloaded.Current.RecentFolders.Should().ContainSingle().Which.Should().Be("C:\\workspace");
    }

    [Fact]
    public void MalformedSettings_FallsBackToDefaults()
    {
        Directory.CreateDirectory(_directory);
        string path = Path.Combine(_directory, "settings.json");
        File.WriteAllText(path, "{ not valid json");

        var service = new SettingsService(path);

        service.Current.Theme.Should().Be(AppTheme.System);
        service.Current.RecentFiles.Should().BeEmpty();
        service.Current.RecentFolders.Should().BeEmpty();
    }

    [Fact]
    public void Settings_Default_to_blocking_remote_assets()
    {
        string path = Path.Combine(_directory, "settings.json");

        var service = new SettingsService(path);

        service.Current.AllowRemoteAssets.Should().BeFalse();
    }

    [Fact]
    public void RecentItems_AreDeduplicatedAndBounded()
    {
        string path = Path.Combine(_directory, "settings.json");
        var service = new SettingsService(path);
        for (int i = 0; i < 20; i++) service.AddRecentFile($"C:\\docs\\{i}.md");
        service.AddRecentFile("C:\\docs\\19.md");

        service.Current.RecentFiles.Should().HaveCount(15);
        service.Current.RecentFiles[0].Should().Be("C:\\docs\\19.md");
        service.Current.RecentFiles.Distinct(StringComparer.OrdinalIgnoreCase).Should().HaveSameCount(service.Current.RecentFiles);
    }

    public void Dispose()
    {
        if (Directory.Exists(_directory)) Directory.Delete(_directory, recursive: true);
    }
}

public sealed class UpdateServiceTests
{
    [Fact]
    public void ParseLatestReleaseJson_RecognizesNewStableVersion()
    {
        const string json = "{\"tag_name\":\"v1.4.0\",\"html_url\":\"https://github.com/ramanacr/marknexia/releases/tag/v1.4.0\",\"draft\":false,\"prerelease\":false}";

        UpdateCheckResult result = UpdateService.ParseLatestReleaseJson(json, "1.3.0");

        result.IsUpdateAvailable.Should().BeTrue();
        result.LatestVersion.Should().Be("1.4.0");
        result.ReleaseUri.Should().NotBeNull();
    }

    [Fact]
    public void ParseLatestReleaseJson_IgnoresPrereleases()
    {
        const string json = "{\"tag_name\":\"v2.0.0\",\"html_url\":\"https://example.test/release\",\"draft\":false,\"prerelease\":true}";

        UpdateCheckResult result = UpdateService.ParseLatestReleaseJson(json, "1.0.0");

        result.IsUpdateAvailable.Should().BeFalse();
        result.Error.Should().NotBeNullOrWhiteSpace();
    }

    [Fact]
    public void ParseLatestReleaseJson_does_not_expose_an_untrusted_release_page()
    {
        const string json = "{\"tag_name\":\"v1.4.0\",\"html_url\":\"https://evil.example/release\",\"draft\":false,\"prerelease\":false}";

        UpdateCheckResult result = UpdateService.ParseLatestReleaseJson(json, "1.3.0");

        result.ReleaseUri.Should().BeNull();
    }

    [Fact]
    public async Task CheckAsync_allows_the_trusted_GitHub_API_endpoint()
    {
        const string json = "{\"tag_name\":\"v1.4.0\",\"html_url\":\"https://github.com/ramanacr/marknexia/releases/tag/v1.4.0\",\"draft\":false,\"prerelease\":false}";
        using var client = new HttpClient(new MappingHandler(new Dictionary<string, Func<HttpResponseMessage>>
        {
            [UpdateService.ReleaseApiEndpoint] = () => new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new StringContent(json)
            }
        }));

        UpdateCheckResult result = await new UpdateService(client).CheckAsync("1.3.0");

        result.IsUpdateAvailable.Should().BeTrue();
        result.LatestVersion.Should().Be("1.4.0");
        result.Error.Should().BeNull();
    }

    [Fact]
    public void ParseLatestReleaseJson_Exposes_only_supported_GitHub_portable_update_assets()
    {
        const string json = """
            {
              "tag_name":"v1.4.0",
              "html_url":"https://github.com/ramanacr/marknexia/releases/tag/v1.4.0",
              "draft":false,
              "prerelease":false,
              "assets":[
                {"name":"Marknexia-v1.4.0-win-x64.zip","browser_download_url":"https://github.com/ramanacr/marknexia/releases/download/v1.4.0/Marknexia-v1.4.0-win-x64.zip","size":123},
                {"name":"Marknexia-v1.4.0-win-x64.zip.sha256","browser_download_url":"https://github.com/ramanacr/marknexia/releases/download/v1.4.0/Marknexia-v1.4.0-win-x64.zip.sha256","size":80},
                {"name":"malicious.zip","browser_download_url":"https://evil.example/malicious.zip","size":123}
              ]
            }
            """;

        UpdateCheckResult result = UpdateService.ParseLatestReleaseJson(json, "1.3.0");

        result.PortablePackage.Should().NotBeNull();
        result.PortableChecksum.Should().NotBeNull();
        result.Assets.Should().NotContain(asset => asset.Name == "malicious.zip");
    }

    [Fact]
    public void ParseLatestReleaseJson_rejects_supported_hosts_on_non_default_https_ports()
    {
        const string json = """
            {
              "tag_name":"v1.4.0",
              "html_url":"https://github.com/ramanacr/marknexia/releases/tag/v1.4.0",
              "draft":false,
              "prerelease":false,
              "assets":[
                {"name":"Marknexia-v1.4.0-win-x64.zip","browser_download_url":"https://github.com:444/releases/download/v1.4.0/Marknexia-v1.4.0-win-x64.zip","size":123},
                {"name":"Marknexia-v1.4.0-win-x64.zip.sha256","browser_download_url":"https://github.com:444/releases/download/v1.4.0/Marknexia-v1.4.0-win-x64.zip.sha256","size":80}
              ]
            }
            """;

        UpdateCheckResult result = UpdateService.ParseLatestReleaseJson(json, "1.3.0");

        result.PortablePackage.Should().BeNull();
        result.PortableChecksum.Should().BeNull();
    }

    [Fact]
    public void ParseLatestReleaseJson_selects_portable_assets_by_requested_architecture()
    {
        const string json = """
            {
              "tag_name":"v1.4.0",
              "html_url":"https://github.com/ramanacr/marknexia/releases/tag/v1.4.0",
              "draft":false,
              "prerelease":false,
              "assets":[
                {"name":"Marknexia-v1.4.0-win-x64.zip","browser_download_url":"https://github.com/ramanacr/marknexia/releases/download/v1.4.0/Marknexia-v1.4.0-win-x64.zip","size":123},
                {"name":"Marknexia-v1.4.0-win-x64.zip.sha256","browser_download_url":"https://github.com/ramanacr/marknexia/releases/download/v1.4.0/Marknexia-v1.4.0-win-x64.zip.sha256","size":80},
                {"name":"Marknexia-v1.4.0-win-arm64.zip","browser_download_url":"https://github.com/ramanacr/marknexia/releases/download/v1.4.0/Marknexia-v1.4.0-win-arm64.zip","size":123},
                {"name":"Marknexia-v1.4.0-win-arm64.zip.sha256","browser_download_url":"https://github.com/ramanacr/marknexia/releases/download/v1.4.0/Marknexia-v1.4.0-win-arm64.zip.sha256","size":80}
              ]
            }
            """;

        UpdateCheckResult result = UpdateService.ParseLatestReleaseJson(json, "1.3.0");

        result.GetPortablePackage("x64")!.Name.Should().Be("Marknexia-v1.4.0-win-x64.zip");
        result.GetPortableChecksum("x64")!.Name.Should().Be("Marknexia-v1.4.0-win-x64.zip.sha256");
        result.GetPortablePackage("arm64")!.Name.Should().Be("Marknexia-v1.4.0-win-arm64.zip");
        result.GetPortableChecksum("arm64")!.Name.Should().Be("Marknexia-v1.4.0-win-arm64.zip.sha256");
        result.GetPortablePackage("x86").Should().BeNull();
    }

    [Fact]
    public async Task DownloadAndStageAsync_verifies_and_extracts_a_trusted_portable_package()
    {
        byte[] package = CreatePackage();
        string checksum = Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(package));
        const string packageUri = "https://github.com/ramanacr/marknexia/releases/download/v1.4.0/Marknexia-v1.4.0-win-x64.zip";
        const string checksumUri = packageUri + ".sha256";
        string json = $$"""
            {
              "tag_name":"v1.4.0",
              "html_url":"https://github.com/ramanacr/marknexia/releases/tag/v1.4.0",
              "draft":false,
              "prerelease":false,
              "assets":[
                {"name":"Marknexia-v1.4.0-win-x64.zip","browser_download_url":"{{packageUri}}","size":{{package.Length}}},
                {"name":"Marknexia-v1.4.0-win-x64.zip.sha256","browser_download_url":"{{checksumUri}}","size":{{checksum.Length}}}
              ]
            }
            """;
        UpdateCheckResult update = UpdateService.ParseLatestReleaseJson(json, "1.3.0");
        string updatesDirectory = Path.Combine(Path.GetTempPath(), $"marknexia-updates-{Guid.NewGuid():N}");

        using var client = new HttpClient(new MappingHandler(new Dictionary<string, Func<HttpResponseMessage>>
        {
            [packageUri] = () => new HttpResponseMessage(HttpStatusCode.OK) { Content = new ByteArrayContent(package) },
            [checksumUri] = () => new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent($"{checksum}  package.zip\n") }
        }));

        try
        {
            var staged = await new UpdateService(client).DownloadAndStageAsync(update, updatesDirectory);

            File.Exists(Path.Combine(staged.PayloadDirectory, "Marknexia.App.exe")).Should().BeTrue();
            (await File.ReadAllBytesAsync(Path.Combine(staged.PayloadDirectory, "Marknexia.App.exe")))
                .Should().Equal(0x4D, 0x5A, 0x01, 0x02);
        }
        finally
        {
            if (Directory.Exists(updatesDirectory)) Directory.Delete(updatesDirectory, recursive: true);
        }
    }

    [Fact]
    public async Task DownloadAndStageAsync_rejects_a_portable_payload_missing_the_bundled_sbom()
    {
        byte[] package = CreatePackage(includeSbom: false);
        string checksum = Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(package));
        const string packageUri = "https://github.com/ramanacr/marknexia/releases/download/v1.4.0/Marknexia-v1.4.0-win-x64.zip";
        const string checksumUri = packageUri + ".sha256";
        string json = $$"""
            {
              "tag_name":"v1.4.0",
              "html_url":"https://github.com/ramanacr/marknexia/releases/tag/v1.4.0",
              "draft":false,
              "prerelease":false,
              "assets":[
                {"name":"Marknexia-v1.4.0-win-x64.zip","browser_download_url":"{{packageUri}}","size":{{package.Length}}},
                {"name":"Marknexia-v1.4.0-win-x64.zip.sha256","browser_download_url":"{{checksumUri}}","size":{{checksum.Length}}}
              ]
            }
            """;
        UpdateCheckResult update = UpdateService.ParseLatestReleaseJson(json, "1.3.0");
        string updatesDirectory = Path.Combine(Path.GetTempPath(), $"marknexia-updates-{Guid.NewGuid():N}");

        using var client = new HttpClient(new MappingHandler(new Dictionary<string, Func<HttpResponseMessage>>
        {
            [packageUri] = () => new HttpResponseMessage(HttpStatusCode.OK) { Content = new ByteArrayContent(package) },
            [checksumUri] = () => new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent($"{checksum}  package.zip\n") }
        }));

        try
        {
            await Assert.ThrowsAsync<InvalidDataException>(() => new UpdateService(client).DownloadAndStageAsync(update, updatesDirectory));
            Directory.GetDirectories(updatesDirectory, "pending-*", SearchOption.TopDirectoryOnly).Should().BeEmpty();
        }
        finally
        {
            if (Directory.Exists(updatesDirectory)) Directory.Delete(updatesDirectory, recursive: true);
        }
    }

    [Fact]
    public async Task DownloadAndStageAsync_rejects_a_redirect_to_an_untrusted_host_and_cleans_up()
    {
        const string packageUri = "https://github.com/ramanacr/marknexia/releases/download/v1.4.0/Marknexia-v1.4.0-win-x64.zip";
        const string checksumUri = packageUri + ".sha256";
        const string json = $$"""
            {
              "tag_name":"v1.4.0",
              "html_url":"https://github.com/ramanacr/marknexia/releases/tag/v1.4.0",
              "draft":false,
              "prerelease":false,
              "assets":[
                {"name":"Marknexia-v1.4.0-win-x64.zip","browser_download_url":"{{packageUri}}","size":10},
                {"name":"Marknexia-v1.4.0-win-x64.zip.sha256","browser_download_url":"{{checksumUri}}","size":80}
              ]
            }
            """;
        UpdateCheckResult update = UpdateService.ParseLatestReleaseJson(json, "1.3.0");
        string updatesDirectory = Path.Combine(Path.GetTempPath(), $"marknexia-updates-{Guid.NewGuid():N}");

        using var client = new HttpClient(new MappingHandler(new Dictionary<string, Func<HttpResponseMessage>>
        {
            [packageUri] = () =>
            {
                var response = new HttpResponseMessage(HttpStatusCode.Redirect);
                response.Headers.Location = new Uri("https://evil.example/package.zip");
                return response;
            },
            [checksumUri] = () => new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent("0") }
        }));

        try
        {
            await Assert.ThrowsAsync<InvalidDataException>(() => new UpdateService(client).DownloadAndStageAsync(update, updatesDirectory));
            Directory.Exists(updatesDirectory).Should().BeTrue();
            Directory.GetDirectories(updatesDirectory, "pending-*", SearchOption.TopDirectoryOnly).Should().BeEmpty();
        }
        finally
        {
            if (Directory.Exists(updatesDirectory)) Directory.Delete(updatesDirectory, recursive: true);
        }
    }

    [Fact]
    public async Task DownloadAndStageAsync_rejects_zip_traversal_before_writing_outside_stage()
    {
        byte[] package = CreatePackage(includeTraversal: true);
        string checksum = Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(package));
        const string packageUri = "https://github.com/ramanacr/marknexia/releases/download/v1.4.0/Marknexia-v1.4.0-win-x64.zip";
        const string checksumUri = packageUri + ".sha256";
        string json = $$"""
            {
              "tag_name":"v1.4.0",
              "html_url":"https://github.com/ramanacr/marknexia/releases/tag/v1.4.0",
              "draft":false,
              "prerelease":false,
              "assets":[
                {"name":"Marknexia-v1.4.0-win-x64.zip","browser_download_url":"{{packageUri}}","size":{{package.Length}}},
                {"name":"Marknexia-v1.4.0-win-x64.zip.sha256","browser_download_url":"{{checksumUri}}","size":{{checksum.Length}}}
              ]
            }
            """;
        UpdateCheckResult update = UpdateService.ParseLatestReleaseJson(json, "1.3.0");
        string updatesDirectory = Path.Combine(Path.GetTempPath(), $"marknexia-updates-{Guid.NewGuid():N}");
        using var client = new HttpClient(new MappingHandler(new Dictionary<string, Func<HttpResponseMessage>>
        {
            [packageUri] = () => new HttpResponseMessage(HttpStatusCode.OK) { Content = new ByteArrayContent(package) },
            [checksumUri] = () => new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent($"{checksum}  package.zip\n") }
        }));

        try
        {
            await Assert.ThrowsAsync<InvalidDataException>(() => new UpdateService(client).DownloadAndStageAsync(update, updatesDirectory));
            File.Exists(Path.Combine(updatesDirectory, "escaped.txt")).Should().BeFalse();
            Directory.GetDirectories(updatesDirectory, "pending-*", SearchOption.TopDirectoryOnly).Should().BeEmpty();
        }
        finally
        {
            if (Directory.Exists(updatesDirectory)) Directory.Delete(updatesDirectory, recursive: true);
        }
    }

    private static byte[] CreatePackage(bool includeTraversal = false, bool includeSbom = true)
    {
        using var stream = new MemoryStream();
        using (var archive = new System.IO.Compression.ZipArchive(stream, System.IO.Compression.ZipArchiveMode.Create, leaveOpen: true))
        {
            if (includeTraversal)
            {
                var traversal = archive.CreateEntry("../../escaped.txt");
                using Stream traversalOutput = traversal.Open();
                traversalOutput.Write([0x65, 0x76, 0x69, 0x6C]);
            }

            AddEntry(archive, "Marknexia.App.exe", [0x4D, 0x5A, 0x01, 0x02]);
            AddEntry(archive, "Marknexia.App.dll", [0x4D, 0x5A, 0x03, 0x04]);
            AddEntry(archive, "Marknexia.App.pri", [0x50, 0x52, 0x49, 0x01]);

            if (includeSbom)
            {
                AddEntry(archive, "marknexia-sbom.spdx.json", [0x7B, 0x7D]);
            }
        }

        return stream.ToArray();

        static void AddEntry(System.IO.Compression.ZipArchive archive, string name, byte[] content)
        {
            var entry = archive.CreateEntry(name);
            using Stream output = entry.Open();
            output.Write(content);
        }
    }

    [Fact]
    public void BuildUpdateScript_ContainsRetryLogicAndProcessTermination()
    {
        string scriptPath = @"C:\Temp\marknexia-update-test.ps1";
        string target = @"C:\Program Files\Marknexia";
        string stage = @"C:\Temp\staging\payload";
        int ownerPid = 12345;

        string script = UpdateService.BuildUpdateScript(scriptPath, target, stage, ownerPid);

        script.Should().Contain("$ownerPid = 12345");
        script.Should().Contain("Stop-TargetProcesses");
        script.Should().Contain("Copy-WithRetry");
        script.Should().Contain("Write-UpdateLog");
        script.Should().Contain("marknexia-update.log");
        script.Should().Contain("Start-Process -FilePath $targetExe");
        script.Should().Contain("Rollback");
    }

    [Fact]
    public void BuildUpdateScript_ProperlyEscapesPathsWithSingleQuotes()
    {
        string scriptPath = @"C:\Temp\mark's update.ps1";
        string target = @"C:\Program Files\Mark's 'App'";
        string stage = @"C:\Temp\staging's\payload";
        int ownerPid = 999;

        string script = UpdateService.BuildUpdateScript(scriptPath, target, stage, ownerPid);

        script.Should().Contain("Mark''s ''App''");
        script.Should().Contain("staging''s");
        script.Should().Contain("mark''s update.ps1");
    }

    private sealed class MappingHandler(IReadOnlyDictionary<string, Func<HttpResponseMessage>> responses) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            if (!responses.TryGetValue(request.RequestUri?.ToString() ?? string.Empty, out Func<HttpResponseMessage>? factory))
                return Task.FromResult(new HttpResponseMessage(HttpStatusCode.NotFound));
            return Task.FromResult(factory());
        }
    }
}

