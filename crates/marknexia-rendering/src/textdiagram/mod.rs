//! Text diagrams: ASCII-art and box-drawing diagrams in code blocks drawn
//! as inline SVG (a Rust-edition feature; `compat/decisions/text-diagrams.md`,
//! TDG-1 to TDG-10).
//!
//! A block is converted when its fence is tagged `bob`, `svgbob`, `ascii`
//! or `diagram`, or when it is untagged or `text` and [`detect::is_diagram`]
//! accepts it. The block becomes a diagram shell with the same toolbar as a
//! Mermaid diagram: the bridge's copy, source, zoom and full-window
//! controls act on it because it carries the `marknexia-mermaid` class (it
//! has no `.mermaid` element, so the Mermaid runtime never touches it).
//!
//! The drawing is an SVG of `<path>` elements under a text layer that keeps
//! the characters that were not drawn, on the same `1ch` by `2ch` grid. The
//! shell is built from fixed template text, the generated SVG and
//! entity-encoded source text, and it reaches the page only through the
//! sanitizer with the rest of the body; its elements and attributes are all
//! in the existing allowlist.

mod detect;
mod draw;
mod grid;

use crate::text::{escape_data_string, escaped_data_string_len, html_encode, html_encoded_len};

pub(crate) use detect::Features;

/// Largest block converted, in bytes of decoded source (TDG-5).
pub(crate) const MAX_BLOCK_BYTES: usize = 64 * 1024;
/// Most lines converted per block.
pub(crate) const MAX_BLOCK_LINES: usize = 400;
/// Widest line converted, in cells.
pub(crate) const MAX_BLOCK_COLUMNS: usize = 512;
/// Most blocks converted per document; later blocks stay code blocks.
pub(crate) const MAX_DOCUMENT_DIAGRAMS: usize = 64;
/// Largest shell markup for one block; a larger result stays a code block.
pub(crate) const MAX_BLOCK_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
/// Most shell markup for one document; later blocks stay code blocks.
pub(crate) const MAX_DOCUMENT_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Converts the eligible code blocks of one document within the
/// per-document limits.
#[derive(Debug)]
pub(crate) struct Converter {
    enabled: bool,
    converted: usize,
    output_bytes: usize,
}

impl Converter {
    pub(crate) const fn new(enabled: bool) -> Self {
        Self {
            enabled,
            converted: 0,
            output_bytes: 0,
        }
    }

    /// Blocks converted so far.
    pub(crate) const fn converted(&self) -> usize {
        self.converted
    }

    /// The diagram shell for `code`, or `None` to keep it a code block:
    /// conversion is off, the fence is not eligible, the heuristic rejects
    /// an untagged block, or a limit is reached. `cancel` is polled once
    /// before an eligible block is classified.
    pub(crate) fn convert(
        &mut self,
        code: &str,
        language: &str,
        cancel: &(dyn Fn() -> bool + Sync),
    ) -> Result<Option<String>, Cancelled> {
        if !self.enabled || self.converted >= MAX_DOCUMENT_DIAGRAMS {
            return Ok(None);
        }
        let Some(drawing) = drawing(code, language, cancel)? else {
            return Ok(None);
        };
        let id = format!("marknexia-text-diagram-{}", self.converted + 1);
        let size = shell_len(code, &id, &drawing);
        if size > MAX_BLOCK_OUTPUT_BYTES || self.output_bytes + size > MAX_DOCUMENT_OUTPUT_BYTES {
            return Ok(None);
        }
        let shell = shell(code, &id, &drawing);
        debug_assert_eq!(shell.len(), size);
        self.converted += 1;
        self.output_bytes += shell.len();
        Ok(Some(shell))
    }
}

/// The render was cancelled before a conversion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Cancelled;

