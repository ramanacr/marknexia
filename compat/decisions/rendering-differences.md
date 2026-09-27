# Rendering pipeline differences (REND-1 to REND-7)

Status: **proposed — requires Task 8 approval**

Scope: `crates/marknexia-rendering` (`Renderer`, both Markdown candidates). It reproduces the .NET `MarkdownRenderer`, `TemplateEngine`, `ColorCodeSyntaxHighlighter`, `MermaidDiagramRenderer` and `SimpleMathRenderer` at fixture revision `38b39f6`.

## Fixture result

All 7 frozen cases in `compat/fixtures/v1/rendering` match byte-for-byte, including the complete templated page, for `rendering-pulldown` and `rendering-comrak` (`tests/parity_v1.rs`). This file therefore has no `parity-allowance` blocks. Every difference below lies outside the frozen corpus. The .NET exporter should add cases for them so they are recorded against the oracle.

## REND-1 — The .NET size limits apply; sanitizer budgets remain

.NET renders any page up to `MaxRenderedHtmlBytes` (128 MiB) after reading a source of up to 50 MiB (`FileService.LargeFileThresholdBytes`). Rust applies the same two limits (`RenderLimits`, with exact edges tested) and no lower one. The sanitizer accepts a 128 MiB body and writes up to 128 MiB (SAN-8), and every pass runs within that 128 MiB budget. What remains different:

