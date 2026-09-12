using System.Net;
using System.Net.Http.Headers;
using FluentAssertions;
using Marknexia.Files;
using Xunit;

namespace Marknexia.Files.Tests;

public sealed class RemoteImageReaderTests
{
    [Fact]
    public async Task ReadAsync_returns_a_bounded_image_response()
    {
        using var client = new HttpClient(new StubHandler(_ => new HttpResponseMessage(HttpStatusCode.OK)
        {
            Content = new ByteArrayContent([0x89, 0x50, 0x4E, 0x47])
            {
                Headers = { ContentType = new MediaTypeHeaderValue("image/png") }
            }
        }));
        var reader = new RemoteImageReader(client);

        LocalAssetResponse result = await reader.ReadAsync(new Uri("https://example.test/image.png"));

        result.StatusCode.Should().Be(200);
        result.ContentType.Should().Be("image/png");
        result.Content.Should().Equal(0x89, 0x50, 0x4E, 0x47);
    }

    [Fact]
    public async Task ReadAsync_rejects_non_image_responses_and_non_http_uris()
    {
        using var client = new HttpClient(new StubHandler(_ => new HttpResponseMessage(HttpStatusCode.OK)
        {
            Content = new StringContent("not an image")
        }));
        var reader = new RemoteImageReader(client);

        (await reader.ReadAsync(new Uri("https://example.test/file.txt"))).StatusCode.Should().Be(415);
        (await reader.ReadAsync(new Uri("file:///secret.png"))).StatusCode.Should().Be(403);
    }

    private sealed class StubHandler(Func<HttpRequestMessage, HttpResponseMessage> factory) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken) =>
            Task.FromResult(factory(request));
    }
}