/// The classified drawing of an eligible block within the block limits.
fn drawing(
    code: &str,
    language: &str,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Drawing>, Cancelled> {
    let eligibility = detect::eligibility(language);
    if eligibility == detect::Eligibility::No
        || code.len() > MAX_BLOCK_BYTES
        || code.trim().is_empty()
    {
        return Ok(None);
    }
    if cancel() {
        return Err(Cancelled);
    }
    let Ok(grid) = grid::Grid::new(code, MAX_BLOCK_LINES, MAX_BLOCK_COLUMNS) else {
        return Ok(None);
    };
    let marks = grid::classify(&grid);
    if eligibility == detect::Eligibility::Heuristic && !detect::is_diagram(code, &grid, &marks) {
        return Ok(None);
    }
    let draw::Drawing { svg, text } = draw::draw(&grid, &marks);
    Ok(Some(Drawing {
        svg,
        text,
        columns: grid.columns.max(1),
        rows: grid.rows.len(),
    }))
}

struct Drawing {
    svg: String,
    text: String,
    columns: usize,
    rows: usize,
}

/// Whether the heuristic accepts an untagged block, and what it measured
/// (test hook).
pub(crate) fn detect_untagged(code: &str) -> (bool, Option<Features>) {
    if code.len() > MAX_BLOCK_BYTES {
        return (false, None);
    }
    let Ok(grid) = grid::Grid::new(code, MAX_BLOCK_LINES, MAX_BLOCK_COLUMNS) else {
        return (false, None);
    };
    let marks = grid::classify(&grid);
    let features = detect::features(&grid, &marks);
    (
        !detect::looks_like_code_or_output(code) && detect::accepts(&features),
        Some(features),
    )
}

/// The shell around a drawing; each piece is filled in [`shell`] in this
/// order.
const SHELL_OPEN: &str =
    "\n<div class=\"marknexia-diagram marknexia-mermaid marknexia-text-diagram\" id=\"";
const SHELL_TOOLBAR: &str = "\">
  <div class=\"marknexia-diagram-toolbar\">
    <span class=\"marknexia-diagram-label\">Diagram</span>
    <div class=\"marknexia-diagram-actions\">
      <button type=\"button\" class=\"marknexia-btn marknexia-btn-copy\" data-marknexia-action=\"copy\" data-copy-text=\"";
const SHELL_ACTIONS: &str = "\" title=\"Copy Diagram Source\">
        Copy
      </button>
      <button type=\"button\" class=\"marknexia-btn marknexia-btn-toggle\" data-marknexia-action=\"toggle-source\" title=\"Toggle Diagram Source\">
        Source
      </button>
      <button type=\"button\" class=\"marknexia-btn\" data-marknexia-action=\"zoom-out\" title=\"Zoom out\" aria-label=\"Zoom out\">\u{2212}</button>
      <button type=\"button\" class=\"marknexia-btn marknexia-diagram-zoom-reset\" data-marknexia-action=\"zoom-reset\" title=\"Reset diagram zoom\" aria-label=\"Reset diagram zoom\">100%</button>
      <button type=\"button\" class=\"marknexia-btn\" data-marknexia-action=\"zoom-in\" title=\"Zoom in\" aria-label=\"Zoom in\">+</button>
      <button type=\"button\" class=\"marknexia-btn\" data-marknexia-action=\"expand\" title=\"Open full-window diagram\" aria-label=\"Open diagram full window\">\u{26f6}</button>
    </div>
  </div>
  <div class=\"marknexia-diagram-viewport\" id=\"";
const SHELL_CANVAS: &str = "-viewport\">
    <div class=\"marknexia-diagram-canvas\"><div class=\"marknexia-text-diagram-figure\" role=\"img\" aria-label=\"";
const SHELL_SIZE: &str = "\" style=\"width: ";
const SHELL_HEIGHT: &str = "ch; height: ";
const SHELL_FIGURE: &str = "ch\">";
const SHELL_TEXT: &str = "<div class=\"marknexia-text-diagram-text\" aria-hidden=\"true\">";
const SHELL_SOURCE: &str = "</div></div></div>
  </div>
  <div class=\"marknexia-diagram-zoom-status\" data-marknexia-zoom-status aria-live=\"polite\">100%</div>
  <div class=\"marknexia-diagram-source\" id=\"";
const SHELL_SOURCE_CODE: &str = "-source\" style=\"display: none;\">
    <pre><code>";
const SHELL_CLOSE: &str = "</code></pre>
  </div>
</div>";

fn decimal_len(value: usize) -> usize {
    value
        .checked_ilog10()
        .map_or(1, |digits| digits as usize + 1)
}

fn shell_len(code: &str, id: &str, drawing: &Drawing) -> usize {
    SHELL_OPEN.len()
        + id.len()
        + SHELL_TOOLBAR.len()
        + escaped_data_string_len(code)
        + SHELL_ACTIONS.len()
        + id.len()
        + SHELL_CANVAS.len()
        + html_encoded_len(code)
        + SHELL_SIZE.len()
        + decimal_len(drawing.columns)
        + SHELL_HEIGHT.len()
        + decimal_len(drawing.rows * 2)
        + SHELL_FIGURE.len()
        + drawing.svg.len()
        + SHELL_TEXT.len()
        + drawing.text.len()
        + SHELL_SOURCE.len()
        + id.len()
        + SHELL_SOURCE_CODE.len()
        + html_encoded_len(code)
        + SHELL_CLOSE.len()
}

fn shell(code: &str, id: &str, drawing: &Drawing) -> String {
    let mut out = String::with_capacity(shell_len(code, id, drawing));
    out.push_str(SHELL_OPEN);
    out.push_str(id);
    out.push_str(SHELL_TOOLBAR);
    escape_data_string(code, &mut out);
    out.push_str(SHELL_ACTIONS);
    out.push_str(id);
    out.push_str(SHELL_CANVAS);
    html_encode(code, &mut out);
    out.push_str(SHELL_SIZE);
    out.push_str(&drawing.columns.to_string());
    out.push_str(SHELL_HEIGHT);
    out.push_str(&(drawing.rows * 2).to_string());
    out.push_str(SHELL_FIGURE);
    out.push_str(&drawing.svg);
    out.push_str(SHELL_TEXT);
    out.push_str(&drawing.text);
    out.push_str(SHELL_SOURCE);
    out.push_str(id);
    out.push_str(SHELL_SOURCE_CODE);
    html_encode(code, &mut out);
    out.push_str(SHELL_CLOSE);
    out
}

/// The page CSS for text diagrams. The template adds it after the bundled
/// stylesheet only when a page has a text diagram, so pages without one
/// keep the .NET bytes. Strokes use `currentColor`, which follows the
/// theme's foreground colour and the forced colours of high-contrast mode.
pub(crate) const TEXT_DIAGRAM_CSS: &str = "
/* Text diagrams (Rust edition, TDG-7) */
.marknexia-text-diagram .marknexia-diagram-canvas {
  font-family: var(--font-mono);
  font-size: 13px;
  font-variant-ligatures: none;
  font-feature-settings: \"calt\" 0, \"liga\" 0;
}
.marknexia-text-diagram-figure {
  position: relative;
  color: var(--color-fg-default);
  line-height: 2ch;
}
.marknexia-text-diagram-svg {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  height: 100%;
  overflow: visible;
  pointer-events: none;
}
.marknexia-text-diagram-svg path {
  fill: none;
  stroke: currentColor;
  stroke-width: 1.25;
  stroke-linecap: square;
}
.marknexia-text-diagram-svg .tdg-heavy {
  stroke-width: 2.5;
}
.marknexia-text-diagram-svg .tdg-double {
  stroke-width: 1;
}
.marknexia-text-diagram-svg .tdg-dashed {
  stroke-dasharray: 3 3;
  stroke-linecap: butt;
}
.marknexia-text-diagram-svg .tdg-head {
  fill: currentColor;
  stroke: none;
}
.markdown-body .marknexia-text-diagram-text {
  position: absolute;
  top: 0;
  left: 0;
  margin: 0;
  padding: 0;
  white-space: pre;
  font: inherit;
  line-height: 2ch;
  color: inherit;
}
@media (forced-colors: active) {
  .marknexia-text-diagram-figure {
    color: CanvasText;
  }
}";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_size_is_exact() {
        let code = "+--> a & \"b\" é\n|\nv\n";
        let drawing = drawing(code, "diagram", &|| false).unwrap().unwrap();
        for id in ["marknexia-text-diagram-1", "marknexia-text-diagram-64"] {
            assert_eq!(
                shell(code, id, &drawing).len(),
                shell_len(code, id, &drawing)
            );
        }
        assert_eq!(decimal_len(0), 1);
        assert_eq!(decimal_len(9), 1);
        assert_eq!(decimal_len(10), 2);
        assert_eq!(decimal_len(400), 3);
    }

    impl Converter {
        fn convert_now(&mut self, code: &str, language: &str) -> Option<String> {
            self.convert(code, language, &|| false).unwrap()
        }
    }

    #[test]
    fn converter_polls_cancellation_only_for_eligible_blocks() {
        let mut converter = Converter::new(true);
        assert_eq!(converter.convert("a", "rust", &|| true), Ok(None));
        assert_eq!(converter.convert("--->", "bob", &|| true), Err(Cancelled));
    }

    #[test]
    fn converter_limits() {
        let mut converter = Converter::new(true);
        assert!(converter.convert_now("a --> b", "rust").is_none());
        assert!(converter.convert_now("   \n", "diagram").is_none());
        assert!(converter.convert_now("a\nb", "diagram").is_some());
        let too_long = "-\n".repeat(MAX_BLOCK_LINES + 1);
        assert!(converter.convert_now(&too_long, "diagram").is_none());
        let too_wide = "-".repeat(MAX_BLOCK_COLUMNS + 1);
        assert!(converter.convert_now(&too_wide, "diagram").is_none());
        // 140 lines of 500 cells: within the line and column limits.
        let too_big = format!("{}\n", "x".repeat(500)).repeat(140);
        assert!(too_big.len() > MAX_BLOCK_BYTES);
        assert!(converter.convert_now(&too_big, "diagram").is_none());
        for _ in 1..MAX_DOCUMENT_DIAGRAMS {
            assert!(converter.convert_now("--->", "bob").is_some());
        }
        assert_eq!(converter.converted(), MAX_DOCUMENT_DIAGRAMS);
        assert!(converter.convert_now("--->", "bob").is_none());
        let mut disabled = Converter::new(false);
        assert!(disabled.convert_now("--->", "bob").is_none());
    }
}
