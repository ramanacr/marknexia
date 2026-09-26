//! Linear pre-scan that bounds attributes per tag before html5ever runs.
//!
//! html5ever 0.40.1 checks each new attribute name against every earlier
//! attribute of the same tag (`current_tag_attrs.iter().any`), so a single
//! tag with k attributes costs O(k²) in the tokenizer. The sink never sees
//! that cost, and both the depth probe and ammonia pay it: 80,000 attributes
//! (549 KB) take about 11 s.
//!
//! This scanner over-approximates the tokenizer's attribute count per tag.
//! It follows the HTML tokenizer's tag, attribute and quoting states byte by
//! byte. Where the real state depends on the tree builder, it keeps **every
//! possible state at once** (a small NFA) and keeps the highest count for each
//! state:
//!
//! * after `<title>`, `<textarea>`, `<style>`, `<xmp>`, `<iframe>`,
//!   `<noembed>`, `<noframes>`, `<script>` or `<noscript>`, the content may
//!   be markup (foreign content, or not HTML namespace) or raw text;
//! * after `<!` the construct is tracked as each kind it can still be, and
//!   each branch ends only on its own terminator:
//!   - a **comment** only on exactly `<!--`, ending at `-->` or `--!>`
//!     (or the abrupt `<!-->` / `<!--->`), per the tokenizer's
//!     comment-start, comment-end-dash, comment-end and comment-end-bang
//!     states;
//!   - a **CDATA section** only on exactly `<![CDATA[` (it is CDATA in
//!     foreign content, a bogus comment otherwise), ending at `]]>`;
//!   - a **bogus comment or doctype** (also `<?` and `</` + non-letter),
//!     ending at the first `>`.
//! * raw text ends at the first matching end tag, except `<script>`, whose
//!   escape states can skip end tags. That branch therefore never ends.
//!
//! The real tokenizer path is always one of the tracked paths, so its
//! attribute count never exceeds the scanner's. The state set is finite
//! (about a hundred states at most, usually a handful), so the scan is linear.

/// Maximum attributes on one tag accepted by the sanitizer.
pub const MAX_ATTRIBUTES_PER_TAG: usize = 256;

const RAW_TEXT_NAMES: [&[u8]; 9] = [
    b"title",
    b"textarea",
    b"style",
    b"xmp",
    b"iframe",
    b"noembed",
    b"noframes",
    b"script",
    b"noscript",
];

const ALL_RAW: u16 = (1 << RAW_TEXT_NAMES.len()) - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Data,
    TagOpen,
    EndTagOpen,
    /// Tag name so far: raw-text names still matching (`mask`) after `len`
    /// bytes. End tags never switch to raw text, so their mask is empty.
    TagName {
        mask: u16,
        len: u8,
    },
    BeforeAttrName {
        raw: Option<u8>,
    },
    AttrName {
        raw: Option<u8>,
    },
    AfterAttrName {
        raw: Option<u8>,
    },
    BeforeValue {
        raw: Option<u8>,
    },
    DoubleQuoted {
        raw: Option<u8>,
    },
    SingleQuoted {
        raw: Option<u8>,
    },
    Unquoted {
        raw: Option<u8>,
    },
    AfterQuoted {
        raw: Option<u8>,
    },
    SelfClosing {
        raw: Option<u8>,
    },
    /// Bogus comment or doctype: ends at the first `>`.
    Bogus,
    /// After `<!`, `dashes` of the `--` that opens a comment matched.
    CommentOpen {
        dashes: u8,
    },
    /// Comment body. `tail` mirrors the tokenizer state: 4 = comment start,
    /// 5 = comment start dash, 0 = comment, 1 = comment end dash,
    /// 2 = comment end, 3 = comment end bang.
    Comment {
        tail: u8,
    },
    /// After `<!`, `matched` bytes of `[CDATA[` matched.
    CdataOpen {
        matched: u8,
    },
    /// CDATA section. `tail`: 1 = `]`, 2 = `]]` (or more).
    Cdata {
        tail: u8,
    },
    /// Raw text of `RAW_TEXT_NAMES[name]` with `progress` bytes of
    /// `</name` matched.
    RawText {
        name: u8,
        progress: u8,
    },
}

const fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r')
}

