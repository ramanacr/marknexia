using FluentAssertions;
using System.Text.Json;
using Marknexia.ParityExporter;
using Xunit;
using Exporter = Marknexia.ParityExporter.ParityExporter;

namespace Marknexia.Parity.Tests;

public sealed class ParityExporterTests
{
    [Fact]
    public void NormalizeHtml_PreservesUserTextUrlsAndIndependentNonceValues()
    {
        const string html = "<p>document-0123456789abcdef0123456789abcdef.marknexia.viewer nonce-abc</p><a href=\"https://document-0123456789abcdef0123456789abcdef.marknexia.viewer/nonce-abc\">link</a><script nonce=\"one\"></script><script nonce=\"two\"></script>";

        ParityNormalizer.NormalizeHtml(html).Should().Be(html);
    }

    [Fact]
    public void NormalizeHtml_CompleteTemplatePreservesTemplateShapedUserContent()
    {
        const string userHost = "document-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.marknexia.viewer";
        string html = $"<!DOCTYPE html><html><head><base href=\"https://document-11111111111111111111111111111111.marknexia.viewer/compat/\"><meta http-equiv=\"Content-Security-Policy\" content=\"base-uri https://document-11111111111111111111111111111111.marknexia.viewer; script-src &#39;nonce-real&#39;\"></head><body><p>{userHost} nonce-real</p><a href=\"https://{userHost}/nonce-real\">user</a><script nonce=\"user-nonce\"></script><script nonce=\"real\">\nwindow.marknexiaBridge =</script></body></html>";

        string normalized = ParityNormalizer.NormalizeHtml(html);

        normalized.Should().Contain(userHost).And.Contain("https://" + userHost + "/nonce-real").And.Contain("nonce=\"user-nonce\"");
    }

    [Fact]
    public async Task VerifyBaseline_DetectsByteCorruptionAndPreservesBaseline()
    {
        string root = ParityTestSupport.FindRepositoryRoot();
        string baseline = Path.Combine(root, "compat", "fixtures", "v1");
        string manifest = Path.Combine(baseline, "manifest.json");
        byte[] original = await File.ReadAllBytesAsync(manifest);
        string copy = ParityTestSupport.CreateTempDirectory();
        try
        {
            ParityTestSupport.CopyDirectory(baseline, copy);
            await File.WriteAllTextAsync(Path.Combine(copy, "manifest.json"), "{\"corrupted\":true}\n");

            await FluentActions.Invoking(() => ParityBaselineVerifier.VerifyAsync(root, copy, CancellationToken.None))
                .Should().ThrowAsync<InvalidDataException>();
            (await File.ReadAllBytesAsync(manifest)).Should().Equal(original);
        }
        finally
        {
            ParityTestSupport.DeleteTempDirectory(copy);
        }
    }

    [Fact]
    public async Task VerifyBaseline_DetectsSchemaValidExpectedValueMutationAndPreservesBaseline()
    {
        string root = ParityTestSupport.FindRepositoryRoot();
        string baseline = Path.Combine(root, "compat", "fixtures", "v1");
        string source = Path.Combine(baseline, "rendering", "rendered-output-limit.case.json");
        byte[] original = await File.ReadAllBytesAsync(source);
        string copy = ParityTestSupport.CreateTempDirectory();
        try
        {
            ParityTestSupport.CopyDirectory(baseline, copy);
            string target = Path.Combine(copy, "rendering", "rendered-output-limit.case.json");
            await File.WriteAllTextAsync(target, (await File.ReadAllTextAsync(target)).Replace("\"maximumBytes\": 1", "\"maximumBytes\": 2", StringComparison.Ordinal));

            await FluentActions.Invoking(() => ParityBaselineVerifier.VerifyAsync(root, copy, CancellationToken.None))
                .Should().ThrowAsync<InvalidDataException>();
            (await File.ReadAllBytesAsync(source)).Should().Equal(original);
        }
        finally { ParityTestSupport.DeleteTempDirectory(copy); }
    }

    [Fact]
    public async Task ExportTwice_ProducesByteIdenticalFixtures()
    {
        string first = ParityTestSupport.CreateTempDirectory();
        string second = ParityTestSupport.CreateTempDirectory();
        var exporter = new Exporter(ParityTestSupport.FindRepositoryRoot());

        try
        {
            await exporter.ExportAsync(first, CancellationToken.None);
            await exporter.ExportAsync(second, CancellationToken.None);

            ParityTestSupport.DirectoryDigest(first)
                .Should().Be(ParityTestSupport.DirectoryDigest(second));
            ParityTestSupport.ValidateEveryCaseAgainstSchema(first).Should().BeTrue();
        }
        finally
        {
            ParityTestSupport.DeleteTempDirectory(first);
            ParityTestSupport.DeleteTempDirectory(second);
        }
    }

