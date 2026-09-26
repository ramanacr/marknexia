//! Port of the .NET `SimpleMathRenderer`: a small, local TeX subset with no
//! font, script or stylesheet dependency. Input is the matched HTML text of a
//! math span (still entity-encoded by the Markdown layer), and, as in .NET,
//! every character is encoded again, so `&lt;` renders as the text `&lt;`.
//!
//! The .NET recursion is unbounded. Here structural nesting is limited to
//! [`MAX_MATH_DEPTH`] generated elements, and anything deeper is emitted as
//! encoded literal text (REND-5). The budget keeps math inside the
//! sanitizer's 256-level depth limit even under the deepest Markdown nesting
//! (32 block + 32 inline containers).

use crate::text::html_encode;

/// Element-nesting budget for rendered math: `^`, `_` and `\text` cost one
/// level, `\frac` and `\sqrt` two (they emit nested spans). At most
/// `MAX_MATH_DEPTH + 3` elements are ever nested, including the wrapper.
pub const MAX_MATH_DEPTH: usize = 32;

fn symbol(command: &str) -> Option<&'static str> {
    Some(match command {
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" => "ϵ",
        "varepsilon" => "ε",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" => "θ",
        "vartheta" => "ϑ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "pi" => "π",
        "varpi" => "ϖ",
        "rho" => "ρ",
        "sigma" => "σ",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" => "ϕ",
        "varphi" => "φ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        "infty" => "∞",
        "pm" => "±",
        "mp" => "∓",
        "times" => "×",
        "cdot" => "⋅",
        "div" => "÷",
        "le" | "leq" => "≤",
        "ge" | "geq" => "≥",
        "neq" | "ne" => "≠",
        "approx" => "≈",
        "equiv" => "≡",
        "to" | "rightarrow" => "→",
        "leftarrow" => "←",
        "leftrightarrow" => "↔",
        "in" => "∈",
        "notin" => "∉",
        "subset" => "⊂",
        "subseteq" => "⊆",
        "cup" => "∪",
        "cap" => "∩",
        "sum" => "∑",
        "prod" => "∏",
        "int" => "∫",
        "partial" => "∂",
        "nabla" => "∇",
        "forall" => "∀",
        "exists" => "∃",
        "land" => "∧",
        "lor" => "∨",
        "neg" => "¬",
        // .NET maps `ell` to an ellipsis too; kept for parity.
        "ldots" | "dots" | "ell" => "…",
        _ => return None,
    })
}

/// `SimpleMathRenderer.Render(expression, mode).HtmlContent`, or `None` once
/// the output would exceed `limit` bytes.
pub(crate) fn render(expression: &str, display: bool, limit: usize) -> Option<String> {
    let normalized = strip_delimiters(expression).trim();
    let chars: Vec<char> = normalized.chars().collect();
    let mut html = String::with_capacity(normalized.len() + 32);
    render_expression(&chars, 0, limit, &mut html);
    if html.len() > limit {
        return None;
    }
    let accessible = if normalized.is_empty() {
        "Mathematical expression"
    } else {
        normalized
    };
    let (tag, class) = if display {
        ("div", "marknexia-math-display")
    } else {
        ("span", "marknexia-math")
    };
    let mut out = String::with_capacity(html.len() + accessible.len() + 64);
    out.push('<');
    out.push_str(tag);
    out.push_str(" class=\"");
    out.push_str(class);
    out.push_str("\" role=\"math\" aria-label=\"");
    html_encode(accessible, &mut out);
    out.push_str("\">");
    out.push_str(&html);
    out.push_str("</");
    out.push_str(tag);
    out.push('>');
    (out.len() <= limit).then_some(out)
}

/// The .NET fallback when math rendering is disabled.
pub(crate) fn fallback(expression: &str, display: bool) -> String {
    let tag = if display { "div" } else { "span" };
    let mut out = String::with_capacity(expression.len() + 64);
    out.push('<');
    out.push_str(tag);
    out.push_str(" class=\"marknexia-math-fallback\" role=\"math\">");
    html_encode(expression, &mut out);
    out.push_str("</");
    out.push_str(tag);
    out.push('>');
    out
}

fn strip_delimiters(expression: &str) -> &str {
    for (open, close) in [("\\(", "\\)"), ("\\[", "\\]"), ("$$", "$$")] {
        if expression.starts_with(open) && expression.ends_with(close) && expression.len() >= 4 {
            return &expression[2..expression.len() - 2];
        }
    }
    expression
}

