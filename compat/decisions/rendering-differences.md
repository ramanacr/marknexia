# Rendering pipeline differences (REND-1 to REND-7)

Status: **proposed — requires Task 8 approval**

Scope: `crates/marknexia-rendering` (`Renderer`, both Markdown candidates). It reproduces the .NET `MarkdownRenderer`, `TemplateEngine`, `ColorCodeSyntaxHighlighter`, `MermaidDiagramRenderer` and `SimpleMathRenderer` at fixture revision `38b39f6`.

## Fixture result

All 7 frozen cases in `compat/fixtures/v1/rendering` match byte-for-byte, including the complete templated page, for `rendering-pulldown` and `rendering-comrak` (`tests/parity_v1.rs`). This file therefore has no `parity-allowance` blocks. Every difference below lies outside the frozen corpus. The .NET exporter should add cases for them so they are recorded against the oracle.

## REND-1 — Sanitizer and host budgets are lower than .NET's page limit

.NET renders any page up to `MaxRenderedHtmlBytes` (128 MiB) after reading a source of up to 50 MiB. Rust keeps both limits (`RenderLimits`, with exact edges tested), but the body also passes `HtmlPolicy`, which has hard caps of 4 MiB input and 8 MiB output (SAN-8 adds depth 256 and a work budget). The WebView `HostDocument` has an 8 MiB cap.

| Input | .NET | Rust |
| --- | --- | --- |
| Body HTML (after code, diagram and math passes) over 4 MiB, for example a 4 MiB paragraph or a 1.5 MiB code block (copy metadata percent-encodes it) | rendered | `RenderError::Sanitizer(InputTooLarge)`, no partial output |
| Raw HTML nested deeper than 256 levels | rendered | `RenderError::Sanitizer(NestingTooDeep)` |

The body is also checked against the 4 MiB budget after each pass, so amplification (copy metadata, diagram shells) fails early rather than building oversized strings.

## REND-2 — Only C# is highlighted

.NET highlights every ColorCode 2.0.15 language. Rust has a Marknexia-owned, linear-time C# tokenizer (`src/highlight.rs`) and no other grammar. For `c#`, `cs`, `csharp` and `cake`, the output has the ColorCode structure `<div class="csharp"><pre>` with `<span class="keyword|number|string|stringCSharpVerbatim|comment|xmlDocTag|xmlDocComment|preprocessorKeyword">` spans. The rules were translated from the regex strings in the ColorCode.Core 2.0.15 string heap, in the order they are stored there.

| Code block language | .NET | Rust |
| --- | --- | --- |
| cpp/c/c++, css, html, java, javascript/js, typescript/ts, powershell/ps1/posh, sql, vb.net, xml, php, python, markdown/md, haskell, koka, fortran, matlab, aspx/asax/ashx and other ColorCode ids or aliases | `<div class="{language}"><pre>` with token spans | `<pre><code>{code}</code></pre>`: the .NET plain-text fallback, without colors |
| C# constructs other than keywords and numbers (the only ones the fixture covers) | ColorCode output | reconstructed ColorCode output, not checked against an oracle run |

The copy button and its `data-copy-text` metadata are the same for every language.

## REND-3 — The page uses `\n` line endings

`TemplateEngine` uses `AppendLine`. On Windows that writes `\r\n`, and the embedded assets are CRLF in the Windows working copy. The frozen fixtures are newline-normalized, so Rust writes `\n` throughout. `max_rendered_html_bytes` is checked against the LF page (`RenderedDocument::page_bytes`, exact). On Windows the .NET page is larger by one byte per line break, so .NET rejects slightly earlier near the limit. The rendered DOM is the same.

## REND-4 — Remote-image blanking runs on a permissive pass, then sanitizes again

