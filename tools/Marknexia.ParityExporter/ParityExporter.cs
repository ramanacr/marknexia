using System.Diagnostics;
using System.IO.Compression;
using System.Net;
using System.Net.Http;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using Marknexia.Core;
using Marknexia.Files;
using Marknexia.Infrastructure;
using Marknexia.Markdown;
using Marknexia.Navigation;
using Marknexia.Rendering;
using Marknexia.Security;

namespace Marknexia.ParityExporter;

public sealed class ParityExporter
{
    private static readonly JsonSerializerOptions JsonOptions = new() { WriteIndented = true };
    private readonly string _repositoryRoot;
    private readonly string? _frozenSourceRevision;

    public ParityExporter(string repositoryRoot, string? frozenSourceRevision = null)
    {
        _repositoryRoot = ValidateRepositoryRoot(repositoryRoot);
        _frozenSourceRevision = frozenSourceRevision;
    }

    public async Task ExportAsync(string outputRoot, CancellationToken cancellationToken)
    {
        string sourceRevision = ResolveSourceRevision(_repositoryRoot, _frozenSourceRevision);
        string fullOutputRoot = Path.GetFullPath(outputRoot);
        ValidateOutputRoot(_repositoryRoot, fullOutputRoot);

        string temporaryRoot = fullOutputRoot + ".building-" + Guid.NewGuid().ToString("N");
        Directory.CreateDirectory(temporaryRoot);
        try
        {
            await ExportToDirectoryAsync(temporaryRoot, sourceRevision, cancellationToken);
            ReplaceOwnedFixtureDirectory(fullOutputRoot, temporaryRoot);
        }
        catch
        {
            TryDeleteDirectory(temporaryRoot);
            throw;
        }
    }

    private async Task ExportToDirectoryAsync(string outputRoot, string sourceRevision, CancellationToken cancellationToken)
    {
        List<ParityCase> cases = await BuildCasesAsync(sourceRevision, cancellationToken);
        foreach (ParityCase parityCase in cases.OrderBy(item => item.Area, StringComparer.Ordinal).ThenBy(item => item.Name, StringComparer.Ordinal))
        {
            string directory = Path.Combine(outputRoot, parityCase.Area);
            Directory.CreateDirectory(directory);
            string path = Path.Combine(directory, parityCase.Name + ".case.json");
            WriteCanonicalJson(path, ToEnvelope(parityCase));
        }

        WriteCanonicalJson(Path.Combine(outputRoot, "manifest.json"), new JsonObject
        {
            ["schemaVersion"] = ParityCase.SchemaVersion,
            ["caseCount"] = cases.Count,
            ["cases"] = new JsonArray(cases.OrderBy(item => item.Area, StringComparer.Ordinal).ThenBy(item => item.Name, StringComparer.Ordinal)
                .Select(item => JsonValue.Create($"{item.Area}/{item.Name}.case.json")).ToArray())
        });
    }

