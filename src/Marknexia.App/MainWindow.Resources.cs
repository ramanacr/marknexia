using Marknexia.Core;
using Marknexia.Files;
using Microsoft.UI.Xaml.Controls;
using Microsoft.Web.WebView2.Core;
using Windows.Foundation;
using Windows.Storage.Streams;

namespace Marknexia.App;

public sealed partial class MainWindow
{
    private static void AddResourceFilters(CoreWebView2 core)
    {
        // One brokered policy covers document images, navigation, and any
        // unexpected subresource. Virtual-host mappings are intentionally kept
        // for the bundled Mermaid script and the large-document cache; those
        // mappings do not raise WebResourceRequested.
        core.AddWebResourceRequestedFilter("*", CoreWebView2WebResourceContext.All);
    }

    private void AttachResourceBroker(CoreWebView2 core)
    {
        AddResourceFilters(core);
        core.WebResourceRequested += CoreWebView2_WebResourceRequested;
    }

    private async void CoreWebView2_WebResourceRequested(
        CoreWebView2 sender,
        CoreWebView2WebResourceRequestedEventArgs args)
    {
        Deferral? deferral = null;
        try
        {
            deferral = args.GetDeferral();
            await CompleteResourceRequestAsync(sender, args);
        }
        catch (Exception)
        {
            // A request can outlive a tab during replacement. Complete it with
            // a generic denial rather than surfacing a path or COM diagnostic.
            try
            {
                args.Response = CreateResponse(sender, new LocalAssetResponse(403, "text/plain", [], "Forbidden"));
            }
            catch { }
        }
        finally
        {
            try { deferral?.Complete(); }
            catch { }
        }
    }

    private async Task CompleteResourceRequestAsync(
        CoreWebView2 core,
        CoreWebView2WebResourceRequestedEventArgs args)
    {
        CoreWebView2WebResourceRequest request = args.Request;
        if (!string.Equals(request.Method, "GET", StringComparison.OrdinalIgnoreCase))
        {
            args.Response = CreateResponse(core, new LocalAssetResponse(405, "text/plain", [], "Method Not Allowed"));
            return;
        }

        Uri requestUri = new(request.Uri, UriKind.Absolute);
        DocumentTabState? state = _tabStates.FirstOrDefault(tab =>
            ReferenceEquals(tab.WebView.CoreWebView2, core));
        DocumentAssetContext? assetContext = state?.Document?.AssetContext;

        if (assetContext != null && IsSameOrigin(requestUri, assetContext.BaseUri))
        {
            if (args.ResourceContext != CoreWebView2WebResourceContext.Image)
            {
                args.Response = CreateResponse(core, new LocalAssetResponse(403, "text/plain", [], "Forbidden"));
                return;
            }

            LocalAssetResponse asset = await new LocalAssetReader(assetContext)
                .ReadAsync(requestUri);
            args.Response = CreateResponse(core, asset);
            return;
        }

        if (IsBundledRuntimeRequest(requestUri, args.ResourceContext)
            || IsRenderCacheRequest(requestUri, args.ResourceContext, state))
        {
            // Let WebView2's existing virtual-host mapping provide the body.
            return;
        }

        if (args.ResourceContext == CoreWebView2WebResourceContext.Image
            && (requestUri.Scheme.Equals("data", StringComparison.OrdinalIgnoreCase)
                || (IsRemoteHttp(requestUri) && _settingsService.Current.AllowRemoteAssets)))
        {
            // CSP and sanitization constrain data images; explicit settings are
            // required for network images. Leaving Response unset continues the
            // browser request under those policies.
            return;
        }

        args.Response = CreateResponse(core, new LocalAssetResponse(403, "text/plain", [], "Forbidden"));
    }

    private static bool IsSameOrigin(Uri request, Uri basis) =>
        request.Scheme.Equals(basis.Scheme, StringComparison.OrdinalIgnoreCase)
        && request.Host.Equals(basis.Host, StringComparison.OrdinalIgnoreCase)
        && request.Port == basis.Port
        && string.IsNullOrEmpty(request.UserInfo);

    private static bool IsRemoteHttp(Uri uri) =>
        uri.Scheme.Equals("http", StringComparison.OrdinalIgnoreCase)
        || uri.Scheme.Equals("https", StringComparison.OrdinalIgnoreCase);

    private static bool IsBundledRuntimeRequest(Uri uri, CoreWebView2WebResourceContext context) =>
        context == CoreWebView2WebResourceContext.Script
        && uri.Scheme.Equals("https", StringComparison.OrdinalIgnoreCase)
        && uri.Host.Equals("marknexia.assets", StringComparison.OrdinalIgnoreCase)
        && uri.AbsolutePath.Equals("/mermaid.min.js", StringComparison.Ordinal);

    private static bool IsRenderCacheRequest(
        Uri uri,
        CoreWebView2WebResourceContext context,
        DocumentTabState? state) =>
        context == CoreWebView2WebResourceContext.Document
        && uri.Scheme.Equals("https", StringComparison.OrdinalIgnoreCase)
        && uri.Host.Equals("marknexia.page", StringComparison.OrdinalIgnoreCase)
        && state?.RenderCacheFilePath is string cachePath
        && uri.AbsolutePath.Equals(
            "/" + Path.GetFileName(cachePath),
            StringComparison.OrdinalIgnoreCase);

    private static CoreWebView2WebResourceResponse CreateResponse(
        CoreWebView2 core,
        LocalAssetResponse result)
    {
        IRandomAccessStream body = new MemoryStream(result.Content, writable: false).AsRandomAccessStream();
        string headers = $"Content-Type: {result.ContentType}\r\n"
            + $"Content-Length: {result.Content.Length}\r\n"
            + "Cache-Control: no-store\r\n"
            + "X-Content-Type-Options: nosniff\r\n";
        return core.Environment.CreateWebResourceResponse(body, result.StatusCode, result.ReasonPhrase, headers);
    }
}
