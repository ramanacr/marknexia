using FluentAssertions;
using Marknexia.Core;
using Marknexia.Files;
using Xunit;

namespace Marknexia.Files.Tests;

public sealed class FileServiceTests
{
    [Fact]
    public async Task ReadTextAsync_RejectsOversizedDocumentsBeforeReadingThem()
    {
        string directory = Path.Combine(Path.GetTempPath(), $"marknexia-large-file-{Guid.NewGuid():N}");
        string path = Path.Combine(directory, "large.md");
        Directory.CreateDirectory(directory);

        try
        {
            await using (var stream = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.Read))
            {
                stream.SetLength(FileService.LargeFileThresholdBytes + 1);
            }

            var service = new FileService(new PathCanonicalizer());
            Func<Task> action = () => service.ReadTextAsync(path);

            var assertion = await action.Should().ThrowAsync<DocumentTooLargeException>();
            assertion.Which.FilePath.Should().Be(Path.GetFullPath(path));
            assertion.Which.SizeBytes.Should().Be(FileService.LargeFileThresholdBytes + 1);
            assertion.Which.MaximumBytes.Should().Be(FileService.LargeFileThresholdBytes);
        }
        finally
        {
            if (Directory.Exists(directory)) Directory.Delete(directory, recursive: true);
        }
    }
}