    private async Task<List<ParityCase>> BuildCasesAsync(string sourceRevision, CancellationToken cancellationToken)
    {
        var cases = new List<ParityCase>();
        var parser = new MarkdigParserAdapter();
        var sanitizer = new HtmlSanitizerService();
        string fixtureRoot = Path.Combine(_repositoryRoot, "test-fixtures", "markdown");

        foreach (string file in Directory.EnumerateFiles(fixtureRoot, "*.md", SearchOption.AllDirectories)
                     .OrderBy(path => Path.GetRelativePath(fixtureRoot, path), StringComparer.Ordinal))
        {
            string relative = Path.GetRelativePath(fixtureRoot, file).Replace('\\', '/');
            string markdown = await File.ReadAllTextAsync(file, cancellationToken);
            ParsedMarkdown parsed = parser.Parse(markdown);
            cases.Add(Case("markdown", CaseName(relative), new JsonObject { ["markdown"] = markdown, ["sourceFile"] = relative }, null,
                ParsedExpected(parsed), sourceRevision));
        }

        foreach ((string name, string markdown) in new[]
        {
            ("duplicate-formatted-headings", "# Hello *World*\n# Hello World\n"),
            ("custom-anchor", "<a id=\"legacy-anchor\"></a>\n# Heading\n"),
            ("tables", "| A | B |\n| - | - |\n| 1 | 2 |\n"),
            ("grid-tables", "+---+---+\n| A | B |\n+===+===+\n| 1 | 2 |\n+---+---+\n"),
            ("task-lists", "- [x] done\n- [ ] todo\n"),
            ("footnotes", "Note.[^1]\n\n[^1]: footnote\n"),
            ("mathematics", "Inline $x^2$ and $$y$$\n"),
            ("emphasis-extras", "~~deleted~~ ==marked==\n"),
            ("alerts", "> [!WARNING]\n> Be careful.\n"),
            ("language-labels", "```c#\nvar value = 1;\n```\n"),
            ("mermaid-block", "```mermaid\ngraph TD\nA-->B\n```\n")
        })
        {
            cases.Add(Case("headings", name, new JsonObject { ["markdown"] = markdown }, null, ParsedExpected(parser.Parse(markdown)), sourceRevision));
        }

        var canonicalizer = new PathCanonicalizer();
        var navigationRoot = Path.Combine(fixtureRoot, "navigation");
        var resolver = new NavigationResolver(canonicalizer, new FileService(canonicalizer));
        var context = new ResolutionContext(Path.Combine(navigationRoot, "README.md"), navigationRoot, null, new NavigationPolicy());
        foreach ((string name, string destination) in new[]
        {
            ("percent-decodes-exactly-once", "docs/literal%2520.md#literal%2520"),
            ("encoded-traversal", "%2e%2e/%2e%2e/secret.md"),
            ("unc-rejected", "//server.test/share/document.md"),
            ("file-uri-rejected", "file://server.test/share/document.md"),
            ("remote-link", "https://github.com"),
            ("external-disabled", "https://github.com")
        })
        {
            ResolutionContext caseContext = name == "external-disabled"
                ? new ResolutionContext(context.CurrentFilePath, context.RepositoryRoot, null, new NavigationPolicy(AllowExternalLinks: false))
                : context;
            NavigationIntent intent = resolver.Resolve(destination, caseContext);
            cases.Add(Case("navigation", name, NavigationInput(destination, caseContext, navigationRoot), VirtualFileSystem(navigationRoot), IntentExpected(intent, navigationRoot), sourceRevision));
        }

        foreach ((string name, string destination) in new[] { ("unc", "//server.test/share/document.md"), ("encoded-unc", "%2f%2fserver.test/share/document.md"), ("file-uri", "file://server.test/share/document.md") })
        {
            var recordingFiles = new RecordingFileService();
            var recordingResolver = new NavigationResolver(canonicalizer, recordingFiles);
            var probeContext = new ResolutionContext(Path.Combine(navigationRoot, "README.md"), null, null, new NavigationPolicy());
            NavigationIntent intent = recordingResolver.Resolve(destination, probeContext);
            cases.Add(Case("navigation", "rejected-before-probe-" + name, NavigationInput(destination, probeContext, navigationRoot), VirtualFileSystem(navigationRoot), new JsonObject
            {
                ["intent"] = IntentExpected(intent, navigationRoot),
                ["fileSystemProbeCount"] = recordingFiles.CheckedPaths.Count
            }, sourceRevision));
        }

        foreach ((string name, string html, bool svg) in new[]
        {
            ("unsafe-html", "<script>alert(1)</script><a href=\"javascript:alert(1)\">bad</a>", false),
            ("unsafe-css", "<div style=\"background:url(javascript:alert(1))\">text</div>", false),
            ("unsafe-uri", "<img src=\"vbscript:msgbox(1)\">", false),
            ("unsafe-svg", "<svg onclick=\"alert(1)\"><script>alert(1)</script><path d=\"M0 0\" /></svg>", true)
        })
        {
            string result = svg ? sanitizer.SanitizeSvg(html) : sanitizer.SanitizeHtml(html);
            cases.Add(Case("sanitizer", name, new JsonObject { ["html"] = html, ["mode"] = svg ? "svg" : "html" }, null,
                new JsonObject { ["sanitizedHtml"] = result }, sourceRevision));
        }

        var renderer = new MarkdownRenderer();
        foreach ((string name, string markdown, bool remote, int maxDiagrams, long maxSource, long htmlLimit) in new[]
        {
            ("remote-images-off", "![remote](https://example.test/image.png)", false, 8, 1_000L, MarkdownRenderer.MaxRenderedHtmlBytes),
            ("remote-images-on", "![remote](https://example.test/image.png)", true, 8, 1_000L, MarkdownRenderer.MaxRenderedHtmlBytes),
            ("code-copy-metadata", "```c#\nvar value = 1;\n```", false, 8, 1_000L, MarkdownRenderer.MaxRenderedHtmlBytes),
            ("diagram-limit", "```mermaid\ngraph TD\nA-->B\n```\n```mermaid\ngraph TD\nB-->C\n```", false, 1, 1_000L, MarkdownRenderer.MaxRenderedHtmlBytes),
            ("diagram-source-limit", "```mermaid\ngraph TD\nA-->B\n```", false, 8, 1L, MarkdownRenderer.MaxRenderedHtmlBytes),
            ("rendered-output-limit", "# bounded output", false, 8, 1_000L, 1L)
        })
        {
            var boundedRenderer = new MarkdownRenderer(renderedHtmlLimitBytes: htmlLimit, maxDiagramCount: maxDiagrams, maxDiagramSourceBytes: maxSource);
            JsonObject input = new() { ["markdown"] = markdown, ["allowRemoteAssets"] = remote, ["theme"] = AppTheme.System.ToString(), ["sourcePath"] = "compat/virtual.md", ["repositoryRoot"] = ".", ["enableDiagrams"] = true, ["enableMath"] = true, ["maxDiagramCount"] = maxDiagrams, ["maxDiagramSourceBytes"] = maxSource, ["maxRenderedHtmlBytes"] = htmlLimit };
            try
            {
                RenderedDocument document = await boundedRenderer.RenderAsync(markdown,
                    new RenderContext(Path.Combine(_repositoryRoot, "compat", "virtual.md"), _repositoryRoot, AppTheme.System, AllowRemoteAssets: remote), cancellationToken);
                cases.Add(Case("rendering", name, input, null,
                    new JsonObject { ["outcome"] = "rendered", ["html"] = ParityNormalizer.NormalizeHtml(document.HtmlContent), ["assets"] = ToNode(document.AssetReferences) }, sourceRevision));
            }
            catch (DocumentTooLargeException exception)
            {
                cases.Add(Case("rendering", name, input, null, new JsonObject { ["outcome"] = "rejected", ["exception"] = exception.GetType().Name, ["maximumBytes"] = exception.MaximumBytes }, sourceRevision));
            }
        }

        cases.Add(CreateMarkdownInputBoundCase(sourceRevision));

        cases.Add(SettingsCase("malformed-settings", "{ not valid json", sourceRevision));
        cases.Add(SettingsCase("recent-item-bounds", null, sourceRevision));

        cases.AddRange(await BuildUpdateArchiveCasesAsync(sourceRevision, cancellationToken));

        return cases;
    }

