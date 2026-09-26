//! Cost model for html5ever's serializer escaping.
//!
//! html5ever 0.40.1 `HtmlSerializer::write_escaped` (used by ammonia's
//! `Document::write_to`) finds the next character to escape in two steps.
//! First `memchr3(quote-or-'<', '<', '>')` over the **whole remaining** text
//! node or attribute value, then `memchr2('&', 0xC2)` inside that prefix.
//! Every `&` or `0xC2` byte (the lead byte of U+0080..U+00BF, including
//! no-break space) therefore rescans up to the next `<`/`>` (and `"` in
//! attribute values). That is quadratic: `"x&y ".repeat(n)` at 2 MiB takes
//! about 26 s.
//!
//! No newer html5ever or ammonia release exists, and ammonia exposes its DOM
//! only behind `cfg(ammonia_unstable)`, so the serializer cannot be replaced
//! from this crate. Instead, the exact scan length is computed in linear time
//! and bounded before serializing. For every text node (tracked by the depth
//! probe, which merges text exactly as rcdom does) and every emitted attribute
//! value (measured in ammonia's attribute filter), it adds the distance from
//! each `&`/`0xC2` byte to the next terminator.

/// Maximum total bytes the serializer's escape scans may cover. Calibrated
/// against html5ever's measured scan throughput; see the SAN-8 decision entry
/// for the serializer time at this threshold.
pub(crate) const MAX_ESCAPE_SCAN_BYTES: u64 = 1 << 34;

/// Incremental scan-length counter for one text node or attribute value.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct EscapeScan {
    pending: u64,
    cost: u64,
}

impl EscapeScan {
    /// Append `bytes` to the value and return the added scan cost.
    pub(crate) fn feed(&mut self, bytes: &[u8], attribute: bool) -> u64 {
        let before = self.cost;
        for &byte in bytes {
            self.cost = self.cost.saturating_add(self.pending);
            match byte {
                b'&' | 0xC2 => self.pending += 1,
                b'<' | b'>' => self.pending = 0,
                b'"' if attribute => self.pending = 0,
                _ => {}
            }
        }
        self.cost - before
    }
}

/// Scan cost of one complete attribute value.
pub(crate) fn attribute_cost(value: &str) -> u64 {
    EscapeScan::default().feed(value.as_bytes(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_is_the_distance_from_each_special_to_the_next_terminator() {
        let mut text = EscapeScan::default();
        assert_eq!(text.feed(b"a&bc<d", false), 3);
        assert_eq!(text.feed(b"&&", false), 1);
        assert_eq!(text.feed(b"xy", false), 4);
        assert_eq!(attribute_cost("&x\"&y"), 2 + 1);
        assert_eq!(attribute_cost("plain"), 0);
    }
}
