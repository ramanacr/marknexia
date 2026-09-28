//! Large-document measurements (run in release with `--ignored
//! --test-threads=1`). Each shape is generated at 4, 16 and 50 MiB of Markdown
//! source and pushed through the Markdown parser, the sanitizer on its own
//! and the complete renderer. The report prints wall time and the peak heap
//! measured by a counting global allocator.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use marknexia_core::contracts::AppTheme;
use marknexia_markdown::{MarkdownEngine, MarkdownOptions, PulldownAdapter};
use marknexia_rendering::{DocumentRenderer, PageIdentity, RenderContext, Renderer};
use marknexia_security::{ContentPolicy, HtmlPolicy};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is forwarded unchanged to the system allocator; the
// counters are only bookkeeping.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            if new_size >= layout.size() {
                let now = CURRENT.fetch_add(new_size - layout.size(), Ordering::Relaxed)
                    + (new_size - layout.size());
                PEAK.fetch_max(now, Ordering::Relaxed);
            } else {
                CURRENT.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const MIB: usize = 1024 * 1024;

/// Repeat `unit` (cycling through its parts) until `size` bytes.
fn fill(size: usize, parts: &[&str]) -> String {
    let mut out = String::with_capacity(size + 4096);
    let mut index = 0;
    while out.len() < size {
        out.push_str(parts[index % parts.len()]);
        index += 1;
    }
    out
}

/// `generate(size)` cut to at most `size` bytes (the last block may be cut).
fn sized(generate: fn(usize) -> String, size: usize) -> String {
    let mut out = generate(size);
    let mut end = size.min(out.len());
    while !out.is_char_boundary(end) {
        end -= 1;
    }
    out.truncate(end);
    out
}

fn prose(size: usize) -> String {
    fill(
        size,
        &[
            "Marknexia renders *emphasis*, **strong text**, `inline code` and [links](docs/page.md) \
             in ordinary paragraphs. Tom &amp; Jerry wrote 20\u{a0}\u{b0}C in 2026 \u{a9}.\n\n",
            "A second paragraph with a footnote-like [reference][r] and a longer sentence that \
             wraps across the editor window so the paragraph is realistic in length.\n\n",
            "[r]: https://example.test/reference\n\n",
        ],
    )
}

fn tables(size: usize) -> String {
    let mut parts = vec!["| Name | Value | Notes |\n| --- | ---: | :--- |\n"];
    parts.extend(std::iter::repeat_n(
        "| `alpha` | 42 | a *short* note with [a link](a.md) |\n",
        40,
    ));
    parts.push("\n");
    fill(size, &parts)
}

fn code(size: usize) -> String {
    fill(
        size,
        &[
            "```cs\npublic static int Add(int a, int b) => a + b; // sum\nvar s = \"text\";\n```\n\n",
            "```sh\ncd build && make -j8 && ./run > out.log 2>&1 || echo \"fail\"\n```\n\n",
            "```\nplain <code> & text\n```\n\n",
        ],
    )
}

fn lists(size: usize) -> String {
    fill(
        size,
        &[
            "- item one\n  - nested *two*\n    - deeper `three`\n- item four\n\n",
            "1. first\n2. second with [link](x.md)\n   1. inner\n\n",
            "- [ ] task\n- [x] done\n\n",
        ],
    )
}

fn links(size: usize) -> String {
    fill(
        size,
        &[
            "See [the guide](docs/guide.md#install), <https://example.test/auto>, ",
            "![diagram](images/d.png \"title\") and [remote](https://example.test/x?y=1&z=2).\n\n",
        ],
    )
}

fn raw_html(size: usize) -> String {
    fill(size, &["<b>x</b>"])
}

fn raw_blocks(size: usize) -> String {
    fill(
        size,
        &[
            "<details>\n<summary>More</summary>\n\nHidden *markdown* content.\n\n</details>\n\n",
            "<div align=\"center\"><img src=\"logo.png\" width=\"80\"></div>\n\n",
            "Text with <kbd>Ctrl</kbd>+<kbd>C</kbd> and <sub>low</sub>.\n\n",
        ],
    )
}

fn mixed(size: usize) -> String {
    fill(
        size,
        &[
            "# Chapter heading\n\n",
            &prose(1500),
            &tables(1500),
            &code(1200),
            &lists(900),
            "> A block quote with **bold** text.\n\n",
            &links(600),
            &raw_blocks(600),
            "Inline math $a^2 + b^2$ and more text.\n\n---\n\n",
        ],
    )
}

/// The whole document inside one raw `<div>`: no top-level block boundary.
fn one_block(size: usize) -> String {
    format!("<div>\n\n{}\n\n</div>\n", prose(size - 20))
}

/// Review I3's worst Markdown shapes for the model and body size.
fn emphasis(size: usize) -> String {
    fill(size, &["*a* "])
}

fn tight_list(size: usize) -> String {
    fill(size, &["- x\n"])
}

fn one_paragraph(size: usize) -> String {
    fill(size, &["word "])
}

fn context() -> RenderContext {
    RenderContext::new(
        AppTheme::System,
        PageIdentity::new([7; 16], [9; 16]).unwrap(),
    )
}

fn measure<T>(stage: impl FnOnce() -> T) -> (T, f64, usize) {
    let base = CURRENT.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let started = Instant::now();
    let value = stage();
    let elapsed = started.elapsed().as_secs_f64();
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(base);
    (value, elapsed, peak)
}

type Shape = (&'static str, fn(usize) -> String);

const SHAPES: &[Shape] = &[
    ("prose", prose),
    ("tables", tables),
    ("code", code),
    ("lists", lists),
    ("links", links),
    ("raw-html <b>x</b>", raw_html),
    ("raw-blocks", raw_blocks),
    ("mixed", mixed),
    ("one-block", one_block),
    ("one-paragraph", one_paragraph),
    ("emphasis *a*", emphasis),
    ("tight-list - x", tight_list),
];

fn sizes() -> Vec<usize> {
    std::env::var("MARKNEXIA_SCALE_MIB")
        .ok()
        .map(|list| {
            list.split(',')
                .filter_map(|mib| mib.trim().parse::<usize>().ok())
                .map(|mib| mib * MIB)
                .collect()
        })
        .unwrap_or_else(|| vec![4 * MIB, 16 * MIB, 50 * MIB - 64])
}

fn shape_filter(name: &str) -> bool {
    std::env::var("MARKNEXIA_SCALE_SHAPES")
        .map(|filter| {
            filter
                .split(',')
                .any(|wanted| name.starts_with(wanted.trim()))
        })
        .unwrap_or(true)
}

/// Write every generated source to `MARKNEXIA_SCALE_DUMP` (text files only),
/// so the .NET renderer can be timed on byte-identical input.
#[test]
#[ignore = "writes the measurement inputs to a directory"]
fn dump_scale_inputs() {
    let Ok(directory) = std::env::var("MARKNEXIA_SCALE_DUMP") else {
        return;
    };
    for &(name, generate) in SHAPES {
        if !shape_filter(name) {
            continue;
        }
        for size in sizes() {
            let file = format!(
                "{}-{}mib.md",
                name.split_whitespace().next().unwrap_or(name),
                size.div_ceil(MIB)
            );
            std::fs::write(
                std::path::Path::new(&directory).join(file),
                sized(generate, size),
            )
            .unwrap();
        }
    }
}

#[test]
#[ignore = "large-document measurement report"]
fn scale_report() {
    let renderer = Renderer::new();
    let policy = HtmlPolicy::new(ContentPolicy::default());
    for &(name, generate) in SHAPES {
        if !shape_filter(name) {
            continue;
        }
        for size in sizes() {
            let source = sized(generate, size);
            let (parsed, parse_s, parse_peak) = measure(|| {
                PulldownAdapter
                    .parse(&source, &MarkdownOptions::default())
                    .map(|parsed| parsed.rendered_body_html)
            });
            let body = parsed.unwrap_or_default();
            let (sanitized, sanitize_s, sanitize_peak) =
                measure(|| policy.sanitize_fragment(&body).map(|f| f.as_str().len()));
            let nodes = marknexia_security::testing::chunk_ranges(&body).map(|chunks| {
                let total: usize = chunks.iter().map(|(_, nodes)| nodes).sum();
                let largest = chunks.iter().map(|(_, nodes)| *nodes).max().unwrap_or(0);
                format!(
                    "{} chunks, {:.3} nodes/B, largest {largest}",
                    chunks.len(),
                    total as f64 / body.len().max(1) as f64
                )
            });
            drop(body);
            let (rendered, render_s, render_peak) =
                measure(|| renderer.render(&source, &context()).map(|d| d.page_bytes()));
            eprintln!(
                "SCALE {name:<18} {:>5.1} MiB | parse {parse_s:>7.3}s {:>6} MiB | sanitize {sanitize_s:>7.3}s {:>6} MiB {:<24} | render {render_s:>7.3}s {:>6} MiB {}",
                source.len() as f64 / MIB as f64,
                parse_peak / MIB,
                sanitize_peak / MIB,
                format!("{:?}", sanitized.map(|n| n / MIB)),
                render_peak / MIB,
                match rendered {
                    Ok(bytes) => format!("page {:.1} MiB", bytes as f64 / MIB as f64),
                    Err(error) => format!("{error:?}"),
                },
            );
            eprintln!(
                "NODES {name:<18} {:>5.1} MiB | {nodes:?}",
                source.len() as f64 / MIB as f64
            );
        }
    }
}
