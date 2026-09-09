using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;
using Marknexia.Core;

namespace Marknexia.Files;

public sealed record LocalAssetResponse(
    int StatusCode,
    string ContentType,
    byte[] Content,
    string ReasonPhrase = "OK");

/// <summary>Reads local images for one immutable document authority.</summary>
public sealed class LocalAssetReader
{
    public const long MaxAssetBytes = 32 * 1024 * 1024;

    private static readonly IReadOnlyDictionary<string, string> ContentTypes =
        new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
        {
            [".apng"] = "image/apng",
            [".avif"] = "image/avif",
            [".bmp"] = "image/bmp",
            [".gif"] = "image/gif",
            [".ico"] = "image/x-icon",
            [".jpeg"] = "image/jpeg",
            [".jpg"] = "image/jpeg",
            [".png"] = "image/png",
            [".svg"] = "image/svg+xml",
            [".webp"] = "image/webp"
        };

    private readonly DocumentAssetContext _context;

    public LocalAssetReader(DocumentAssetContext context)
    {
        _context = context ?? throw new ArgumentNullException(nameof(context));
    }

    public async Task<LocalAssetResponse> ReadAsync(Uri requestUri, CancellationToken cancellationToken = default)
    {
        if (!IsAuthorizedUri(requestUri))
        {
            return Forbidden();
        }

        string escapedPath;
        try
        {
            escapedPath = requestUri.GetComponents(UriComponents.Path, UriFormat.UriEscaped);
        }
        catch (UriFormatException)
        {
            return Forbidden();
        }

        string decodedPath;
        try
        {
            // Decode exactly once. A literal %20 in a filename is therefore
            // represented by %2520 in the document URL and stays literal.
            decodedPath = Uri.UnescapeDataString(escapedPath);
        }
        catch (UriFormatException)
        {
            return Forbidden();
        }

        if (!IsSafeUrlPath(decodedPath))
        {
            return Forbidden();
        }

        string relativePath = decodedPath.TrimStart('/');
        if (relativePath.Length == 0)
        {
            return NotFound();
        }

        string extension = Path.GetExtension(relativePath);
        if (!ContentTypes.TryGetValue(extension, out string? contentType))
        {
            return new LocalAssetResponse(415, "text/plain", [], "Unsupported Media Type");
        }

        string candidatePath;
        try
        {
            candidatePath = Path.GetFullPath(Path.Combine(
                _context.RootPath,
                relativePath.Replace('/', Path.DirectorySeparatorChar)));
        }
        catch (Exception ex) when (ex is ArgumentException or NotSupportedException or PathTooLongException)
        {
            return Forbidden();
        }

        if (!IsLexicallyWithinRoot(candidatePath, _context.RootPath)
            || IsUncPath(candidatePath)
            || HasReparsePointInPath(_context.RootPath, candidatePath))
        {
            return Forbidden();
        }

        cancellationToken.ThrowIfCancellationRequested();
        if (!File.Exists(candidatePath))
        {
            return NotFound();
        }

        try
        {
            await using var stream = new FileStream(
                candidatePath,
                FileMode.Open,
                FileAccess.Read,
                FileShare.Read,
                bufferSize: 64 * 1024,
                options: FileOptions.Asynchronous | FileOptions.SequentialScan);

            if (stream.Length > MaxAssetBytes)
            {
                return new LocalAssetResponse(413, "text/plain", [], "Payload Too Large");
            }

            if (OperatingSystem.IsWindows()
                && !IsFinalPathWithinRoot(stream.SafeFileHandle, _context.RootPath))
            {
                return Forbidden();
            }

            byte[] bytes = new byte[checked((int)stream.Length)];
            await stream.ReadExactlyAsync(bytes, cancellationToken).ConfigureAwait(false);
            return new LocalAssetResponse(200, contentType, bytes);
        }
        catch (OperationCanceledException)
        {
            throw;
        }
        catch (UnauthorizedAccessException)
        {
            return Forbidden();
        }
        catch (FileNotFoundException)
        {
            return NotFound();
        }
        catch (DirectoryNotFoundException)
        {
            return NotFound();
        }
        catch (IOException)
        {
            // Do not expose filesystem details through the WebView response.
            return new LocalAssetResponse(404, "text/plain", [], "Not Found");
        }
    }

    private bool IsAuthorizedUri(Uri? requestUri)
    {
        if (requestUri is null || !requestUri.IsAbsoluteUri
            || !string.Equals(requestUri.Scheme, _context.BaseUri.Scheme, StringComparison.OrdinalIgnoreCase)
            || !string.Equals(requestUri.Host, _context.BaseUri.Host, StringComparison.OrdinalIgnoreCase)
            || requestUri.Port != _context.BaseUri.Port
            || !string.IsNullOrEmpty(requestUri.UserInfo))
        {
            return false;
        }

        return true;
    }