/// Returns `false` when some tag may carry more than `limit` attributes.
pub(crate) fn within_attribute_limit(input: &str, limit: usize) -> bool {
    let bytes = input.as_bytes();
    let mut current: Vec<(State, usize)> = vec![(State::Data, 0)];
    let mut next: Vec<(State, usize)> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        // Fast path: in plain data only `<` changes state.
        if current.len() == 1 && current[0].0 == State::Data {
            match bytes[index..].iter().position(|&byte| byte == b'<') {
                Some(offset) => index += offset,
                None => return true,
            }
        }
        let byte = bytes[index];
        index += 1;
        next.clear();
        for &(state, count) in &current {
            if !step(state, count, byte, limit, &mut next) {
                return false;
            }
        }
        std::mem::swap(&mut current, &mut next);
    }
    true
}

fn add(next: &mut Vec<(State, usize)>, state: State, count: usize) {
    if let Some(entry) = next.iter_mut().find(|(existing, _)| *existing == state) {
        entry.1 = entry.1.max(count);
    } else {
        next.push((state, count));
    }
}

/// Start a new attribute; `false` when the tag goes over `limit`.
fn new_attribute(
    next: &mut Vec<(State, usize)>,
    raw: Option<u8>,
    count: usize,
    limit: usize,
) -> bool {
    let count = count + 1;
    if count > limit {
        return false;
    }
    add(next, State::AttrName { raw }, count);
    true
}

/// A tag ends at `>`: the tokenizer returns to data, or (for a raw-text
/// start tag in the HTML namespace) switches to raw text.
fn emit(next: &mut Vec<(State, usize)>, raw: Option<u8>) {
    add(next, State::Data, 0);
    if let Some(name) = raw {
        add(next, State::RawText { name, progress: 0 }, 0);
    }
}

fn raw_of(mask: u16, len: u8) -> Option<u8> {
    RAW_TEXT_NAMES
        .iter()
        .enumerate()
        .find(|(index, name)| mask & (1 << index) != 0 && name.len() == usize::from(len))
        .and_then(|(index, _)| u8::try_from(index).ok())
}

