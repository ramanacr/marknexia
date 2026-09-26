# Sanitizer differences: ammonia-based Rust policy vs .NET Ganss oracle

Status: **proposed — requires Task 8 approval.** Nothing here changes a v1 expected result. The frozen fixtures under `compat/fixtures/v1/sanitizer/` stay as they are. Each difference below is asserted by name in `crates/marknexia-security/tests/parity_v1.rs` or `crates/marknexia-security/tests/hostile_properties.rs`, so none of them passes silently.

Oracle: `src/Marknexia.Security/HtmlSanitizerService.cs` (Ganss HtmlSanitizer 9.2.1039 on AngleSharp, with a regex pre-filter). Target: `crates/marknexia-security` (ammonia 4.2.0 / html5ever 0.40, with CSS decided on `cssparser` 0.38 tokens).

## SAN-1 — CSS shorthand expansion (`unsafe-css` fixture)

* **Input:** `<div style="background:url(javascript:alert(1))">text</div>`
* **.NET:** `<div style="background-position: initial; background-size: initial; background-repeat: initial; background-attachment: initial; background-origin: initial; background-clip: initial; background-color: initial">text</div>`
* **Rust:** `<div>text</div>`
* **Why it differs:** AngleSharp's CSSOM expands `background` into longhands before Ganss removes the `background-image` longhand that holds the `url()`. The leftover longhands, all set to `initial`, are an artifact of that CSSOM. To match them, Rust would need AngleSharp's per-property shorthand grammar. The Rust policy works on tokens. It drops any declaration that contains `url()` or another active construct, and it drops the `style` attribute when no declaration is left (as Ganss does for a style that ends up empty).
* **Observable effect:** The .NET output resets seven background longhands to `initial`, which does nothing on an element with no other background. Rendering is the same. The security outcome is the same: no URL is fetched and no script runs.

## SAN-2 — CSS value normalization

AngleSharp re-serializes values it understands, for example `color: red` becomes `color: rgba(255, 0, 0, 1)`, and it drops values that fail property grammar. Rust keeps any allowlisted property whose value contains only inert tokens (identifiers, numbers, dimensions, percentages, hashes, strings, commas, `! + - * / . %` delimiters, and allowlisted functions such as `rgb()`, `calc()` and gradients). It re-serializes the value token by token as `name: value` joined by `"; "`. The value must re-tokenize to itself; otherwise the whole style attribute is dropped. Rust therefore keeps grammatically invalid but inert values such as `color: foo`. The simple forms the rendering fixtures use (`display: none`) serialize the same way in both.

## SAN-3 — URL policy is stricter than Ganss

Rust sends every `href`, `src`, `action`, `cite` and `longdesc` value through the existing `UrlPolicy`. Links and images are decided independently.

| Value | .NET | Rust |
|---|---|---|
| `/absolute/path`, `//host/x` | kept | attribute removed |
| `https://host:8443/`, IP literals, trailing-dot hosts, userinfo | kept | attribute removed |
| `mailto:` with headers, `tel:` extensions | kept | attribute removed |
| value with leading or trailing whitespace (non-script scheme) | kept (trimmed scheme check) | attribute removed |
| `<img src="https://…">` with remote images denied | kept (the Markdown layer blanks it earlier) | attribute removed |
| `<img src="http://…">` | kept | attribute removed |
| empty value (`src=""`) | kept | kept |

## SAN-4 — Script schemes: rewrite instead of removal

The .NET regex pre-filter rewrites a *literal, quoted* `href|src="javascript:…"` or `vbscript:` value to `"#"`. Any other script-scheme spelling reaches Ganss, which removes the attribute. That covers entity-encoded (`&#106;avascript:`), unquoted, and tab/newline-obfuscated values, plus `xlink:href`. Rust makes the decision on the parsed, entity-decoded value using the WHATWG URL parser. Every value whose scheme is `javascript` or `vbscript` becomes `"#"`, including `xlink:href` (which Rust treats as `href`, because html5ever gives it the local name `href`). Either way the result is inert.

## SAN-5 — `<use>` references are fragment-only

Ganss keeps `<use href="https://…">`. Rust accepts only same-document `#fragment` references on `<use>`, validated with `UrlContext::SvgReference`, and removes every other value. `xlink:href` is kept only when it passes the same check. Ganss would remove `xlink:href` on `<use>` in all cases, because it is not in its allowed attributes.

## SAN-6 — Unknown elements keep their text

Ganss removes a disallowed element together with its children. Rust lists every known HTML, SVG and MathML element outside the allowlist in ammonia's `clean_content_tags`, so those elements are removed with their content, as in .NET. An **unknown or custom** element (for example `<x-widget>text<b>b</b></x-widget>`) is unwrapped: the element goes, and its allowlisted children and text stay. What survives is still sanitized by the same policy.

## SAN-7 — SVG output budget

.NET has no separate SVG output bound. Rust bounds `SvgPolicy::sanitize` input with `max_svg_input_bytes` and its output with `max_html_output_bytes`, while serializing.

## SAN-8 — Nesting depth budget

AngleSharp has no nesting limit. html5ever's tree builder scans the open-element stack on many tags, so 20,000 nested `<div>`s (100 KiB) cost tens of seconds. Rust runs a depth probe first, using the same html5ever fragment parse. It rejects the whole input with `SanitizeError::NestingTooDeep` once the tree depth exceeds `MAX_NESTING_DEPTH` (256), and returns no partial output. Browsers cap parser nesting at 512.

## SAN-9 — Attribute value escaping

AngleSharp's formatter escapes only `&`, U+00A0 and `"` in attribute values. html5ever also escapes `<` and `>`, so `alt="<b>"` is emitted as `alt="&lt;b&gt;"`. Both parse back to the same value.
