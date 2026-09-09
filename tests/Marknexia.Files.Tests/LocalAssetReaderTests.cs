using Marknexia.Core;
using Xunit;

namespace Marknexia.Files.Tests;

public sealed class LocalAssetReaderTests : IDisposable
{
    private readonly string _fixture = Path.Combine(Path.GetTempPath(), "marknexia-assets-" + Guid.NewGuid().ToString("N"));
    private readonly DocumentAssetContext _context;

    public LocalAssetReaderTests()
    {
        string root = Path.Combine(_fixture, "repository");
        Directory.CreateDirectory(Path.Combine(root, "docs", "nested"));
        Directory.CreateDirectory(Path.Combine(root, "images"));
        _context = DocumentAssetContext.Create(Path.Combine(root, "docs", "nested", "README.md"), root);
    }

    [Theory]
    [InlineData("image.png", "docs/nested/image.png")]
    [InlineData("../image.png", "docs/image.png")]
    [InlineData("/images/root.png", "images/root.png")]
    [InlineData("image%20name.png", "docs/nested/image name.png")]
    [InlineData("literal%2520.png", "docs/nested/literal%20.png")]
    public async Task Reads_only_the_requested_image_with_exact_once_URL_decoding(string url, string relativePath)
    {
        byte[] expected = [137, 80, 78, 71, 13, 10, 26, 10];
        await File.WriteAllBytesAsync(Path.Combine(_context.RootPath, relativePath), expected);
        var result = await new LocalAssetReader(_context).ReadAsync(new Uri(_context.BaseUri, url));

        Assert.Equal(200, result.StatusCode);
        Assert.Equal("image/png", result.ContentType);
        Assert.Equal(expected, result.Content);
    }

    [Theory]
    [InlineData("https://elsewhere.example/image.png")]
    [InlineData("http://document.example/image.png")]
    public async Task Rejects_requests_outside_the_document_origin(string url)
    {
        var result = await new LocalAssetReader(_context).ReadAsync(new Uri(url));

        Assert.Equal(403, result.StatusCode);
        Assert.Empty(result.Content);
    }

    [Theory]
    [InlineData("missing.png", 404)]
    [InlineData("notes.txt", 415)]
    [InlineData("../outside.exe", 415)]
    public async Task Does_not_serve_missing_unsupported_or_non_image_paths(string url, int statusCode)
    {
        var result = await new LocalAssetReader(_context).ReadAsync(new Uri(_context.BaseUri, url));

        Assert.Equal(statusCode, result.StatusCode);
        Assert.Empty(result.Content);
    }

    [Fact]
    public async Task Rejects_a_file_that_escapes_the_authorized_root()
    {
        string outside = Path.Combine(_fixture, "outside.png");
        await File.WriteAllBytesAsync(outside, [1, 2, 3]);

        var result = await new LocalAssetReader(_context).ReadAsync(new Uri(_context.BaseUri, "%2e%2e/%2e%2e/%2e%2e/outside.png"));

        Assert.Equal(404, result.StatusCode);
        Assert.Empty(result.Content);
    }

    [Fact]
    public async Task Standalone_documents_can_read_a_sibling_asset_through_one_parent_segment()
    {
        string standaloneRoot = Path.Combine(_fixture, "standalone");
        string source = Path.Combine(standaloneRoot, "docs", "README.md");
        Directory.CreateDirectory(Path.Combine(standaloneRoot, "docs"));
        Directory.CreateDirectory(Path.Combine(standaloneRoot, "images"));
        byte[] expected = [137, 80, 78, 71, 13, 10, 26, 10];
        await File.WriteAllBytesAsync(Path.Combine(standaloneRoot, "images", "logo.png"), expected);

        var context = DocumentAssetContext.Create(source, repositoryRoot: null);
        var result = await new LocalAssetReader(context).ReadAsync(new Uri(context.BaseUri, "../images/logo.png"));

        Assert.Equal(200, result.StatusCode);
        Assert.Equal("image/png", result.ContentType);
        Assert.Equal(expected, result.Content);
    }

    public void Dispose() => Directory.Delete(_fixture, recursive: true);
}
