using System.Net;

namespace Marknexia.Files;

/// <summary>Downloads explicitly authorized remote images for the WebView broker.</summary>
public sealed class RemoteImageReader
{
    public const long MaxRemoteImageBytes = 32 * 1024 * 1024;
    private readonly HttpClient _client;

    public RemoteImageReader(HttpClient client)
    {
        _client = client ?? throw new ArgumentNullException(nameof(client));
    }

    public async Task<LocalAssetResponse> ReadAsync(Uri requestUri, CancellationToken cancellationToken = default)
    {
        if (requestUri is null || !requestUri.IsAbsoluteUri
            || (!requestUri.Scheme.Equals(Uri.UriSchemeHttp, StringComparison.OrdinalIgnoreCase)
                && !requestUri.Scheme.Equals(Uri.UriSchemeHttps, StringComparison.OrdinalIgnoreCase))
            || !string.IsNullOrEmpty(requestUri.UserInfo))
        {
            return new LocalAssetResponse(403, "text/plain", [], "Forbidden");
        }

        try
        {
            using HttpResponseMessage response = await _client.GetAsync(
                requestUri,
                HttpCompletionOption.ResponseHeadersRead,
                cancellationToken).ConfigureAwait(false);
            if (response.StatusCode != HttpStatusCode.OK)
            {
                return new LocalAssetResponse((int)response.StatusCode, "text/plain", [], "Remote image unavailable");
            }

            string? contentType = response.Content.Headers.ContentType?.MediaType;
            if (string.IsNullOrWhiteSpace(contentType)
                || !contentType.StartsWith("image/", StringComparison.OrdinalIgnoreCase))
            {
                return new LocalAssetResponse(415, "text/plain", [], "Unsupported Media Type");
            }

            if (response.Content.Headers.ContentLength > MaxRemoteImageBytes)
            {
                return new LocalAssetResponse(413, "text/plain", [], "Payload Too Large");
            }

            byte[] content = await response.Content.ReadAsByteArrayAsync(cancellationToken).ConfigureAwait(false);
            return content.LongLength > MaxRemoteImageBytes
                ? new LocalAssetResponse(413, "text/plain", [], "Payload Too Large")
                : new LocalAssetResponse(200, contentType, content);
        }
        catch (HttpRequestException)
        {
            return new LocalAssetResponse(502, "text/plain", [], "Bad Gateway");
        }
        catch (TaskCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            return new LocalAssetResponse(504, "text/plain", [], "Gateway Timeout");
        }
        catch (OperationCanceledException)
        {
            throw;
        }
    }
}
