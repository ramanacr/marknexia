using System.Security.Cryptography;
using System.Text;
using Marknexia.Core;

namespace Marknexia.Files;

public sealed class FileService : IFileService
{
    private readonly IPathCanonicalizer _pathCanonicalizer;
    public const long LargeFileThresholdBytes = 50 * 1024 * 1024; // 50 MB

    public FileService(IPathCanonicalizer pathCanonicalizer)
    {
        _pathCanonicalizer = pathCanonicalizer ?? throw new ArgumentNullException(nameof(pathCanonicalizer));
    }

    public async Task<FileReadResult> ReadTextAsync(string filePath, CancellationToken cancellationToken = default)
    {
        string canonicalPath = _pathCanonicalizer.CanonicalizePath(filePath);

        if (!File.Exists(canonicalPath))
        {
            throw new FileNotFoundException($"Document file not found: {canonicalPath}", canonicalPath);
        }

        await using var stream = new FileStream(
            canonicalPath,
            FileMode.Open,
            FileAccess.Read,
            FileShare.ReadWrite | FileShare.Delete,
            bufferSize: 64 * 1024,
            options: FileOptions.Asynchronous | FileOptions.SequentialScan);

        long size = stream.Length;
        if (size > LargeFileThresholdBytes)
        {
            throw new DocumentTooLargeException(canonicalPath, size, LargeFileThresholdBytes);
        }

        byte[] rawBytes = new byte[checked((int)size)];
        await stream.ReadExactlyAsync(rawBytes, cancellationToken).ConfigureAwait(false);
        var (content, encodingName) = DecodeBytes(rawBytes);
        string hash = ComputeContentHash(content);

        return new FileReadResult(content, encodingName, hash, size);
    }

    public bool FileExists(string filePath)
    {
        if (string.IsNullOrWhiteSpace(filePath)) return false;
        string canonical = _pathCanonicalizer.CanonicalizePath(filePath);
        return File.Exists(canonical);
    }

    public bool DirectoryExists(string directoryPath)
    {
        if (string.IsNullOrWhiteSpace(directoryPath)) return false;
        string canonical = _pathCanonicalizer.CanonicalizePath(directoryPath);
        return Directory.Exists(canonical);
    }

    public string ComputeContentHash(string content)
    {
        byte[] hashBytes = SHA256.HashData(Encoding.UTF8.GetBytes(content));
        return Convert.ToHexString(hashBytes).ToLowerInvariant();
    }

    private static (string Content, string EncodingName) DecodeBytes(byte[] bytes)
    {
        if (bytes.Length == 0)
        {
            return (string.Empty, "UTF-8");
        }

        // Check BOM
        if (bytes.Length >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF)
        {
            return (Encoding.UTF8.GetString(bytes, 3, bytes.Length - 3), "UTF-8 with BOM");
        }
        if (bytes.Length >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE)
        {
            return (Encoding.Unicode.GetString(bytes, 2, bytes.Length - 2), "UTF-16 LE");
        }
        if (bytes.Length >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF)
        {
            return (Encoding.BigEndianUnicode.GetString(bytes, 2, bytes.Length - 2), "UTF-16 BE");
        }
        if (bytes.Length >= 4 && bytes[0] == 0 && bytes[1] == 0 && bytes[2] == 0xFE && bytes[3] == 0xFF)
        {
            return (Encoding.UTF32.GetString(bytes, 4, bytes.Length - 4), "UTF-32 BE");
        }
        if (bytes.Length >= 4 && bytes[0] == 0xFF && bytes[1] == 0xFE && bytes[2] == 0 && bytes[3] == 0)
        {
            return (Encoding.UTF32.GetString(bytes, 4, bytes.Length - 4), "UTF-32 LE");
        }

        // Try strict UTF-8
        try
        {
            var strictUtf8 = new UTF8Encoding(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true);
            return (strictUtf8.GetString(bytes), "UTF-8");
        }
        catch (DecoderFallbackException)
        {
            // Fallback to Latin-1
            return (Encoding.Latin1.GetString(bytes), "Latin1/ISO-8859-1");
        }
    }
}
