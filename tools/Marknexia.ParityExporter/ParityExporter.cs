using System.Diagnostics;
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

public sealed class ParityExporter(string repositoryRoot)
{
    private static readonly JsonSerializerOptions JsonOptions = new() { WriteIndented = true };
    private readonly string _repositoryRoot = Path.GetFullPath(repositoryRoot);

    public Task ExportAsync(string outputRoot, CancellationToken cancellationToken) =>
        ExportAsync(_repositoryRoot, outputRoot, cancellationToken);

    public async Task ExportAsync(string repositoryRoot, string outputRoot, CancellationToken cancellationToken)
    {
        string sourceRevision = GetSourceRevision(repositoryRoot);
        string fullOutputRoot = Path.GetFullPath(outputRoot);
        if (Directory.Exists(fullOutputRoot)) Directory.Delete(fullOutputRoot, recursive: true);
        Directory.CreateDirectory(fullOutputRoot);

        List<ParityCase> cases = await BuildCasesAsync(sourceRevision, cancellationToken);
        foreach (ParityCase parityCase in cases.OrderBy(item => item.Area, StringComparer.Ordinal).ThenBy(item => item.Name, StringComparer.Ordinal))
        {
            string directory = Path.Combine(fullOutputRoot, parityCase.Area);
            Directory.CreateDirectory(directory);
            string path = Path.Combine(directory, parityCase.Name + ".case.json");
            WriteCanonicalJson(path, ToEnvelope(parityCase));
        }

        WriteCanonicalJson(Path.Combine(fullOutputRoot, "manifest.json"), new JsonObject
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
            cases.Add(Case("navigation", name, new JsonObject { ["destination"] = destination }, VirtualFileSystem(navigationRoot), IntentExpected(intent), sourceRevision));
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
                new JsonObject { ["sanitizedHtml"] = ParityNormalizer.NormalizeHtml(result) }, sourceRevision));
        }

        var renderer = new MarkdownRenderer();
        foreach ((string name, string markdown, bool remote, int maxDiagrams, long maxSource) in new[]
        {
            ("remote-images-off", "![remote](https://example.test/image.png)", false, 8, 1_000L),
            ("remote-images-on", "![remote](https://example.test/image.png)", true, 8, 1_000L),
            ("code-copy-metadata", "```c#\nvar value = 1;\n```", false, 8, 1_000L),
            ("diagram-limit", "```mermaid\ngraph TD\nA-->B\n```\n```mermaid\ngraph TD\nB-->C\n```", false, 1, 1_000L),
            ("diagram-source-limit", "```mermaid\ngraph TD\nA-->B\n```", false, 8, 1L)
        })
        {
            var boundedRenderer = new MarkdownRenderer(maxDiagramCount: maxDiagrams, maxDiagramSourceBytes: maxSource);
            RenderedDocument document = await boundedRenderer.RenderAsync(markdown,
                new RenderContext(Path.Combine(_repositoryRoot, "compat", "virtual.md"), _repositoryRoot, AppTheme.System, AllowRemoteAssets: remote), cancellationToken);
            cases.Add(Case("rendering", name, new JsonObject { ["markdown"] = markdown, ["allowRemoteAssets"] = remote }, null,
                new JsonObject { ["html"] = ParityNormalizer.NormalizeHtml(document.HtmlContent), ["assets"] = ToNode(document.AssetReferences) }, sourceRevision));
        }

        cases.Add(SettingsCase("malformed-settings", "{ not valid json", sourceRevision));
        cases.Add(SettingsCase("recent-item-bounds", null, sourceRevision));

        foreach ((string name, string json) in new[]
        {
            ("checksum-mismatch", "{\"tag_name\":\"v1.4.0\",\"html_url\":\"https://github.com/ramanacr/marknexia/releases/tag/v1.4.0\",\"draft\":false,\"prerelease\":false,\"assets\":[{\"name\":\"Marknexia-v1.4.0-win-x64.zip\",\"browser_download_url\":\"https://github.com/ramanacr/marknexia/releases/download/v1.4.0/Marknexia-v1.4.0-win-x64.zip\",\"size\":123}]}"),
            ("missing-payload-files", "{\"tag_name\":\"v1.4.0\",\"html_url\":\"https://evil.example/release\",\"draft\":false,\"prerelease\":false}"),
            ("archive-traversal", "{\"tag_name\":\"v1.4.0\",\"html_url\":\"https://github.com/ramanacr/marknexia/releases/tag/v1.4.0\",\"draft\":false,\"prerelease\":true}"),
            ("expansion-limits", "{\"tag_name\":\"v999999999999999999999\",\"html_url\":\"https://github.com/ramanacr/marknexia/releases/tag/v999999999999999999999\",\"draft\":false,\"prerelease\":false}"),
            ("stale-stage-cleanup", "{\"tag_name\":\"v1.4.0\",\"html_url\":\"https://github.com/ramanacr/marknexia/releases/tag/v1.4.0\",\"draft\":true,\"prerelease\":false}")
        })
        {
            UpdateCheckResult update = UpdateService.ParseLatestReleaseJson(json, "1.3.0");
            cases.Add(Case("update-archives", name, new JsonObject { ["releaseJson"] = json }, null,
                new JsonObject { ["isUpdateAvailable"] = update.IsUpdateAvailable, ["latestVersion"] = update.LatestVersion, ["error"] = update.Error, ["assetCount"] = update.Assets.Count }, sourceRevision));
        }

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
            if (malformedJson is null)
            {
                for (int index = 0; index < 20; index++) service.AddRecentFile($"C:\\docs\\{index}.md");
                service.AddRecentFile("C:\\docs\\19.md");
            }
            return Case("settings", name, new JsonObject { ["settingsJson"] = malformedJson }, null, new JsonObject
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

    private static ParityCase Case(string area, string name, JsonNode input, JsonNode? virtualFileSystem, JsonNode expected, string sourceRevision) =>
        new(area, name, input, virtualFileSystem, expected, sourceRevision);

    private static JsonObject ParsedExpected(ParsedMarkdown parsed) => new()
    {
        ["renderedBodyHtml"] = ParityNormalizer.NormalizeHtml(parsed.RenderedBodyHtml),
        ["headings"] = ToNode(parsed.Headings),
        ["customAnchors"] = ToNode(parsed.CustomAnchors),
        ["links"] = ToNode(parsed.ExtractedLinks),
        ["images"] = ToNode(parsed.ExtractedImages),
        ["diagrams"] = ToNode(parsed.DiagramBlocks)
    };

    private static JsonObject IntentExpected(NavigationIntent intent) => new()
    {
        ["kind"] = intent.Kind.ToString(),
        ["fragment"] = intent.Fragment,
        ["isSafe"] = intent.IsSafe,
        ["targetDocument"] = intent.TargetDocument?.ToString(),
        ["externalUri"] = intent.ExternalUri?.ToString(),
        ["diagnostic"] = intent.Diagnostic
    };

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

    private static string CaseName(string value) => value.Replace('/', '-').Replace(' ', '-').Replace("%", "percent-", StringComparison.Ordinal)
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

    private static string GetSourceRevision(string repositoryRoot)
    {
        try
        {
            using var process = Process.Start(new ProcessStartInfo("git", "rev-parse HEAD") { WorkingDirectory = repositoryRoot, RedirectStandardOutput = true, UseShellExecute = false });
            return process is not null && process.WaitForExit(5000) && process.ExitCode == 0 ? process.StandardOutput.ReadToEnd().Trim() : "unavailable";
        }
        catch { return "unavailable"; }
    }
}