    private static bool IsSafeUrlPath(string path)
    {
        if (path.IndexOfAny(['\\', '\0']) >= 0)
        {
            return false;
        }

        return path.All(character => !char.IsControl(character));
    }

    private static bool IsLexicallyWithinRoot(string candidatePath, string rootPath)
    {
        string candidate = Path.TrimEndingDirectorySeparator(Path.GetFullPath(candidatePath));
        string root = Path.TrimEndingDirectorySeparator(Path.GetFullPath(rootPath));

        return candidate.Equals(root, StringComparison.OrdinalIgnoreCase)
            || candidate.StartsWith(root + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase);
    }

    private static bool HasReparsePointInPath(string rootPath, string candidatePath)
    {
        string? volumeRoot = Path.GetPathRoot(candidatePath);
        if (string.IsNullOrEmpty(volumeRoot))
        {
            return true;
        }

        string normalizedRoot = Path.TrimEndingDirectorySeparator(Path.GetFullPath(rootPath));
        string normalizedCandidate = Path.GetFullPath(candidatePath);
        if (!IsLexicallyWithinRoot(normalizedCandidate, normalizedRoot))
        {
            return true;
        }

        string current = Path.TrimEndingDirectorySeparator(volumeRoot);
        string remainder = normalizedCandidate[volumeRoot.Length..];
        foreach (string component in remainder.Split([Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar], StringSplitOptions.RemoveEmptyEntries))
        {
            current = Path.Combine(current, component);
            if (!File.Exists(current) && !Directory.Exists(current))
            {
                return false;
            }

            try
            {
                if ((File.GetAttributes(current) & FileAttributes.ReparsePoint) != 0)
                {
                    return true;
                }
            }
            catch (IOException)
            {
                return true;
            }
            catch (UnauthorizedAccessException)
            {
                return true;
            }
        }

        return false;
    }

    private static bool IsUncPath(string path) => path.StartsWith("\\\\", StringComparison.Ordinal);

    private static bool IsFinalPathWithinRoot(SafeFileHandle handle, string rootPath)
    {
        string? finalPath = GetFinalPath(handle);
        if (string.IsNullOrEmpty(finalPath))
        {
            return false;
        }

        string candidate = NormalizeFinalPath(finalPath);
        if (IsUncPath(candidate))
        {
            return false;
        }

        using SafeFileHandle rootHandle = OpenDirectoryHandle(Path.GetFullPath(rootPath));
        if (rootHandle.IsInvalid)
        {
            return false;
        }

        string? finalRootPath = GetFinalPath(rootHandle);
        if (string.IsNullOrEmpty(finalRootPath))
        {
            return false;
        }

        string root = NormalizeFinalPath(finalRootPath);
        if (IsUncPath(root))
        {
            return false;
        }

        return candidate.Equals(root, StringComparison.OrdinalIgnoreCase)
            || candidate.StartsWith(root + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase);
    }

    private static string? GetFinalPath(SafeFileHandle handle)
    {
        var capacity = 512;
        while (capacity <= 32 * 1024)
        {
            var buffer = new StringBuilder(capacity);
            uint length = GetFinalPathNameByHandle(handle, buffer, buffer.Capacity, 0);
            if (length == 0)
            {
                return null;
            }

            if (length < buffer.Capacity)
            {
                return NormalizeFinalPath(buffer.ToString());
            }

            capacity *= 2;
        }

        return null;
    }

    private static string NormalizeFinalPath(string path)
    {
        if (path.StartsWith("\\\\?\\UNC\\", StringComparison.OrdinalIgnoreCase))
        {
            return "\\\\" + path[8..];
        }

        return path.StartsWith("\\\\?\\", StringComparison.Ordinal) ? path[4..] : path;
    }

    private static LocalAssetResponse Forbidden() => new(403, "text/plain", [], "Forbidden");
    private static LocalAssetResponse NotFound() => new(404, "text/plain", [], "Not Found");

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint GetFinalPathNameByHandle(
        SafeFileHandle hFile,
        StringBuilder lpszFilePath,
        int cchFilePath,
        uint dwFlags);

    private static SafeFileHandle OpenDirectoryHandle(string path) =>
        CreateFile(
            path,
            desiredAccess: 0,
            shareMode: 0x00000001 | 0x00000002 | 0x00000004,
            securityAttributes: IntPtr.Zero,
            creationDisposition: 3,
            flagsAndAttributes: 0x02000000,
            templateFile: IntPtr.Zero);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFile(
        string fileName,
        uint desiredAccess,
        uint shareMode,
        IntPtr securityAttributes,
        uint creationDisposition,
        uint flagsAndAttributes,
        IntPtr templateFile);
}
