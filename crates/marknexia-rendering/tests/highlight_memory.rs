//! Peak heap of the regex engine on 4 MiB comments, the shapes that hold
//! the most backtracking state per code unit (a loop iteration per
//! character). The engine caps its state at `MAX_ENTRIES` entries and falls
//! back to plain text past it (HL-1), so the peak stays bounded whatever the
//! input size. One test, so no other test allocates concurrently.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use marknexia_rendering::testing::highlight_code;

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards to the system allocator and only counts sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `GlobalAlloc::dealloc`'s contract.
        unsafe { System.dealloc(pointer, layout) };
        CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract.
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

const FOUR_MIB: usize = 4 * 1024 * 1024;
/// The engine's state cap is about 128 MiB plus vector growth; the input
/// (UTF-8, UTF-16 and the output) adds about 40 MiB at 4 MiB.
const PEAK_BOUND: usize = 400 * 1024 * 1024;

#[test]
fn four_mib_comments_have_a_bounded_peak() {
    let body = "x".repeat(FOUR_MIB);
    for (language, code) in [
        ("javascript", format!("/*{body}*/")),
        ("html", format!("<!--{body}-->")),
        ("cpp", format!("/*{body}*/")),
        // Unterminated: every start position scans to the end.
        ("javascript", format!("/*{body}")),
        // Under the cap: highlighted.
        ("javascript", format!("/*{}*/", &body[..FOUR_MIB / 4])),
    ] {
        let baseline = CURRENT.load(Ordering::Relaxed);
        PEAK.store(baseline, Ordering::Relaxed);
        let started = Instant::now();
        let (html, steps) = highlight_code(&code, language);
        let elapsed = started.elapsed();
        let peak = PEAK.load(Ordering::Relaxed) - baseline;
        let highlighted = !html.starts_with("<pre><code>");
        eprintln!(
            "{language} {} B: peak {} MiB above baseline, {steps} steps, {elapsed:?}, highlighted: {highlighted}",
            code.len(),
            peak / (1024 * 1024)
        );
        assert!(peak < PEAK_BOUND, "{language}: peak {peak} bytes");
        // 4 MiB comments trip the state cap and fall back; 1 MiB fits.
        assert_eq!(highlighted, code.len() < FOUR_MIB, "{language}");
        drop(html);
    }
}
