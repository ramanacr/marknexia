use marknexia_core::slug::{SlugSet, generate_heading_slug};

#[test]
fn formatted_duplicate_headings_match_frozen_oracle() {
    let mut seen = SlugSet::default();
    assert_eq!(
        generate_heading_slug("Hello *World*", &mut seen),
        "hello-world"
    );
    assert_eq!(
        generate_heading_slug("Hello World", &mut seen),
        "hello-world-1"
    );
    assert_eq!(
        generate_heading_slug("HELLO WORLD", &mut seen),
        "hello-world-2"
    );
}

#[test]
fn normalization_preserves_unicode_letters_digits_and_underscores() {
    let mut seen = SlugSet::default();
    assert_eq!(
        generate_heading_slug("  Café 42_test  ", &mut seen),
        "café-42test"
    );
    assert_eq!(generate_heading_slug("***", &mut seen), "heading");
    assert_eq!(generate_heading_slug("***", &mut seen), "heading-1");
}

#[test]
fn reset_and_independent_sets_are_deterministic() {
    let mut first = SlugSet::default();
    let mut second = SlugSet::default();
    assert_eq!(generate_heading_slug("A B", &mut first), "a-b");
    assert_eq!(generate_heading_slug("A B", &mut first), "a-b-1");
    assert_eq!(generate_heading_slug("A B", &mut second), "a-b");
    first.clear();
    assert_eq!(generate_heading_slug("A B", &mut first), "a-b");
}
