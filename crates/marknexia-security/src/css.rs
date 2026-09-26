//! Inline `style` declaration sanitizer built on the `cssparser` tokenizer.
//!
//! Security decisions are made on CSS Syntax Level 3 tokens, never on the
//! source text: property names are decoded identifiers checked against the
//! Ganss `AllowedCssProperties` set, and values are rebuilt token by token
//! from an allowlist of inert token kinds and functions. `url()`, `image-set()`,
//! `expression()`, `var()`, `attr()`, at-keywords, `{}` blocks, bad strings and
//! bad URLs all reject the declaration. The rebuilt text is re-tokenized and
//! must reproduce itself exactly, so token juxtaposition cannot form a new
//! token (for example `url` followed by `(`) that was never validated.

use cssparser::{Delimiter, ParseError, Parser, ToCss, Token};

type CssError = ParseError<()>;

/// Deepest nested function or bracket block accepted in a value.
const MAX_VALUE_NESTING: u8 = 16;

/// Sanitize the body of a `style` attribute. Returns the retained
/// declarations serialized as `name: value` joined with `"; "`, or an empty
/// string when nothing survives or the result is not a stable fixpoint.
pub(crate) fn sanitize_declarations(css: &str) -> String {
    let once = sanitize_once(css);
    if once.is_empty() || sanitize_once(&once) != once {
        return String::new();
    }
    once
}

fn sanitize_once(css: &str) -> String {
    let mut parser = Parser::new(css);
    parser.set_nested_block_limit(MAX_VALUE_NESTING);
    let mut declarations: Vec<String> = Vec::new();
    loop {
        if parser.is_exhausted() {
            break;
        }
        let result: Result<Option<String>, CssError> =
            parser.parse_until_after(Delimiter::Semicolon, parse_declaration);
        if let Ok(Some(declaration)) = result {
            declarations.push(declaration);
        }
    }
    declarations.join("; ")
}

fn parse_declaration(parser: &mut Parser<'_>) -> Result<Option<String>, CssError> {
    let name = parser.expect_ident_cloned()?.to_ascii_lowercase();
    parser.expect_colon()?;
    if !is_allowed_property(&name) {
        return Ok(None);
    }
    let mut value = String::new();
    serialize_value(parser, &mut value)?;
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("{name}: {value}")))
}

