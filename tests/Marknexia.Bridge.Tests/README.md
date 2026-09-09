# Rendered document bridge tests

These Node tests load the application's actual `bridge.js` in headless Chromium.
They verify DOM behavior that .NET string-based renderer tests cannot establish:
literal search, visible occurrence counts, selection, forward/backward wrapping,
CSS whitespace and inline/block boundaries, collapsed content, scroll-container
visibility, cache invalidation, and delegated
copy/link/source controls. The bridge runs before a real document load; only the
native host's message receiver is stubbed, and uncaught browser errors fail tests.
They do not drive the Windows desktop or establish native WebView2/XAML behavior.

Use Node 22 and the committed lockfile. From this directory:

```powershell
npm ci --ignore-scripts
npx --no-install playwright install chromium
npm test
```

To use an already installed Chrome instead of downloading test Chromium:

```powershell
$env:MARKNEXIA_TEST_BROWSER = 'chrome'
npm test
Remove-Item Env:MARKNEXIA_TEST_BROWSER
```

The CI and release workflows use Playwright's pinned Chromium build. Playwright
is a development-only dependency; it is not shipped with Marknexia.