fn step(
    state: State,
    count: usize,
    byte: u8,
    limit: usize,
    next: &mut Vec<(State, usize)>,
) -> bool {
    match state {
        State::Data => add(
            next,
            if byte == b'<' {
                State::TagOpen
            } else {
                State::Data
            },
            0,
        ),
        State::TagOpen => match byte {
            b'/' => add(next, State::EndTagOpen, 0),
            b'!' => {
                add(next, State::Bogus, 0);
                add(next, State::CommentOpen { dashes: 0 }, 0);
                add(next, State::CdataOpen { matched: 0 }, 0);
            }
            b'?' => add(next, State::Bogus, 0),
            b'<' => add(next, State::TagOpen, 0),
            letter if letter.is_ascii_alphabetic() => add(
                next,
                State::TagName {
                    mask: match_mask(ALL_RAW, 0, letter),
                    len: 1,
                },
                0,
            ),
            _ => add(next, State::Data, 0),
        },
        State::EndTagOpen => match byte {
            b'>' => add(next, State::Data, 0),
            letter if letter.is_ascii_alphabetic() => {
                add(next, State::TagName { mask: 0, len: 1 }, 0);
            }
            _ => add(next, State::Bogus, 0),
        },
        State::TagName { mask, len } => {
            let raw = raw_of(mask, len);
            match byte {
                space if is_space(space) => add(next, State::BeforeAttrName { raw }, count),
                b'/' => add(next, State::SelfClosing { raw }, count),
                b'>' => emit(next, raw),
                other => {
                    let mask = match_mask(mask, usize::from(len), other);
                    let len = if mask == 0 {
                        len
                    } else {
                        len.saturating_add(1)
                    };
                    add(next, State::TagName { mask, len }, count);
                }
            }
        }
        State::BeforeAttrName { raw } => match byte {
            space if is_space(space) => add(next, State::BeforeAttrName { raw }, count),
            b'/' => add(next, State::SelfClosing { raw }, count),
            b'>' => emit(next, raw),
            _ => return new_attribute(next, raw, count, limit),
        },
        State::AttrName { raw } => match byte {
            space if is_space(space) => add(next, State::AfterAttrName { raw }, count),
            b'/' => add(next, State::SelfClosing { raw }, count),
            b'>' => emit(next, raw),
            b'=' => add(next, State::BeforeValue { raw }, count),
            _ => add(next, State::AttrName { raw }, count),
        },
        State::AfterAttrName { raw } => match byte {
            space if is_space(space) => add(next, State::AfterAttrName { raw }, count),
            b'/' => add(next, State::SelfClosing { raw }, count),
            b'=' => add(next, State::BeforeValue { raw }, count),
            b'>' => emit(next, raw),
            _ => return new_attribute(next, raw, count, limit),
        },
        State::BeforeValue { raw } => match byte {
            space if is_space(space) => add(next, State::BeforeValue { raw }, count),
            b'"' => add(next, State::DoubleQuoted { raw }, count),
            b'\'' => add(next, State::SingleQuoted { raw }, count),
            b'>' => emit(next, raw),
            _ => add(next, State::Unquoted { raw }, count),
        },
        State::DoubleQuoted { raw } => add(
            next,
            if byte == b'"' {
                State::AfterQuoted { raw }
            } else {
                State::DoubleQuoted { raw }
            },
            count,
        ),
        State::SingleQuoted { raw } => add(
            next,
            if byte == b'\'' {
                State::AfterQuoted { raw }
            } else {
                State::SingleQuoted { raw }
            },
            count,
        ),
        State::Unquoted { raw } => match byte {
            space if is_space(space) => add(next, State::BeforeAttrName { raw }, count),
            b'>' => emit(next, raw),
            _ => add(next, State::Unquoted { raw }, count),
        },
        State::AfterQuoted { raw } => match byte {
            space if is_space(space) => add(next, State::BeforeAttrName { raw }, count),
            b'/' => add(next, State::SelfClosing { raw }, count),
            b'>' => emit(next, raw),
            _ => return new_attribute(next, raw, count, limit),
        },
        State::SelfClosing { raw } => match byte {
            b'>' => emit(next, raw),
            // Reconsumed in the before-attribute-name state.
            space if is_space(space) => add(next, State::BeforeAttrName { raw }, count),
            b'/' => add(next, State::SelfClosing { raw }, count),
            _ => return new_attribute(next, raw, count, limit),
        },
        State::Bogus => {
            if byte == b'>' {
                add(next, State::Data, 0);
            } else {
                add(next, State::Bogus, 0);
            }
        }
        State::CommentOpen { dashes } => {
            // Not `<!--`: this branch dies; the bogus branch covers it.
            if byte == b'-' {
                if dashes == 1 {
                    add(next, State::Comment { tail: 4 }, 0);
                } else {
                    add(next, State::CommentOpen { dashes: 1 }, 0);
                }
            }
        }
        State::Comment { tail } => {
            let ends = byte == b'>' && matches!(tail, 2..=5);
            if ends {
                add(next, State::Data, 0);
            } else {
                let tail = match (tail, byte) {
                    // comment start
                    (4, b'-') => 5,
                    // comment start dash / comment end dash
                    (5 | 1, b'-') => 2,
                    // comment end: extra dashes stay, `!` enters end bang
                    (2, b'-') => 2,
                    (2, b'!') => 3,
                    // comment / comment end bang: a dash enters end dash
                    (_, b'-') => 1,
                    _ => 0,
                };
                add(next, State::Comment { tail }, 0);
            }
        }
        State::CdataOpen { matched } => {
            const OPEN: &[u8] = b"[CDATA[";
            if byte == OPEN[usize::from(matched)] {
                if usize::from(matched) + 1 == OPEN.len() {
                    add(next, State::Cdata { tail: 0 }, 0);
                } else {
                    add(
                        next,
                        State::CdataOpen {
                            matched: matched + 1,
                        },
                        0,
                    );
                }
            }
        }
        State::Cdata { tail } => {
            if byte == b'>' && tail == 2 {
                add(next, State::Data, 0);
            } else {
                let tail = match (tail, byte) {
                    (1 | 2, b']') => 2,
                    (_, b']') => 1,
                    _ => 0,
                };
                add(next, State::Cdata { tail }, 0);
            }
        }
        State::RawText { name, progress } => {
            let target = RAW_TEXT_NAMES[usize::from(name)];
            let full = target.len() + 2;
            let progress = usize::from(progress);
            if progress == full {
                // `</name` matched: the raw text ends here if the name is
                // properly terminated. Only script data may keep going.
                let ended = match byte {
                    space if is_space(space) => {
                        add(next, State::BeforeAttrName { raw: None }, 0);
                        true
                    }
                    b'/' => {
                        add(next, State::SelfClosing { raw: None }, 0);
                        true
                    }
                    b'>' => {
                        add(next, State::Data, 0);
                        true
                    }
                    _ => false,
                };
                if ended && target != b"script" {
                    return true;
                }
            }
            let matched = match progress {
                0 => byte == b'<',
                1 => byte == b'/',
                p if p < full => byte.eq_ignore_ascii_case(&target[p - 2]),
                _ => false,
            };
            let progress = if matched {
                progress + 1
            } else if byte == b'<' {
                1
            } else {
                0
            };
            add(
                next,
                State::RawText {
                    name,
                    progress: u8::try_from(progress).unwrap_or(0),
                },
                0,
            );
        }
    }
    true
}

