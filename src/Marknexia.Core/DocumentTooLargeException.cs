namespace Marknexia.Core;

public sealed class DocumentTooLargeException : IOException
{
    public string FilePath { get; }
    public long SizeBytes { get; }
    public long MaximumBytes { get; }

    public DocumentTooLargeException(string filePath, long sizeBytes, long maximumBytes)
        : base($"Document '{filePath}' is {sizeBytes:N0} bytes; the maximum supported size is {maximumBytes:N0} bytes.")
    {
        FilePath = filePath;
        SizeBytes = sizeBytes;
        MaximumBytes = maximumBytes;
    }
}
