//! Deterministic heading identifiers matching the .NET behavioral oracle.

use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct SlugSet {
    counts: HashMap<String, u32>,
}

impl SlugSet {
    pub fn clear(&mut self) {
        self.counts.clear();
    }
}

pub fn generate_heading_slug(heading: &str, seen: &mut SlugSet) -> String {
    if heading.trim().is_empty() {
        return String::new();
    }

    let mut clean = String::with_capacity(heading.len());
    let mut in_tag = false;
    for ch in heading.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            '*' | '_' | '`' | '~' if !in_tag => {}
            _ if !in_tag => clean.push(ch),
            _ => {}
        }
    }

    let mut base = String::with_capacity(clean.len());
    let mut pending_hyphen = false;
    for ch in clean.trim().to_lowercase().chars() {
        if ch.is_alphanumeric() || ch == '-' || ch == '_' {
            if pending_hyphen && !base.is_empty() && !base.ends_with('-') {
                base.push('-');
            }
            pending_hyphen = false;
            if ch != '-' || !base.ends_with('-') {
                base.push(ch);
            }
        } else if ch.is_whitespace() {
            pending_hyphen = true;
        }
    }
    let base = base.trim_matches('-');
    let base = if base.is_empty() { "heading" } else { base };
    let count = seen.counts.entry(base.to_owned()).or_insert(0);
    let slug = if *count == 0 {
        base.to_owned()
    } else {
        format!("{base}-{count}")
    };
    *count = count.saturating_add(1);
    slug
}