fn is_letter(character: char) -> bool {
    character.is_alphabetic()
}

fn encode_char(character: char, out: &mut String) {
    let mut buffer = [0; 4];
    html_encode(character.encode_utf8(&mut buffer), out);
}

fn encode_chars(chars: &[char], out: &mut String) {
    let text: String = chars.iter().collect();
    html_encode(&text, out);
}

fn render_expression(expression: &[char], depth: usize, limit: usize, out: &mut String) {
    if depth > MAX_MATH_DEPTH {
        encode_chars(expression, out);
        return;
    }
    let mut index = 0;
    while index < expression.len() {
        if out.len() > limit {
            return;
        }
        let current = expression[index];
        if current == '^' || current == '_' {
            let argument = read_atom(expression, &mut index);
            let tag = if current == '^' { "sup" } else { "sub" };
            out.push('<');
            out.push_str(tag);
            out.push('>');
            render_expression(argument, depth + 1, limit, out);
            out.push_str("</");
            out.push_str(tag);
            out.push('>');
            index += 1;
            continue;
        }
        if current == '\\' {
            let command = read_command(expression, &mut index);
            let name: String = command.iter().collect();
            match name.as_str() {
                "frac" | "dfrac" | "tfrac" => {
                    let numerator = read_group(expression, &mut index);
                    let denominator = read_group(expression, &mut index);
                    out.push_str(
                        "<span class=\"marknexia-fraction\"><span class=\"marknexia-numerator\">",
                    );
                    render_expression(numerator, depth + 2, limit, out);
                    out.push_str("</span><span class=\"marknexia-denominator\">");
                    render_expression(denominator, depth + 2, limit, out);
                    out.push_str("</span></span>");
                }
                "sqrt" => {
                    skip_optional_group(expression, &mut index);
                    let radicand = read_group(expression, &mut index);
                    out.push_str("<span class=\"marknexia-sqrt\"><span aria-hidden=\"true\">√</span><span class=\"marknexia-radicand\">");
                    render_expression(radicand, depth + 2, limit, out);
                    out.push_str("</span></span>");
                }
                "text" | "operatorname" => {
                    let text = read_group(expression, &mut index);
                    render_expression(text, depth + 1, limit, out);
                }
                "left" | "right" => {
                    if index + 1 < expression.len() {
                        index += 1;
                        let delimiter = expression[index];
                        if delimiter != '.' {
                            encode_char(delimiter, out);
                        }
                    }
                }
                _ if command.len() == 1 && !is_letter(command[0]) => encode_chars(command, out),
                _ => match symbol(&name) {
                    Some(symbol) => html_encode(symbol, out),
                    None => {
                        out.push('\\');
                        html_encode(&name, out);
                    }
                },
            }
            index += 1;
            continue;
        }
        if current != '{' && current != '}' {
            encode_char(current, out);
        }
        index += 1;
    }
}

/// `ReadCommand`: `index` ends on the command's last character. A trailing
/// backslash yields the command `\`, leaving `index` past the end.
fn read_command<'a>(expression: &'a [char], index: &mut usize) -> &'a [char] {
    *index += 1;
    if *index >= expression.len() {
        return &['\\'];
    }
    if !is_letter(expression[*index]) {
        return &expression[*index..=*index];
    }
    let start = *index;
    while *index + 1 < expression.len() && is_letter(expression[*index + 1]) {
        *index += 1;
    }
    &expression[start..=*index]
}

fn skip_space(expression: &[char], from: usize) -> usize {
    let mut next = from;
    while next < expression.len() && expression[next].is_whitespace() {
        next += 1;
    }
    next
}

fn read_atom<'a>(expression: &'a [char], index: &mut usize) -> &'a [char] {
    read_argument(expression, index, false)
}

fn read_group<'a>(expression: &'a [char], index: &mut usize) -> &'a [char] {
    read_argument(expression, index, true)
}