fn match_mask(mask: u16, position: usize, byte: u8) -> u16 {
    let mut result = 0;
    for (index, name) in RAW_TEXT_NAMES.iter().enumerate() {
        if mask & (1 << index) != 0
            && name
                .get(position)
                .is_some_and(|expected| expected.eq_ignore_ascii_case(&byte))
        {
            result |= 1 << index;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag_with(attributes: usize) -> String {
        let mut tag = String::from("<p");
        for index in 0..attributes {
            tag.push_str(&format!(" a{index}"));
        }
        tag.push('>');
        tag
    }

    #[test]
    fn counts_attributes_per_tag() {
        assert!(within_attribute_limit(&tag_with(256), 256));
        assert!(!within_attribute_limit(&tag_with(257), 256));
        assert!(within_attribute_limit(&tag_with(200).repeat(1_000), 256));
        assert!(!within_attribute_limit("<p a=1 b='x' c=\"y\" d/e>", 4));
        assert!(within_attribute_limit("<p a=1 b='x' c=\"y\" d>", 4));
    }

    #[test]
    fn quoted_values_do_not_hide_or_inflate_attributes() {
        let value = " x".repeat(1_000);
        assert!(within_attribute_limit(&format!("<p title=\"{value}\">"), 4));
        assert!(within_attribute_limit(&format!("<p title='{value}'>"), 4));
        assert!(within_attribute_limit(&format!("<p>{value}</p>"), 4));
    }

    #[test]
    fn comment_and_cdata_terminators_match_the_tokenizer() {
        // Abrupt empty comments end immediately; a following tag is counted.
        for prefix in [
            "<!-->",
            "<!--->",
            "<!---->",
            "<!-- x -->",
            "<!-- x --!>",
            "<!x>",
            "<?x>",
        ] {
            assert!(
                !within_attribute_limit(&format!("{prefix}{}", tag_with(300)), 256),
                "{prefix}"
            );
            assert!(
                within_attribute_limit(&format!("{prefix}{}", tag_with(10)), 256),
                "{prefix}"
            );
        }
    }

    #[test]
    fn raw_text_misalignment_cannot_hide_a_real_tag() {
        // Scanned as markup, `y="` opens a quote that swallows the real tag
        // start; the raw-text path resumes at `</textarea>` and sees it.
        let attack = format!("<textarea><x y=\"</textarea>{}", tag_with(300));
        assert!(!within_attribute_limit(&attack, 256));
        let attack = format!(
            "<textarea><x y=\"</textarea><p a=\"X>\" {}",
            &tag_with(300)[3..]
        );
        assert!(!within_attribute_limit(&attack, 256));
        let attack = format!("<script><!--<script>\"</script>{}", tag_with(300));
        assert!(!within_attribute_limit(&attack, 256));
        let attack = format!("<!-- \" -->{}", tag_with(300));
        assert!(!within_attribute_limit(&attack, 256));
        let attack = format!("<svg><![CDATA[ \" ]]>{}", tag_with(300));
        assert!(!within_attribute_limit(&attack, 256));
        let attack = format!("<style>'</style>{}", tag_with(300));
        assert!(!within_attribute_limit(&attack, 256));
        let attack = format!("<!-- a > \" > b -->{}", tag_with(300));
        assert!(!within_attribute_limit(&attack, 256));
        let attack = format!("<!x \" >{}", tag_with(300));
        assert!(!within_attribute_limit(&attack, 256));
        // A comment does not end at `]]>`, and CDATA does not end at `-->`
        // or `--!>`.
        for attack in [
            format!("<!-- ]]> <x y=\" -->{}", tag_with(300)),
            format!("<svg><![CDATA[ --> <x y=\" ]]>{}</svg>", tag_with(300)),
            format!("<svg><![CDATA[ --!> <x y=\" ]]>{}</svg>", tag_with(300)),
            format!("<!--!> <x y=\" -->{}", tag_with(300)),
            format!("<!-- --!-> <x y=\" -->{}", tag_with(300)),
        ] {
            assert!(!within_attribute_limit(&attack, 256), "{}", &attack[..30]);
        }
    }
}