    private ParityCase SettingsCase(string name, string? malformedJson, string sourceRevision)
    {
        string directory = Path.Combine(Path.GetTempPath(), "marknexia-parity-settings-" + Guid.NewGuid().ToString("N"));
        string path = Path.Combine(directory, "settings.json");
        try
        {
            Directory.CreateDirectory(directory);
            if (malformedJson is not null) File.WriteAllText(path, malformedJson);
            var service = new SettingsService(path);
            JsonArray operations = new();
            if (malformedJson is null)
            {
                for (int index = 0; index < 20; index++)
                {
                    string recent = $"C:\\docs\\{index}.md";
                    operations.Add(new JsonObject { ["operation"] = "addRecentFile", ["path"] = recent });
                    service.AddRecentFile(recent);
                }
                operations.Add(new JsonObject { ["operation"] = "addRecentFile", ["path"] = "C:\\docs\\19.md" });
                service.AddRecentFile("C:\\docs\\19.md");
            }
            return Case("settings", name, new JsonObject { ["settingsJson"] = malformedJson, ["operations"] = operations }, null, new JsonObject
            {
                ["theme"] = service.Current.Theme.ToString(),
                ["allowRemoteAssets"] = service.Current.AllowRemoteAssets,
                ["recentFiles"] = ToNode(service.Current.RecentFiles)
            }, sourceRevision);
        }
        finally
        {
            if (Directory.Exists(directory)) Directory.Delete(directory, recursive: true);
        }
    }

