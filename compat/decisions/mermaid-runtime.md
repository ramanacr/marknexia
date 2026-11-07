# Mermaid runtime in the viewport (MER-1 to MER-5)

Status: **approved — owner decision 2026-11-06 (G0)**

Scope: `crates/marknexia-webview` (`runtime`, `broker`, `policy::HostDocument`, `build.rs`) and `packaging/webview/mermaid.lock.json`. The page bytes come from `crates/marknexia-rendering` and are unchanged.

## Parity that holds

* **Page bytes.** The rendered page is unchanged. The diagram shell, the limit fallback, the runtime tag `<script src="https://marknexia.assets/mermaid.min.js" nonce="{nonce}"></script>` and the policy `script-src 'nonce-{nonce}' https://marknexia.assets` are the .NET `TemplateEngine` and `MermaidDiagramRenderer` output. The frozen cases `diagram-limit` (one rendered diagram with the runtime tag, one count fallback) and `diagram-source-limit` match byte for byte (`crates/marknexia-rendering/tests/parity_v1.rs`).
* **The runtime.** Rust serves the file the .NET edition embeds, `src/Marknexia.Rendering/Assets/mermaid.min.js` (Mermaid 10.9.1). `build.rs` reads that exact file and checks both pins in `mermaid.lock.json`: the LF repository bytes (3,335,717 bytes, SHA-256 `61b335a4…36d6`) and the CRLF bytes (3,337,746 bytes, SHA-256 `1e9cd6ab…21a5`). The CRLF bytes are the ones served. They are byte-identical to the file that the installed .NET edition extracted to `%LOCALAPPDATA%\Marknexia\Assets\mermaid.min.js` on the development host (verified 2026-11-06). A change to the file or to the lock fails the build. Nothing is downloaded at build time or at run time.
* **Request rule.** Like .NET `IsBundledRuntimeRequest`, only a script-context `GET` for `https://marknexia.assets/mermaid.min.js` is served. Every other path on that host, and every other resource kind, is denied with 403.
* **Limits and fallbacks.** The diagram count limit (64) and source limit (1 MiB) and the `role="note"` source fallback are the .NET ones. A page with only fallbacks carries no runtime tag, as in .NET (`hasMermaid` is false). Like .NET, the shell is built before sanitizing: a shell that the sanitizer then removes (for example inside raw `<script>` content) still makes the page load the runtime, which finds no diagram.
* **Runtime failure.** `bridge.js` is the .NET bridge. When `mermaid.render` rejects or throws, the diagram's `-error` block is shown with the message and the viewport is hidden. As in .NET, this is an in-page diagnostic only; no host diagnostic is raised. The limit diagnostics (`mermaid-N: diagram limit exceeded`, `mermaid-N: diagram source limit exceeded`) come from the Markdown layer, as before.

## Observable differences

| ID | Condition | .NET | Rust |
| --- | --- | --- | --- |
| MER-1 | Where the runtime is read from | Extracted to `%LOCALAPPDATA%\Marknexia\Assets\mermaid.min.js` (rewritten when the hash differs or the file is a reparse point), then served by `SetVirtualHostNameToFolderMapping("marknexia.assets", …, Allow)` | Served from memory through the WebView2 resource broker. No file is written, and no folder is mapped. |
| MER-2 | Which documents can load the runtime | After the first page with a diagram, the host mapping stays on that `CoreWebView2` for later pages in the same tab | Only a document whose rendered page carries the runtime tag with its own nonce (`HostDocument::loads_mermaid_runtime`). Other documents get 403. The sanitized body cannot add a script, so a page without diagrams never requests it. |
| MER-3 | Request URI variants and methods | `Uri.AbsolutePath` comparison: a query string (`?v=1`) is still served; scheme and host compare case-insensitively; `Uri` unescapes unreserved characters, so `/%6dermaid.min.js` is served as `/mermaid.min.js`. `IsBundledRuntimeRequest` does not check the method, so a non-`GET` script request goes to the folder mapping. | Exact comparison with the normalized URI that WebView2 reports: a query, a fragment, any percent-encoded character (including `%6d`) or a port is denied with 403. A non-`GET` request is refused with 405 (`Method Not Allowed`), before the URI is examined. |
| MER-4 | Response headers | The virtual host mapping's defaults (MIME type from the extension; browser caching) | `Content-Type: text/javascript; charset=utf-8`, `X-Content-Type-Options: nosniff`, `Cache-Control: no-store`, and the default broker `Content-Security-Policy`. Each page load reads the runtime again, streamed in place from the bytes embedded in the binary (no copy). |
| MER-5 | A .NET build from an LF checkout | Ships the LF bytes (`61b335a4…`) | Always serves the pinned CRLF bytes. Template literals normalize CRLF to LF, so the script behaves the same. |

## Evidence

* Headless: `crates/marknexia-webview/tests/rendered_document.rs` (`a_page_with_a_diagram_gets_the_pinned_mermaid_runtime_and_nothing_else`, `a_page_without_a_diagram_never_gets_the_runtime`, `trusted_and_generated_documents_never_get_the_runtime`), `tests/broker.rs` (`the_mermaid_runtime_is_served_only_when_enabled_and_only_as_a_script`, `every_single_edit_of_the_runtime_uri_is_denied`), `src/runtime.rs` (lock and embedded bytes agree), and the `build.rs` SHA-256 checks with known-answer self-tests.
* Native: `session::native_tests::native_mermaid_runtime_renders_and_falls_back` (ignored; needs an interactive host with Evergreen). It checks that a valid diagram becomes SVG, that an invalid one shows the in-page error with a message, and that a diagram over the source limit stays the source fallback without the runtime. It also checks that a page without diagrams cannot load the runtime: a `<script src>` injected through `ExecuteScript` fails and `fetch` is rejected. Finally, a diagram with `%%{init: {'securityLevel': 'loose'}}%%` and `click` bindings must keep `securityLevel` `strict` and render an SVG with no `on*` attribute and no `javascript:` value. The test runs in the `webview-session` native gate (`--lib --ignored`). It was compiled but not run in this lane, because the workstation was locked.

## Approval needed

Task 8 must approve or change MER-1 to MER-5. The SBOM and third-party notices for the Rust package must list Mermaid 10.9.1 (MIT) from `mermaid.lock.json` (Phase 7).