.NET sanitizes with remote images kept and then regex-blanks them (`src=""`, `srcset` removed). Rust does the same with `RemoteImagePolicy::AllowHttps`, applies the same blanking scanner, and sanitizes the result again under `Deny`, so the returned `SanitizedFragment` comes from the document's real policy. When the body contains no `src`, a single `Deny` pass gives the same result and is used instead. What remains different comes from SAN-3: the Rust policy removes `http://…` and `//host/…` image sources, where .NET keeps them and then blanks them. So Rust emits `<img alt="…">` where .NET emits `<img src="" alt="…">`. The fixture (`https://`) matches exactly.

## REND-5 — Math renderer bounds

`SimpleMathRenderer` is ported with its quirks preserved:

* The matched text is still entity-encoded and is encoded again, so `$a<b$` shows `a&lt;b`.
* `x^{2}` reads the atom `}` (`ReadAtom` hands the brace position to `ReadGroup`), so it renders `x<sup></sup>`.
* Leading white space inside the span disables delimiter stripping.

Two differences:

| Input | .NET | Rust |
| --- | --- | --- |
| Structural nesting (`\frac`, `\sqrt`, `\text`, `^`, `_`) deeper than `MAX_MATH_DEPTH` (32) | unbounded recursion; stack overflow at extreme depth | deeper content is emitted as encoded literal text |
| `^\` or `_\` at the end of an expression | `ArgumentOutOfRangeException`, and the whole render fails | argument clamped to `\` |

The `ReadAtom`/`ReadGroup` mutual recursion runs as a loop, so `^{^{^{…` cannot overflow the stack.

## REND-6 — .NET text primitives are approximated

* **HTML decoding.** `WebUtility.HtmlDecode` (code-block and diagram source) decodes numeric references and uses the HTML5 named-reference table from `markup5ever`, a superset of the .NET HTML 4 table. HTML5-only names such as `&check;` inside raw-HTML `<pre><code>` decode in Rust but not in .NET. A numeric reference to a lone surrogate becomes U+FFFD, because Rust strings cannot hold one.
* **Regex classes.** `\s` uses Unicode White_Space, the same set as .NET. `\w` is exact for ASCII. For non-ASCII it uses `char::is_alphanumeric`, which differs from .NET `[\p{L}\p{Mn}\p{Nd}\p{Pc}]` for some marks and other numerics (`²`). `char.IsLetter` in the math port is approximated with `char::is_alphabetic`.
* **Case-insensitive matching.** The Mermaid and math passes and remote-image blanking are ASCII case-insensitive. .NET `RegexOptions.IgnoreCase` also treats U+212A KELVIN SIGN as `k` and U+017F LONG S as `s`, so for example `<pre><code clasſ="language-mermaid">` would be a diagram in .NET only.
* **Leftmost-match emulation and linear time.** Every .NET regex pass is a direct scanner that reproduces the regex's leftmost-match and backtracking outcome. Scanners memoize failed searches, so inputs that are quadratic under the .NET regexes (unterminated quotes, `/*`, `[type:"`, long blank runs, unterminated math spans) stay linear.

## REND-7 — API shape

These are additive and do not change the output:

* The caller supplies the template's document id and CSP nonce (`PageIdentity`, 16 random bytes each). .NET draws them from `Guid.NewGuid()` and `RandomNumberGenerator` internally.
* The document directory is passed relative to the asset root. A path that escapes the root fails with `RenderError::InvalidDocumentDirectory`, as .NET throws `ArgumentException`.
* Headings, anchors, image references and diagram sources are returned as `PlainText`, which reaches HTML only through `PlainText::encode_html`. The body is returned only as a `SanitizedFragment`.
* Diagnostics are the Markdown layer's additive ones (`markdown-source-diagnostics.md`, `markdown-resource-limits.md`).
* The 50 MiB pre-read check is `Renderer::check_source_size`. `render` also applies it to the string length.

## Approval needed

Task 8 must approve or change:

1. The REND-1 budgets, or raise the sanitizer caps.
2. C#-only highlighting (REND-2), or a decision to add more grammars.
3. The LF page measurement (REND-3).
4. The math bounds (REND-5).

REND-4, REND-6 and REND-7 are recorded so reviewers can see them.