/// .NET `ReadAtom` (`group == false`) and `ReadGroup` (`group == true`),
/// which call each other: `ReadAtom` on `{` hands the *brace* position to
/// `ReadGroup`, which then inspects the character after it, so `x^{2}`
/// reads the atom `}` (a preserved .NET quirk, REND-5). The mutual recursion
/// advances at least one character per step and is unbounded in .NET
/// (`^{^{^{…` overflows the stack), so it runs as a loop here.
fn read_argument<'a>(expression: &'a [char], index: &mut usize, mut group: bool) -> &'a [char] {
    loop {
        let next = skip_space(expression, *index + 1);
        if next >= expression.len() {
            *index = expression.len().saturating_sub(1);
            return &[];
        }
        if !group {
            if expression[next] == '{' {
                *index = next;
                group = true;
                continue;
            }
            if expression[next] == '\\' {
                let mut command_end = next + 1;
                if command_end < expression.len() && is_letter(expression[command_end]) {
                    while command_end + 1 < expression.len()
                        && is_letter(expression[command_end + 1])
                    {
                        command_end += 1;
                    }
                }
                *index = command_end;
                // .NET `expression[next..(commandEnd + 1)]` on a trailing `\`
                // throws and fails the whole render; clamp instead (REND-5).
                return &expression[next..(command_end + 1).min(expression.len())];
            }
            *index = next;
            return &expression[next..=next];
        }
        if expression[next] != '{' {
            *index = next;
            group = false;
            continue;
        }
        return brace_group(expression, index, next);
    }
}

fn brace_group<'a>(expression: &'a [char], index: &mut usize, next: usize) -> &'a [char] {
    let start = next + 1;
    let mut depth = 1_usize;
    for (cursor, &character) in expression.iter().enumerate().skip(start) {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    *index = cursor;
                    return &expression[start..cursor];
                }
            }
            _ => {}
        }
    }
    *index = expression.len() - 1;
    &expression[start..]
}

fn skip_optional_group(expression: &[char], index: &mut usize) {
    let next = skip_space(expression, *index + 1);
    if next >= expression.len() || expression[next] != '[' {
        return;
    }
    let mut depth = 1_usize;
    for (cursor, &character) in expression.iter().enumerate().skip(next + 1) {
        match character {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    *index = cursor;
                    return;
                }
            }
            _ => {}
        }
    }
    *index = expression.len() - 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_all(expression: &str, display: bool) -> String {
        render(expression, display, usize::MAX).unwrap()
    }

    #[test]
    fn output_budget_stops_growth() {
        let expression = format!("\\({}\\)", "\\frac{a}{b}".repeat(10_000));
        assert!(render(&expression, false, 10_000).is_none());
        assert!(render(&expression, false, usize::MAX).is_some());
    }

    #[test]
    fn renders_the_supported_subset() {
        assert_eq!(
            render_all("\\(x^2 + \\alpha_{i}\\)", false),
            "<span class=\"marknexia-math\" role=\"math\" aria-label=\"x^2 + \\alpha_{i}\">x<sup>2</sup> + α<sub></sub></span>"
        );
        assert_eq!(
            render_all("$$\\frac{a}{b}$$", true),
            "<div class=\"marknexia-math-display\" role=\"math\" aria-label=\"\\frac{a}{b}\"><span class=\"marknexia-fraction\"><span class=\"marknexia-numerator\">a</span><span class=\"marknexia-denominator\">b</span></span></div>"
        );
        assert_eq!(
            render_all("\\(\\)", false),
            "<span class=\"marknexia-math\" role=\"math\" aria-label=\"Mathematical expression\"></span>"
        );
    }

    #[test]
    fn double_encodes_like_dotnet() {
        assert_eq!(
            render_all("\\(a &lt; b\\)", false),
            "<span class=\"marknexia-math\" role=\"math\" aria-label=\"a &amp;lt; b\">a &amp;lt; b</span>"
        );
    }

    #[test]
    fn unknown_commands_and_edges() {
        assert!(render_all("\\(\\foo \\left( x \\right. \\)", false).contains("\\foo ( x "));
        assert!(render_all("\\(x^\\)", false).contains("x<sup></sup>"));
        assert!(render_all("\\(\\sqrt[3]{y}\\)", false).contains("marknexia-radicand\">y<"));
        let _ = render_all("\\(\\\\)", false);
        let _ = render_all("\\(^\\\\)", false);
    }

    #[test]
    fn deep_nesting_is_bounded() {
        // `^{` chains are read iteratively (and, as in .NET, yield `}`).
        let chain = format!("\\({}x{}\\)", "^{".repeat(100_000), "}".repeat(100_000));
        assert_eq!(render_all(&chain, false).matches("<sup>").count(), 1);
        // Structural nesting stops at the depth limit.
        let deep = format!(
            "\\({}x{}\\)",
            "\\sqrt{".repeat(100_000),
            "}".repeat(100_000)
        );
        let html = render_all(&deep, false);
        assert_eq!(
            html.matches("marknexia-radicand").count(),
            MAX_MATH_DEPTH / 2 + 1
        );
    }
}