fn serialize_value(parser: &mut Parser<'_>, out: &mut String) -> Result<(), CssError> {
    let mut pending_space = false;
    loop {
        let token = match parser.next_including_whitespace_and_comments() {
            Ok(token) => token.clone(),
            Err(_) => return Ok(()),
        };
        // Comments are separators, never joiners: `url/**/(` must not
        // serialize as `url(`.
        if matches!(token, Token::WhiteSpace(_) | Token::Comment(_)) {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        let closing = match &token {
            Token::Function(name) => {
                if !is_allowed_function(&name.to_ascii_lowercase()) {
                    return Err(CssError::custom(()));
                }
                Some(')')
            }
            Token::ParenthesisBlock => Some(')'),
            Token::SquareBracketBlock => Some(']'),
            Token::Ident(_)
            | Token::Hash(_)
            | Token::IDHash(_)
            | Token::QuotedString(_)
            | Token::Number { .. }
            | Token::Percentage { .. }
            | Token::Dimension { .. }
            | Token::Comma => None,
            Token::Delim('!' | '+' | '-' | '*' | '/' | '.' | '%') => None,
            // url(), bad tokens, at-keywords, `{}` blocks, `:`, attribute
            // matchers, CDO/CDC and escapes that tokenize as `\` delimiters.
            _ => return Err(CssError::custom(())),
        };
        token.to_css(out).map_err(|_| CssError::custom(()))?;
        if let Some(closing) = closing {
            parser.parse_nested_block(|nested| serialize_value(nested, out))?;
            out.push(closing);
        }
    }
}

/// Whether an SVG `fill`/`stroke` presentation value is inert. The value is
/// tokenized as CSS. `url()` may only name a same-document fragment
/// (`url(#id)` or `url("#id")`), and every other function must be on the
/// value allowlist.
pub(crate) fn is_safe_paint(value: &str) -> bool {
    let mut parser = Parser::new(value);
    parser.set_nested_block_limit(MAX_VALUE_NESTING);
    paint_tokens_are_safe(&mut parser).is_ok()
}

fn paint_tokens_are_safe(parser: &mut Parser<'_>) -> Result<(), CssError> {
    loop {
        let token = match parser.next() {
            Ok(token) => token.clone(),
            Err(_) => return Ok(()),
        };
        match &token {
            Token::UnquotedUrl(url) if is_fragment_reference(url) => {}
            Token::Function(name) if name.eq_ignore_ascii_case("url") => {
                parser.parse_nested_block(|nested| {
                    let url = nested.expect_string()?.clone();
                    if !is_fragment_reference(&url) {
                        return Err(CssError::custom(()));
                    }
                    nested.expect_exhausted()?;
                    Ok(())
                })?;
            }
            Token::Function(name) if is_allowed_function(&name.to_ascii_lowercase()) => {
                parser.parse_nested_block(paint_tokens_are_safe)?;
            }
            Token::ParenthesisBlock | Token::SquareBracketBlock => {
                parser.parse_nested_block(paint_tokens_are_safe)?;
            }
            Token::Ident(_)
            | Token::Hash(_)
            | Token::IDHash(_)
            | Token::Number { .. }
            | Token::Percentage { .. }
            | Token::Dimension { .. }
            | Token::Comma
            | Token::Delim('!' | '+' | '-' | '*' | '/' | '.' | '%') => {}
            _ => return Err(CssError::custom(())),
        }
    }
}

fn is_fragment_reference(url: &str) -> bool {
    url.len() > 1
        && url.starts_with('#')
        && url[1..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn is_allowed_function(name: &str) -> bool {
    matches!(
        name,
        "rgb"
            | "rgba"
            | "hsl"
            | "hsla"
            | "hwb"
            | "lab"
            | "lch"
            | "oklab"
            | "oklch"
            | "color"
            | "color-mix"
            | "calc"
            | "min"
            | "max"
            | "clamp"
            | "translate"
            | "translatex"
            | "translatey"
            | "translatez"
            | "translate3d"
            | "rotate"
            | "rotatex"
            | "rotatey"
            | "rotatez"
            | "rotate3d"
            | "scale"
            | "scalex"
            | "scaley"
            | "scalez"
            | "scale3d"
            | "skew"
            | "skewx"
            | "skewy"
            | "matrix"
            | "matrix3d"
            | "perspective"
            | "cubic-bezier"
            | "steps"
            | "linear"
            | "repeat"
            | "minmax"
            | "fit-content"
            | "linear-gradient"
            | "radial-gradient"
            | "conic-gradient"
            | "repeating-linear-gradient"
            | "repeating-radial-gradient"
            | "repeating-conic-gradient"
            | "blur"
            | "brightness"
            | "contrast"
            | "drop-shadow"
            | "grayscale"
            | "hue-rotate"
            | "invert"
            | "opacity"
            | "saturate"
            | "sepia"
            | "counter"
            | "counters"
            | "rect"
            | "inset"
            | "circle"
            | "ellipse"
            | "polygon"
    )
}

/// Ganss HtmlSanitizer 9.2.1039 `HtmlSanitizerDefaults.AllowedCssProperties`.
/// Custom properties (`--*`) stay disallowed, matching the .NET default.
pub(crate) const ALLOWED_CSS_PROPERTIES: &[&str] = &[
    "align-content",
    "align-items",
    "align-self",
    "all",
    "animation",
    "animation-delay",
    "animation-direction",
    "animation-duration",
    "animation-fill-mode",
    "animation-iteration-count",
    "animation-name",
    "animation-play-state",
    "animation-timing-function",
    "backface-visibility",
    "background",
    "background-attachment",
    "background-blend-mode",
    "background-clip",
    "background-color",
    "background-image",
    "background-origin",
    "background-position",
    "background-position-x",
    "background-position-y",
    "background-repeat",
    "background-repeat-x",
    "background-repeat-y",
    "background-size",
    "border",
    "border-bottom",
    "border-bottom-color",
    "border-bottom-left-radius",
    "border-bottom-right-radius",
    "border-bottom-style",
    "border-bottom-width",
    "border-collapse",
    "border-color",
    "border-image",
    "border-image-outset",
    "border-image-repeat",
    "border-image-slice",
    "border-image-source",
    "border-image-width",
    "border-left",
    "border-left-color",
    "border-left-style",
    "border-left-width",
    "border-radius",
    "border-right",
    "border-right-color",
    "border-right-style",
    "border-right-width",
    "border-spacing",
    "border-style",
    "border-top",
    "border-top-color",
    "border-top-left-radius",
    "border-top-right-radius",
    "border-top-style",
    "border-top-width",
    "border-width",
    "bottom",
    "box-decoration-break",
    "box-shadow",
    "box-sizing",
    "break-after",
    "break-before",
    "break-inside",
    "caption-side",
    "caret-color",
    "clear",
    "clip",
    "color",
    "column-count",
    "column-fill",
    "column-gap",
    "column-rule",
    "column-rule-color",
    "column-rule-style",
    "column-rule-width",
    "column-span",
    "column-width",
    "columns",
    "content",
    "counter-increment",
    "counter-reset",
    "cursor",
    "direction",
    "display",
    "empty-cells",
    "filter",
    "flex",
    "flex-basis",
    "flex-direction",
    "flex-flow",
    "flex-grow",
    "flex-shrink",
    "flex-wrap",
    "float",
    "font",
    "font-family",
    "font-feature-settings",
    "font-kerning",
    "font-language-override",
    "font-size",
    "font-size-adjust",
    "font-stretch",
    "font-style",
    "font-synthesis",
    "font-variant",
    "font-variant-alternates",
    "font-variant-caps",
    "font-variant-east-asian",
    "font-variant-ligatures",
    "font-variant-numeric",
    "font-variant-position",
    "font-weight",
    "gap",
    "grid",
    "grid-area",
    "grid-auto-columns",
    "grid-auto-flow",
    "grid-auto-rows",
    "grid-column",
    "grid-column-end",
    "grid-column-gap",
    "grid-column-start",
    "grid-gap",
    "grid-row",
    "grid-row-end",
    "grid-row-gap",
    "grid-row-start",
    "grid-template",
    "grid-template-areas",
    "grid-template-columns",
    "grid-template-rows",
    "hanging-punctuation",
    "height",
    "hyphens",
    "image-rendering",
    "isolation",
    "justify-content",
    "left",
    "letter-spacing",
    "line-break",
    "line-height",
    "list-style",
    "list-style-image",
    "list-style-position",
    "list-style-type",
    "margin",
    "margin-bottom",
    "margin-left",
    "margin-right",
    "margin-top",
    "mask",
    "mask-clip",
    "mask-composite",
    "mask-image",
    "mask-mode",
    "mask-origin",
    "mask-position",
    "mask-repeat",
    "mask-size",
    "mask-type",
    "max-height",
    "max-width",
    "min-height",
    "min-width",
    "mix-blend-mode",
    "object-fit",
    "object-position",
    "opacity",
    "order",
    "orphans",
    "outline",
    "outline-color",
    "outline-offset",
    "outline-style",
    "outline-width",
    "overflow",
    "overflow-wrap",
    "overflow-x",
    "overflow-y",
    "padding",
    "padding-bottom",
    "padding-left",
    "padding-right",
    "padding-top",
    "page-break-after",
    "page-break-before",
    "page-break-inside",
    "perspective",
    "perspective-origin",
    "pointer-events",
    "position",
    "quotes",
    "resize",
    "right",
    "row-gap",
    "scroll-behavior",
    "tab-size",
    "table-layout",
    "text-align",
    "text-align-last",
    "text-combine-upright",
    "text-decoration",
    "text-decoration-color",
    "text-decoration-line",
    "text-decoration-skip",
    "text-decoration-style",
    "text-indent",
    "text-justify",
    "text-orientation",
    "text-overflow",
    "text-shadow",
    "text-transform",
    "text-underline-position",
    "top",
    "transform",
    "transform-origin",
    "transform-style",
    "transition",
    "transition-delay",
    "transition-duration",
    "transition-property",
    "transition-timing-function",
    "unicode-bidi",
    "unicode-range",
    "user-select",
    "vertical-align",
    "visibility",
    "white-space",
    "widows",
    "width",
    "word-break",
    "word-spacing",
    "word-wrap",
    "writing-mode",
    "z-index",
];

fn is_allowed_property(name: &str) -> bool {
    ALLOWED_CSS_PROPERTIES.binary_search(&name).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_table_is_sorted_for_binary_search() {
        assert!(ALLOWED_CSS_PROPERTIES.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn declarations_are_normalized_and_filtered() {
        assert_eq!(sanitize_declarations("display:none"), "display: none");
        assert_eq!(
            sanitize_declarations("COLOR: red ; behavior: url(x.htc); margin:0 auto"),
            "color: red; margin: 0 auto"
        );
        assert_eq!(
            sanitize_declarations("background:url(javascript:alert(1))"),
            ""
        );
        assert_eq!(sanitize_declarations("width: expression(alert(1))"), "");
        assert_eq!(sanitize_declarations("width: e\\78 pression(alert(1))"), "");
        assert_eq!(sanitize_declarations("background: u\\72l(x)"), "");
        // A comment separates tokens; it never joins `url` and `(` into a function.
        assert_eq!(
            sanitize_declarations("background: url/**/(x)"),
            "background: url (x)"
        );
        assert_eq!(
            sanitize_declarations("color: red !important"),
            "color: red !important"
        );
        assert_eq!(sanitize_declarations("--x: url(y)"), "");
        assert_eq!(sanitize_declarations("color: var(--x)"), "");
        assert_eq!(sanitize_declarations("a{color:red}"), "");
    }

    #[test]
    fn paint_allows_only_fragment_urls() {
        for safe in [
            "red",
            "#fff",
            "none",
            "url(#grad)",
            "url('#grad') red",
            "rgb(1, 2, 3)",
            "currentColor",
        ] {
            assert!(is_safe_paint(safe), "{safe}");
        }
        for unsafe_value in [
            "url(https://evil.example/x.svg#p)",
            "URL(\"https://evil.example/x.svg#p\")",
            "url(x.svg#p)",
            "url(#)",
            "url(#a) url(https://evil.example/)",
            r"u\72l(https://evil.example/)",
            "url(javascript:alert(1))",
            "var(--x)",
            "image(x.png)",
            "url(#a b)",
        ] {
            assert!(!is_safe_paint(unsafe_value), "{unsafe_value}");
        }
    }
}
