//! Lexical Windows path validation and component-wise repository containment.

const MAX_PATH_BYTES: usize = 32 * 1024;

fn is_reserved_component(segment: &str) -> bool {
    if segment.ends_with([' ', '.']) {
        return true;
    }
    let base = segment.split('.').next().unwrap_or_default();
    let upper = base.to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL"].contains(&upper.as_str()) {
        return true;
    }
    let Some(port) = upper
        .strip_prefix("COM")
        .or_else(|| upper.strip_prefix("LPT"))
    else {
        return false;
    };
    matches!(
        port,
        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
    )
}

/// A lexically validated absolute Windows path, case-folded for containment.
///
/// Only ASCII `A`-`Z` fold (to `a`-`z`). Every non-ASCII character must match
/// exactly. Unicode case pairs can't be trusted here. Full lowercasing merges
/// look-alikes that NTFS and .NET keep distinct (KELVIN SIGN U+212A and `k`).
/// Current Unicode tables also merge pairs added after a volume's `$UpCase`
/// table was fixed at format time (Georgian Mtavruli U+1C90 and U+10D0,
/// Cherokee U+13A0 and U+AB70, Latin Extended-D U+A7C0 and U+A7C1). Either
/// would let a sibling directory pass containment. With ASCII-only folding, a
/// non-ASCII case variant that Windows treats as equal is blocked (a false
/// block), never an escape. See `compat/decisions/navigation-probe-order.md`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalPath(String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PathError {
    TooLong,
    NetworkOrDevice,
    NotAbsolute,
    InvalidComponent,
    Traversal,
}

impl CanonicalPath {
    pub fn new(input: &str) -> Result<Self, PathError> {
        if input.len() > MAX_PATH_BYTES {
            return Err(PathError::TooLong);
        }
        let normalized = input.replace('\\', "/");
        if normalized.starts_with("//") {
            return Err(PathError::NetworkOrDevice);
        }
        let source = normalized.as_bytes();
        if source.len() < 3
            || !source[0].is_ascii_alphabetic()
            || source[1] != b':'
            || source[2] != b'/'
        {
            return Err(PathError::NotAbsolute);
        }
        if normalized.chars().any(char::is_control) {
            return Err(PathError::InvalidComponent);
        }
        let mut segments = Vec::new();
        for segment in normalized[3..].split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    segments.pop().ok_or(PathError::Traversal)?;
                }
                _ if segment.contains([':', '<', '>', '"', '|', '?', '*'])
                    || is_reserved_component(segment) =>
                {
                    return Err(PathError::InvalidComponent);
                }
                _ => segments.push(segment.to_ascii_lowercase()),
            }
        }
        let drive = (source[0] as char).to_ascii_lowercase();
        let canonical = if segments.is_empty() {
            format!("{drive}:/")
        } else {
            format!("{drive}:/{}", segments.join("/"))
        };
        Ok(Self(canonical))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|part| !part.is_empty())
    }
}

#[derive(Clone, Debug)]
pub struct RepositoryScope {
    root: CanonicalPath,
}

impl RepositoryScope {
    pub fn new(root: &str) -> Result<Self, PathError> {
        Ok(Self {
            root: CanonicalPath::new(root)?,
        })
    }

    pub fn contains(&self, candidate: &CanonicalPath) -> bool {
        let root: Vec<_> = self.root.components().collect();
        let target: Vec<_> = candidate.components().collect();
        target.len() >= root.len() && root.iter().zip(target.iter()).all(|(a, b)| a == b)
    }

    /// Authorize only after the native adapter obtains `final_target` from the
    /// already-opened file handle. A path-based canonicalize/open sequence is
    /// not equivalent: a reparse point can change between those operations.
    pub fn contains_opened_target(
        &self,
        lexical_target: &CanonicalPath,
        final_target: &CanonicalPath,
    ) -> bool {
        self.contains(lexical_target) && self.contains(final_target)
    }
}