| Input | .NET | Rust |
| --- | --- | --- |
| Raw HTML nested deeper than 256 levels | rendered | `RenderError::Sanitizer(NestingTooDeep)` |
| Raw HTML beyond the tree-construction work or reparent budget (SAN-8): flat content under about 250 open elements, or misnesting repaired more than once per 256 bytes | rendered, slowly | `RenderError::Sanitizer(TooComplex)` |
| A pass (copy metadata, C# highlighting at about 15×, diagram shells, `\frac` chains at about 10×) that takes the body past 128 MiB | builds the page, then `DocumentTooLargeException` | `RenderError::Sanitizer(InputTooLarge)` before building that block |
| A body over 128 MiB after the passes that sanitizing would shrink under the page limit (megabytes of removed `<script>` or `<style>` content) | rendered | `RenderError::Sanitizer(InputTooLarge)` |

Before building a code-block, diagram or math replacement, a pass checks its exact size against the budget left. For highlighted C# the check uses an exact lower bound. The sizes are derived from the replacements' template text (review M5: the old constants were 7 to 54 bytes too large, so blocks at the edge were rejected early). `passes::tests::*_budget_edges_are_exact` check that a block needing `n` bytes is accepted at a budget of `n` and rejected at `n - 1`. Each pass takes the body by value and returns it without a copy when nothing matched, so only one intermediate body is alive at a time.

**Display.** The WebView `HostDocument` (`crates/marknexia-webview`, another lane) still caps a hosted document at 8 MiB. Rust renders larger pages, but the shell cannot display them until that cap is raised.

**Speed and memory.** `crates/marknexia-rendering/tests/scale.rs` (`scale_report`, ignored, release) renders generated Markdown shapes at 4, 16 and 50 MiB. .NET was timed on the same files through `MarkdownRenderer.RenderAsync` (Release build, .NET 10.0.401), one process per file with a 600 s timeout. Rust "before" is the double-sanitizing pipeline of `44be9ba` with only its 4/8 MiB caps raised. Times are seconds. Rust memory is the peak heap from a counting allocator; .NET memory is the peak working set. The .NET runs overlapped with Rust builds on the same 8-thread machine, so they are an upper bound.

| Shape | MiB | .NET | Rust before | Rust after |
| --- | --- | --- | --- | --- |
| prose | 4 | 8.3 s, 237 MiB | 1.96 s, 78 MiB | 0.36 s, 67 MiB |
| prose | 16 | 82.3 s, 799 MiB | 9.1 s, 315 MiB | 1.30 s, 269 MiB |
| prose | 50 | timeout (> 600 s) | not run | 4.0 s, 857 MiB (58.8 MiB page) |
| tables | 4 | 37.9 s, 749 MiB | 10.1 s, 330 MiB | 1.74 s, 257 MiB |
| tables | 16 | timeout | 49.5 s, 1,320 MiB | 7.0 s, 1,029 MiB |
| tables | 50 | timeout | not run | `RenderedTooLarge` (body over 128 MiB) in 6.7 s |
| code | 4 | 42.3 s, 848 MiB | 11.6 s, 416 MiB | 1.58 s, 95 MiB (28 MiB page) |
| code | 16 | timeout | 47.6 s, 1,639 MiB | 6.2 s, 383 MiB (112 MiB page) |
| code | 50 | timeout | not run | `Sanitizer(InputTooLarge)` (copy metadata over 128 MiB) in 1.9 s |
| lists | 4 | 42.4 s, 733 MiB | — | 2.15 s, 310 MiB |
| lists | 16 | timeout | — | 8.7 s, 1,240 MiB |
| lists | 50 | timeout | — | `RenderedTooLarge` in 8.5 s |
| links | 4 / 16 / 50 | 12.1 s / 90.6 s / timeout | — | 0.93 s / 2.6 s / 8.1 s (1,414 MiB) |
| raw `<b>x</b>` | 4 | timeout | 5.3 s, 356 MiB | 1.87 s, 356 MiB |
| raw `<b>x</b>` | 16 | timeout | 20.5 s, 1,424 MiB | 8.3 s, 1,424 MiB |
| raw `<b>x</b>` | 50 | timeout | not run | 27.6 s, 5,010 MiB |
| raw HTML blocks | 4 / 16 / 50 | 44.2 s / timeout / not run | — | 0.55 s / 2.3 s / 7.0 s (1,318 MiB) |
| mixed | 4 | 32.8 s, 616 MiB | 9.8 s, 318 MiB | 1.19 s, 144 MiB |
| mixed | 16 | 250 s, 1,788 MiB | 39.8 s, 1,272 MiB | 4.5 s, 579 MiB |
| mixed | 50 | not run | not run | `Sanitizer(InputTooLarge)` (passes over 128 MiB) in 4.4 s |
| one raw `<div>` around everything | 4 / 16 / 50 | 8.8 s / 74.5 s / not run | 1.73 s / 6.9 s / — | 0.68 s / 2.5 s / 7.9 s (857 MiB) |
| one paragraph | 4 / 16 / 50 | 1.16 s / 1.23 s / not run | — | 0.04 s / 0.15 s / 0.47 s |

Where the Rust time and memory now go at 50 MiB:

* **The Markdown parse is the largest part.** `marknexia-markdown` builds a full document model: 1.8 to 8.5 s, and 17 to 100 bytes of heap per source byte (5.0 GiB for 50 MiB of `<b>x</b>`, 3.9 GiB for lists). It is outside this change.
* **The sanitizer is parallel.** It keeps one chunk's DOM per worker (under 220 MiB at 50 MiB) and cleans about 10 to 30 MB of body per second.
* **Single chunks are sequential.** A document with no split point, such as a single 50 MiB paragraph of raw inline HTML or everything inside one raw `<div>`, is one chunk (SAN-8): it is sanitized on one thread and its whole DOM is in memory (2.3 GiB for 50 MiB of `<b>x</b>`).

**Cancellation.** `Renderer::render_cancellable(source, context, cancel)` polls `cancel` between stages and inside the sanitizer, at most about 16 KiB of body apart, and returns `RenderError::Cancelled`. Only the Markdown parse cannot be interrupted.

## REND-2 — Only C# is highlighted

.NET highlights every ColorCode 2.0.15 language. Rust has a Marknexia-owned, linear-time C# tokenizer (`src/highlight.rs`) and no other grammar. For `c#`, `cs`, `csharp` and `cake`, the output has the ColorCode structure `<div class="csharp"><pre>` with `<span class="keyword|number|string|stringCSharpVerbatim|comment|xmlDocTag|xmlDocComment|preprocessorKeyword">` spans. The rules were translated from the regex strings in the ColorCode.Core 2.0.15 string heap, in the order they are stored there.

| Code block language | .NET | Rust |
| --- | --- | --- |
| cpp/c/c++, css, html, java, javascript/js, typescript/ts, powershell/ps1/posh, sql, vb.net, xml, php, python, markdown/md, haskell, koka, fortran, matlab, aspx/asax/ashx and other ColorCode ids or aliases | `<div class="{language}"><pre>` with token spans | `<pre><code>{code}</code></pre>`: the .NET plain-text fallback, without colors |
| C# constructs other than keywords and numbers (the only ones the fixture covers) | ColorCode output | reconstructed ColorCode output, not checked against an oracle run |

The copy button and its `data-copy-text` metadata are the same for every language.

## REND-3 — The page uses `\n` line endings

`TemplateEngine` uses `AppendLine`. On Windows that writes `\r\n`, and the embedded assets are CRLF in the Windows working copy. The frozen fixtures are newline-normalized, so Rust writes `\n` throughout. `max_rendered_html_bytes` is checked against the LF page (`RenderedDocument::page_bytes`, exact). On Windows the .NET page is larger by one byte per line break, so .NET rejects slightly earlier near the limit. The rendered DOM is the same.

## REND-4 — Remote-image blanking happens in the sanitizer's single pass

.NET sanitizes with remote images kept, then regex-blanks the sanitized markup: a quoted `<img src>` starting with `http://`, `https://` or `//` becomes `src=""`, and a remote `srcset` is removed. With remote assets off, Rust sanitizes once under `RemoteImagePolicy::Blank`. Its attribute filter takes the same decision on the parsed, entity-decoded value: an `<img src>` that starts with one of those prefixes, in any case, becomes `src=""` (SAN-3). `srcset` is never an allowed attribute. The returned `SanitizedFragment` therefore comes straight from the document's real policy, with no permissive pass, no string pass and no second sanitization. With remote assets on, the policy is `AllowHttps`, as before.

The frozen fixtures (`remote-images-off`, `remote-images-on`) match exactly. `http://` and `//host/…` sources are now blanked to `src=""` as in .NET; the old double pass removed them. What still differs is stricter than .NET, where the .NET pattern misses a source and only the page CSP blocks the fetch. Both cases are pinned by `remote_sources_dotnet_leaks_are_removed`, and the .NET-identical cases by `remote_image_sources_are_blanked_like_dotnet`:

| Input (remote assets off) | .NET | Rust |
| --- | --- | --- |
| `<input type="image" src="https://…">` (`BlockRemoteImages` only matches `<img`) | remote `src` kept | `src` removed |
| `<img src="https://…/a'b.png">`, a quote character inside the value (`[^"']*` stops at it) | remote `src` kept | `src=""` |
| `<img src=" https://…">`, leading white space | remote `src` kept | `src` removed (SAN-3) |

**.NET fixture request:** add these inputs, plus `http://` and `//` image sources, to the exporter's rendering cases.

## REND-5 — Math renderer bounds

`SimpleMathRenderer` is ported with its quirks preserved:

* The matched text is still entity-encoded and is encoded again, so `$a<b$` shows `a&lt;b`.
* `x^{2}` reads the atom `}` (`ReadAtom` hands the brace position to `ReadGroup`), so it renders `x<sup></sup>`.
* Leading white space inside the span disables delimiter stripping.

Two differences:

| Input | .NET | Rust |
| --- | --- | --- |
| Structural nesting beyond an element budget of `MAX_MATH_DEPTH` (32). `^`, `_` and `\text` cost 1, `\frac` and `\sqrt` cost 2 because they emit nested spans. | unbounded recursion; stack overflow at extreme depth | deeper content is emitted as encoded literal text |
| Math output larger than the remaining 128 MiB body budget (REND-1) | rendered | `RenderError::Sanitizer(InputTooLarge)`, stopped while rendering |
| `^\` or `_\` at the end of an expression | `ArgumentOutOfRangeException`, and the whole render fails | argument clamped to `\` |

The `ReadAtom`/`ReadGroup` mutual recursion runs as a loop, so `^{^{^{…` cannot overflow the stack.

The budget counts generated elements, not recursion, so math nests at most 35 elements including its wrapper. The Markdown layer nests at most 32 block and 32 inline containers plus a few wrappers. Together that stays far below the sanitizer's 256-level limit, so deep math never turns a document into `NestingTooDeep` (`deep_math_inside_deep_markdown_stays_within_sanitizer_depth`). Only raw HTML that is already close to the limit can still reach it, and such a document fails the same way without math.

## REND-6 — .NET text primitives are approximated

* **HTML decoding.** `WebUtility.HtmlDecode` (code-block and diagram source) decodes numeric references and uses the HTML5 named-reference table from `markup5ever`, a superset of the .NET HTML 4 table. HTML5-only names such as `&check;` inside raw-HTML `<pre><code>` decode in Rust but not in .NET. A numeric reference to a lone surrogate becomes U+FFFD, because Rust strings cannot hold one.
* **Regex classes.** `\s` uses Unicode White_Space, the same set as .NET. `\w` is exact for ASCII. For non-ASCII it uses `char::is_alphanumeric`, which differs from .NET `[\p{L}\p{Mn}\p{Nd}\p{Pc}]` for some marks and other numerics (`²`). `char.IsLetter` in the math port is approximated with `char::is_alphabetic`.
* **Case-insensitive matching.** The Mermaid and math passes and remote-image blanking are ASCII case-insensitive. .NET `RegexOptions.IgnoreCase` also treats U+212A KELVIN SIGN as `k` and U+017F LONG S as `s`, so for example `<pre><code clasſ="language-mermaid">` would be a diagram in .NET only.
* **Leftmost-match emulation and linear time.** Every .NET regex pass is a direct scanner that reproduces the regex's leftmost-match and backtracking outcome. Inputs that are quadratic under the .NET regexes stay linear here because the scanners memoize their searches:
  * unterminated quotes, `/*` and blank runs keep failure bounds;
  * the C# attribute rule's next `]`/`"` and next `]` searches use a forward cache (`NextOf`), and its string search is cached per opening quote;
  * unterminated math spans keep a dead-from offset per tag;
  * remote-image blanking keeps a failed-candidate bound.

  Review C1 found the attribute stop search unmemoized, which was quadratic: `[type:` repeated 64,000 times took 27.2 s. It is now linear, at 60 ms for n = 64,000 and 159 ms for n = 200,000 (release). The remaining searches were audited, and each one either consumes what it scans or scans regions that do not overlap.

## REND-7 — API shape

These are additive and do not change the output:

* The caller supplies the template's document id and CSP nonce (`PageIdentity`, 16 bytes each, generated from the OS RNG by the host crate). .NET draws them from `Guid.NewGuid()` and `RandomNumberGenerator` internally. `PageIdentity::new` rejects an all-zero id or nonce, and its `Debug` output redacts the nonce.
* The document directory is passed relative to the asset root. A path that escapes the root fails with `RenderError::InvalidDocumentDirectory`, as .NET throws `ArgumentException`.
* Headings, anchors, image references and diagram sources are returned as `PlainText`, which reaches HTML only through `PlainText::encode_html`. The body is returned only as a `SanitizedFragment`.
* Diagnostics are the Markdown layer's additive ones (`markdown-source-diagnostics.md`, `markdown-resource-limits.md`).
* The 50 MiB pre-read check is `Renderer::check_source_size`. `render` also applies it to the string length.
* `Renderer::render_cancellable` takes a `&(dyn Fn() -> bool + Sync)` and returns `RenderError::Cancelled` once it returns `true` (REND-1). .NET's `RenderAsync` takes a `CancellationToken` and checks it between regex matches; `render` keeps its signature.

## Approval needed

Task 8 must approve or change:

1. The REND-1 remaining budget differences (nesting, work and reparent budgets; a pre-sanitizer body over 128 MiB).
2. C#-only highlighting (REND-2), or a decision to add more grammars.
3. The LF page measurement (REND-3).
4. The math bounds (REND-5).

REND-4, REND-6 and REND-7 are recorded so reviewers can see them.
