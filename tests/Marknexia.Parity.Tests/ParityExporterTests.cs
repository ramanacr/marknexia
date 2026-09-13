using FluentAssertions;
using Marknexia.ParityExporter;
using Xunit;
using Exporter = Marknexia.ParityExporter.ParityExporter;

namespace Marknexia.Parity.Tests;

public sealed class ParityExporterTests
{
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
}
