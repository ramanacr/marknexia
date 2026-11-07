//! Renders a Markdown file headless and writes the complete page, for
//! inspecting text diagrams in a browser:
//!
//! ```text
//! cargo run -p marknexia-rendering --example text_diagram_preview -- <in.md> <out.html> [--no-text-diagrams]
//! ```
//!
//! The page is the trusted template around the sanitized body, as the shell
//! hosts it. Opened from a file, the bridge script's host messages do nothing.

use marknexia_core::contracts::AppTheme;
use marknexia_rendering::{DocumentRenderer, PageIdentity, RenderContext, Renderer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let (Some(input), Some(output)) = (arguments.next(), arguments.next()) else {
        return Err("usage: text_diagram_preview <in.md> <out.html> [--no-text-diagrams]".into());
    };
    let source = std::fs::read_to_string(&input)?;
    let mut context = RenderContext::new(AppTheme::System, PageIdentity::new([1; 16], [2; 16])?);
    context.enable_text_diagrams = arguments.next().as_deref() != Some("--no-text-diagrams");
    let document = Renderer::new().render(&source, &context)?;
    std::fs::write(&output, document.page_html())?;
    println!(
        "{output}: {} bytes, {} text diagram(s)",
        document.page_bytes(),
        document.text_diagrams()
    );
    Ok(())
}