    private static JsonObject NavigationInput(string destination, ResolutionContext context, string virtualRoot) => new()
    {
        ["destination"] = destination,
        ["currentFile"] = Path.GetRelativePath(virtualRoot, context.CurrentFilePath).Replace('\\', '/'),
        ["repositoryRoot"] = context.RepositoryRoot is null ? null : "navigation",
        ["policy"] = new JsonObject
        {
            ["enforceRepositorySandbox"] = context.Policy.EnforceRepositorySandbox,
            ["allowExternalLinks"] = context.Policy.AllowExternalLinks,
            ["allowRemoteAssets"] = context.Policy.AllowRemoteAssets
        }
    };

    private static ParityCase CreateMarkdownInputBoundCase(string sourceRevision)
    {
        string directory = Path.Combine(Path.GetTempPath(), "marknexia-parity-file-bound-" + Guid.NewGuid().ToString("N"));
        string path = Path.Combine(directory, "large.md");
        try
        {
            Directory.CreateDirectory(directory);
            using (FileStream stream = new(path, FileMode.CreateNew, FileAccess.Write, FileShare.None))
                stream.SetLength(FileService.LargeFileThresholdBytes + 1);
            var fileService = new FileService(new PathCanonicalizer());
            try
            {
                fileService.ReadTextAsync(path).GetAwaiter().GetResult();
                throw new InvalidOperationException("The file-size boundary case unexpectedly read an oversized document.");
            }
            catch (DocumentTooLargeException exception)
            {
                return Case("rendering", "markdown-input-limit", new JsonObject
                {
                    ["inputBytes"] = exception.SizeBytes,
                    ["maximumBytes"] = FileService.LargeFileThresholdBytes
                }, new JsonObject { ["root"] = "input-bound", ["files"] = new JsonArray("large.md") }, new JsonObject
                {
                    ["outcome"] = "rejected-before-read",
                    ["exception"] = exception.GetType().Name,
                    ["maximumBytes"] = exception.MaximumBytes
                }, sourceRevision);
            }
        }
        finally { TryDeleteDirectory(directory); }
    }

    private static async Task<IEnumerable<ParityCase>> BuildUpdateArchiveCasesAsync(string sourceRevision, CancellationToken cancellationToken)
    {
        var cases = new List<ParityCase>();
        byte[] validPayload = CreateZip(new Dictionary<string, byte[]>
        {
            ["Marknexia.App.exe"] = [1], ["Marknexia.App.dll"] = [2], ["Marknexia.App.pri"] = [3], ["marknexia-sbom.spdx.json"] = [4]
        });
        cases.Add(await StageArchiveCaseAsync("checksum-mismatch", validPayload, new string('0', 64), sourceRevision, cancellationToken));
        cases.Add(await StageArchiveCaseAsync("archive-traversal", CreateZip(new Dictionary<string, byte[]> { ["../escaped.txt"] = [1] }), null, sourceRevision, cancellationToken));
        cases.Add(await StageArchiveCaseAsync("missing-payload-files", CreateZip(new Dictionary<string, byte[]> { ["Marknexia.App.exe"] = [1] }), null, sourceRevision, cancellationToken));
        cases.Add(await StageArchiveCaseAsync("expansion-limits", CreateZipWithDeclaredLength(UpdateService.MaxDownloadBytes + 1), null, sourceRevision, cancellationToken));
        cases.Add(await StageArchiveCaseAsync("stale-stage-cleanup", CreateZip(new Dictionary<string, byte[]> { ["Marknexia.App.exe"] = [1] }), null, sourceRevision, cancellationToken, preExistingStaleStage: true));
        return cases;
    }