    [Fact]
    public async Task ExportedCorpus_EqualsCommittedBaselineAndContainsEveryRequiredArea()
    {
        string root = ParityTestSupport.FindRepositoryRoot();
        string exported = ParityTestSupport.CreateTempDirectory();
        try
        {
            string frozenRevision = ParityBaselineVerifier.GetFrozenRevision(Path.Combine(root, "compat", "fixtures", "v1"));
            await new Exporter(root, frozenRevision).ExportAsync(exported, CancellationToken.None);

            ParityTestSupport.ValidateEveryCaseAgainstSchema(exported).Should().BeTrue();
            ParityTestSupport.ReadManifestCaseCount(exported).Should().BeGreaterThan(0);
            ParityTestSupport.DirectoryDigest(exported).Should().Be(ParityTestSupport.DirectoryDigest(Path.Combine(root, "compat", "fixtures", "v1")));
            ParityTestSupport.AreaNames(exported).Should().BeEquivalentTo(["markdown", "headings", "navigation", "sanitizer", "rendering", "settings", "update-archives"]);
        }
        finally
        {
            ParityTestSupport.DeleteTempDirectory(exported);
        }
    }

    [Fact]
    public async Task Export_RejectsRepositoryRootWithoutDeletingIt()
    {
        string root = ParityTestSupport.FindRepositoryRoot();
        string marker = Path.Combine(root, "Marknexia.slnx");
        byte[] original = await File.ReadAllBytesAsync(marker);

        await FluentActions.Invoking(() => new Exporter(root).ExportAsync(root, CancellationToken.None))
            .Should().ThrowAsync<InvalidOperationException>();

        (await File.ReadAllBytesAsync(marker)).Should().Equal(original);
    }

    [Fact]
    public void FixtureValidation_CanRunTwiceInOneProcess()
    {
        string fixtures = Path.Combine(ParityTestSupport.FindRepositoryRoot(), "compat", "fixtures", "v1");

        ParityTestSupport.ValidateEveryCaseAgainstSchema(fixtures).Should().BeTrue();
        ParityTestSupport.ValidateEveryCaseAgainstSchema(fixtures).Should().BeTrue();
    }

    [Fact]
    public async Task VerifyBaseline_ReplaysAndValidatesTwiceInOneProcess()
    {
        string root = ParityTestSupport.FindRepositoryRoot();
        string fixtures = Path.Combine(root, "compat", "fixtures", "v1");
        await FluentActions.Invoking(() => ParityBaselineVerifier.VerifyAsync(root, fixtures, CancellationToken.None))
            .Should().NotThrowAsync();
        await FluentActions.Invoking(() => ParityBaselineVerifier.VerifyAsync(root, fixtures, CancellationToken.None))
            .Should().NotThrowAsync();
    }

    [Fact]
    public void ArchiveFixtures_RecordLiteralSecurityOutcomes()
    {
        string root = Path.Combine(ParityTestSupport.FindRepositoryRoot(), "compat", "fixtures", "v1", "update-archives");
        using JsonDocument traversal = JsonDocument.Parse(File.ReadAllText(Path.Combine(root, "archive-traversal.case.json")));
        using JsonDocument expansion = JsonDocument.Parse(File.ReadAllText(Path.Combine(root, "expansion-limits.case.json")));

        traversal.RootElement.GetProperty("expected").GetProperty("message").GetString().Should().Be("The update archive contains a path traversal entry.");
        expansion.RootElement.GetProperty("input").GetProperty("packageEntries")[0].GetProperty("declaredUncompressedBytes").GetInt64().Should().Be(1_000_000_001L);
    }

    [Fact]
    public void ArchiveFixture_UnexpectedSuccessExceptionIsNotAnExpectedRejectionType()
    {
        var exception = new Exporter.UnexpectedStageSuccessException("valid-package");

        exception.Should().NotBeAssignableTo<InvalidDataException>().And.NotBeAssignableTo<InvalidOperationException>();
    }

    [Fact]
    public async Task Cli_ReturnsNonzeroAndWritesConciseErrorInsteadOfThrowing()
    {
        using var error = new StringWriter();

        int result = await ParityExporterCli.RunAsync(["--unknown-option"], TextWriter.Null, error);

        result.Should().Be(1);
        error.ToString().Should().Contain("Parity exporter failed:").And.NotContain("at Marknexia");
    }
}