    private static async Task<ParityCase> StageArchiveCaseAsync(string name, byte[] archive, string? checksumOverride, string sourceRevision, CancellationToken cancellationToken, bool preExistingStaleStage = false)
    {
        string root = Path.Combine(Path.GetTempPath(), "marknexia-parity-update-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(root);
        string staleStage = Path.Combine(root, "pending-stale-fixture");
        if (preExistingStaleStage) Directory.CreateDirectory(staleStage);
        Uri packageUri = new("https://github.com/ramanacr/marknexia/releases/download/v1.4.0/Marknexia-v1.4.0-win-x64.zip");
        Uri checksumUri = new("https://github.com/ramanacr/marknexia/releases/download/v1.4.0/Marknexia-v1.4.0-win-x64.zip.sha256");
        string checksum = checksumOverride ?? Convert.ToHexString(SHA256.HashData(archive)).ToLowerInvariant();
        var responses = new Dictionary<Uri, byte[]> { [packageUri] = archive, [checksumUri] = Encoding.UTF8.GetBytes(checksum + "  package.zip\n") };
        var update = new UpdateCheckResult(true, "1.3.0", "1.4.0", new Uri("https://github.com/ramanacr/marknexia/releases/tag/v1.4.0"))
        {
            Assets = new[] { new UpdateAsset("Marknexia-v1.4.0-win-x64.zip", packageUri, archive.LongLength), new UpdateAsset("Marknexia-v1.4.0-win-x64.zip.sha256", checksumUri, checksum.Length + 14) }
        };
        try
        {
            using var client = new HttpClient(new FixtureHttpHandler(responses));
            try
            {
                await new UpdateService(client).DownloadAndStageAsync(update, root, cancellationToken);
                throw new UnexpectedStageSuccessException(name);
            }
            catch (Exception exception) when (exception is InvalidDataException or InvalidOperationException)
            {
                return Case("update-archives", name, new JsonObject
                {
                    ["archiveBytesBase64"] = Convert.ToBase64String(archive),
                    ["packageEntries"] = DescribeArchiveEntries(archive),
                    ["checksumMatches"] = checksumOverride is null,
                    ["checksumText"] = checksum + "  package.zip\\n",
                    ["packageBytes"] = archive.LongLength,
                    ["maximumExpandedBytes"] = UpdateService.MaxDownloadBytes
                    , ["preExistingStage"] = preExistingStaleStage ? "pending-stale-fixture" : null
                }, new JsonObject { ["root"] = "updates", ["files"] = new JsonArray() }, new JsonObject
                {
                    ["outcome"] = "rejected",
                    ["exception"] = exception.GetType().Name,
                    ["message"] = exception.Message,
                    ["pendingStageDirectories"] = Directory.EnumerateDirectories(root, "pending-*", SearchOption.TopDirectoryOnly).Count()
                    , ["preExistingStageStillExists"] = preExistingStaleStage && Directory.Exists(staleStage)
                }, sourceRevision);
            }
        }
        finally { TryDeleteDirectory(root); }
    }

    private static byte[] CreateZip(IReadOnlyDictionary<string, byte[]> files)
    {
        using var stream = new MemoryStream();
        using (var archive = new ZipArchive(stream, ZipArchiveMode.Create, leaveOpen: true))
            foreach ((string name, byte[] contents) in files)
            {
                ZipArchiveEntry entry = archive.CreateEntry(name, CompressionLevel.NoCompression);
                entry.LastWriteTime = new DateTimeOffset(2020, 1, 1, 0, 0, 0, TimeSpan.Zero);
                using Stream target = entry.Open();
                target.Write(contents);
            }
        return stream.ToArray();
    }

    private static byte[] CreateZipWithDeclaredLength(long length)
    {
        const string name = "oversized.bin";
        byte[] nameBytes = Encoding.UTF8.GetBytes(name);
        using var stream = new MemoryStream();
        using var writer = new BinaryWriter(stream, Encoding.UTF8, leaveOpen: true);
        writer.Write(0x04034b50u); writer.Write((ushort)20); writer.Write((ushort)0); writer.Write((ushort)0); writer.Write((ushort)0); writer.Write((ushort)0);
        writer.Write(0u); writer.Write(0u); writer.Write(0u); writer.Write((ushort)nameBytes.Length); writer.Write((ushort)0); writer.Write(nameBytes);
        long centralOffset = stream.Position;
        writer.Write(0x02014b50u); writer.Write((ushort)20); writer.Write((ushort)20); writer.Write((ushort)0); writer.Write((ushort)0); writer.Write((ushort)0); writer.Write((ushort)0);
        writer.Write(0u); writer.Write(0u); writer.Write(checked((uint)length)); writer.Write((ushort)nameBytes.Length); writer.Write((ushort)0); writer.Write((ushort)0); writer.Write((ushort)0); writer.Write((ushort)0); writer.Write(0u); writer.Write(0u); writer.Write(nameBytes);
        long centralLength = stream.Position - centralOffset;
        writer.Write(0x06054b50u); writer.Write((ushort)0); writer.Write((ushort)0); writer.Write((ushort)1); writer.Write((ushort)1); writer.Write(checked((uint)centralLength)); writer.Write(checked((uint)centralOffset)); writer.Write((ushort)0);
        return stream.ToArray();
    }

    private static JsonArray DescribeArchiveEntries(byte[] archive)
    {
        try
        {
            using var stream = new MemoryStream(archive);
            using var zip = new ZipArchive(stream, ZipArchiveMode.Read);
            return new JsonArray(zip.Entries.Select(entry => (JsonNode)new JsonObject
            {
                ["path"] = entry.FullName,
                ["declaredUncompressedBytes"] = entry.Length,
                ["contentBase64"] = entry.Length <= 1024 ? Convert.ToBase64String(ReadEntry(entry)) : null
            }).ToArray());
        }
        catch (InvalidDataException) { return new JsonArray(new JsonObject { ["path"] = "oversized.bin", ["declaredUncompressedBytes"] = UpdateService.MaxDownloadBytes + 1, ["contentBase64"] = null }); }
    }

    private static byte[] ReadEntry(ZipArchiveEntry entry)
    {
        using Stream source = entry.Open();
        using var buffer = new MemoryStream();
        source.CopyTo(buffer);
        return buffer.ToArray();
    }

    private sealed class FixtureHttpHandler(IReadOnlyDictionary<Uri, byte[]> responses) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            if (request.RequestUri is null || !responses.TryGetValue(request.RequestUri, out byte[]? content))
                return Task.FromResult(new HttpResponseMessage(HttpStatusCode.NotFound));
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK) { Content = new ByteArrayContent(content) });
        }
    }

    public sealed class UnexpectedStageSuccessException(string name) : Exception($"The '{name}' update archive fixture unexpectedly staged.");

    private sealed class RecordingFileService : IFileService
    {
        public List<string> CheckedPaths { get; } = new();
        public bool FileExists(string filePath) { CheckedPaths.Add(filePath); return false; }
        public bool DirectoryExists(string directoryPath) { CheckedPaths.Add(directoryPath); return false; }
        public string ComputeContentHash(string content) => throw new NotSupportedException();
        public Task<FileReadResult> ReadTextAsync(string filePath, CancellationToken cancellationToken = default) => throw new NotSupportedException();
    }

    private static ParityCase Case(string area, string name, JsonNode input, JsonNode? virtualFileSystem, JsonNode expected, string sourceRevision) =>
        new(area, name, input, virtualFileSystem, expected, sourceRevision);

    private static JsonObject ParsedExpected(ParsedMarkdown parsed) => new()
    {
        ["renderedBodyHtml"] = parsed.RenderedBodyHtml,
        ["headings"] = ToNode(parsed.Headings),
        ["customAnchors"] = ToNode(parsed.CustomAnchors),
        ["links"] = ToNode(parsed.ExtractedLinks),
        ["images"] = ToNode(parsed.ExtractedImages),
        ["diagrams"] = ToNode(parsed.DiagramBlocks)
    };

    private static JsonObject IntentExpected(NavigationIntent intent, string virtualRoot) => new()
    {
        ["kind"] = intent.Kind.ToString(),
        ["fragment"] = intent.Fragment,
        ["isSafe"] = intent.IsSafe,
        ["targetDocument"] = ToVirtualDocument(intent.TargetDocument, virtualRoot),
        ["externalUri"] = intent.ExternalUri?.ToString(),
        ["diagnostic"] = intent.Diagnostic
    };

    private static string? ToVirtualDocument(DocumentUri? document, string virtualRoot)
    {
        if (document is null) return null;
        string relative = Path.GetRelativePath(virtualRoot, document.CanonicalPath).Replace('\\', '/');
        if (relative.StartsWith("../", StringComparison.Ordinal) || relative == "..")
            throw new InvalidDataException("Navigation fixture target escaped its declared virtual root.");
        return string.IsNullOrEmpty(document.Fragment) ? relative : relative + "#" + document.Fragment;
    }

    private static JsonObject VirtualFileSystem(string root) => new()
    {
        ["root"] = "navigation",
        ["files"] = new JsonArray(Directory.EnumerateFiles(root, "*", SearchOption.AllDirectories)
            .OrderBy(path => Path.GetRelativePath(root, path), StringComparer.Ordinal)
            .Select(path => JsonValue.Create(Path.GetRelativePath(root, path).Replace('\\', '/'))).ToArray())
    };

    private static JsonNode? ToNode<T>(T value) => JsonNode.Parse(JsonSerializer.Serialize(value, JsonOptions));

    private static JsonObject ToEnvelope(ParityCase parityCase) => new()
    {
        ["schemaVersion"] = ParityCase.SchemaVersion,
        ["area"] = parityCase.Area,
        ["name"] = parityCase.Name,
        ["input"] = parityCase.Input,
        ["virtualFileSystem"] = parityCase.VirtualFileSystem,
        ["expected"] = parityCase.Expected,
        ["sourceRevision"] = parityCase.SourceRevision
    };

    private static string CaseName(string value) => value.Replace('/', '-').Replace(' ', '-').Replace(':', '-').Replace("%", "percent-", StringComparison.Ordinal)
        .Replace(".md", string.Empty, StringComparison.OrdinalIgnoreCase).ToLowerInvariant();

    private static void WriteCanonicalJson(string path, JsonNode node)
    {
        using var stream = new MemoryStream();
        using (var writer = new Utf8JsonWriter(stream, new JsonWriterOptions { Indented = true, Encoder = System.Text.Encodings.Web.JavaScriptEncoder.Default }))
        {
            WriteNode(writer, node);
        }
        string text = Encoding.UTF8.GetString(stream.ToArray()).Replace("\r\n", "\n", StringComparison.Ordinal) + "\n";
        File.WriteAllText(path, text, new UTF8Encoding(encoderShouldEmitUTF8Identifier: false));
    }

    private static void WriteNode(Utf8JsonWriter writer, JsonNode? node)
    {
        if (node is null) { writer.WriteNullValue(); return; }
        if (node is JsonObject obj)
        {
            writer.WriteStartObject();
            foreach ((string key, JsonNode? value) in obj.OrderBy(pair => pair.Key, StringComparer.Ordinal)) { writer.WritePropertyName(key); WriteNode(writer, value); }
            writer.WriteEndObject();
            return;
        }
        if (node is JsonArray array)
        {
            writer.WriteStartArray();
            foreach (JsonNode? value in array) WriteNode(writer, value);
            writer.WriteEndArray();
            return;
        }
        node.WriteTo(writer);
    }

    private static string ValidateRepositoryRoot(string repositoryRoot)
    {
        string fullRoot = Path.GetFullPath(repositoryRoot);
        if (!File.Exists(Path.Combine(fullRoot, "Marknexia.slnx")))
            throw new DirectoryNotFoundException("The parity exporter must be given the Marknexia repository root.");
        return fullRoot;
    }

    private static string ResolveSourceRevision(string repositoryRoot, string? frozenSourceRevision)
    {
        string revision = string.IsNullOrWhiteSpace(frozenSourceRevision)
            ? RunGit(repositoryRoot, "rev-parse HEAD")
            : RunGit(repositoryRoot, $"rev-parse {frozenSourceRevision}^{{commit}}");

        if (!string.IsNullOrWhiteSpace(frozenSourceRevision))
        {
            // A v1 replay is valid only while the files that define the oracle still
            // match its declared revision. Exporter/test changes do not affect this check.
            RunGitExpectSuccess(repositoryRoot, $"diff --quiet {revision} -- src test-fixtures Directory.Packages.props Directory.Build.props Directory.Build.targets global.json NuGet.config packages.lock.json");
        }
        else if (!string.IsNullOrWhiteSpace(RunGit(repositoryRoot, "status --porcelain --untracked-files=all -- src test-fixtures Directory.Packages.props Directory.Build.props Directory.Build.targets global.json NuGet.config packages.lock.json")))
        {
            throw new InvalidOperationException("Refusing to stamp a parity baseline from dirty oracle/build inputs. Commit or revert the documented allowlist first.");
        }

        return revision;
    }

    private static string RunGit(string repositoryRoot, string arguments)
    {
        using var process = Process.Start(new ProcessStartInfo("git", arguments)
        {
            WorkingDirectory = repositoryRoot,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false
        }) ?? throw new InvalidOperationException("Could not start git to resolve parity provenance.");
        if (!process.WaitForExit(5000) || process.ExitCode != 0)
            throw new InvalidOperationException($"Could not resolve parity provenance: {process.StandardError.ReadToEnd().Trim()}");
        return process.StandardOutput.ReadToEnd().Trim();
    }

    private static void RunGitExpectSuccess(string repositoryRoot, string arguments)
    {
        using var process = Process.Start(new ProcessStartInfo("git", arguments)
        {
            WorkingDirectory = repositoryRoot,
            RedirectStandardError = true,
            UseShellExecute = false
        }) ?? throw new InvalidOperationException("Could not start git to validate parity provenance.");
        if (!process.WaitForExit(5000) || process.ExitCode != 0)
            throw new InvalidOperationException("The behavioral oracle differs from the frozen parity revision; regenerate a new baseline instead of relabeling it.");
    }

    private static void ValidateOutputRoot(string repositoryRoot, string outputRoot)
    {
        if (PathsEqual(repositoryRoot, outputRoot) || IsWithin(repositoryRoot, outputRoot))
            throw new InvalidOperationException("The parity output directory must not be the repository root or an ancestor of it.");

        string fixturesRoot = Path.Combine(repositoryRoot, "compat", "fixtures");
        if (IsWithin(outputRoot, repositoryRoot) && !IsWithin(outputRoot, fixturesRoot) && !PathsEqual(outputRoot, fixturesRoot))
            throw new InvalidOperationException("Repository-local parity output must stay under compat/fixtures.");

        if (Directory.Exists(outputRoot) && Directory.EnumerateFileSystemEntries(outputRoot).Any() && !IsOwnedFixtureDirectory(outputRoot))
            throw new InvalidOperationException("Refusing to replace a non-fixture output directory.");
    }

    private static bool IsOwnedFixtureDirectory(string directory)
    {
        string manifest = Path.Combine(directory, "manifest.json");
        if (!File.Exists(manifest)) return false;
        try
        {
            using JsonDocument document = JsonDocument.Parse(File.ReadAllText(manifest));
            if (!document.RootElement.TryGetProperty("schemaVersion", out JsonElement version)
                || version.GetString() != ParityCase.SchemaVersion) return false;
            return Directory.EnumerateFiles(directory, "*", SearchOption.AllDirectories).All(file =>
                Path.GetFileName(file).Equals("manifest.json", StringComparison.Ordinal)
                || file.EndsWith(".case.json", StringComparison.Ordinal));
        }
        catch (JsonException) { return false; }
    }

    private static void ReplaceOwnedFixtureDirectory(string outputRoot, string temporaryRoot)
    {
        if (Directory.Exists(outputRoot))
        {
            if (Directory.EnumerateFileSystemEntries(outputRoot).Any() && !IsOwnedFixtureDirectory(outputRoot))
                throw new InvalidOperationException("Refusing to replace a directory that is not an owned parity fixture corpus.");
            Directory.Delete(outputRoot, recursive: true);
        }
        Directory.Move(temporaryRoot, outputRoot);
    }

    private static bool PathsEqual(string first, string second) =>
        Path.GetFullPath(first).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar)
            .Equals(Path.GetFullPath(second).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar), StringComparison.OrdinalIgnoreCase);

    private static bool IsWithin(string candidate, string root)
    {
        string normalizedCandidate = Path.GetFullPath(candidate).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
        string normalizedRoot = Path.GetFullPath(root).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
        return normalizedCandidate.StartsWith(normalizedRoot + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase);
    }

    private static void TryDeleteDirectory(string path)
    {
        try { if (Directory.Exists(path)) Directory.Delete(path, recursive: true); }
        catch { }
    }
}
