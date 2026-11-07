//! Strict SemVer 2.0.0 versions for `update-manifest-v1`.
//!
//! The manifest `version` must be `MAJOR.MINOR.PATCH` with an optional
//! pre-release. Build metadata (`+…`) is refused: two versions that differ
//! only in build metadata have equal precedence, so they could not be
//! ordered for the downgrade check.

use std::{cmp::Ordering, fmt};

/// Longest accepted version text.
pub const MAX_VERSION_LENGTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Identifier {
    Numeric(u64),
    Alphanumeric(String),
}

impl Ord for Identifier {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Numeric(left), Self::Numeric(right)) => left.cmp(right),
            // Numeric identifiers have lower precedence (SemVer 11.4.3).
            (Self::Numeric(_), Self::Alphanumeric(_)) => Ordering::Less,
            (Self::Alphanumeric(_), Self::Numeric(_)) => Ordering::Greater,
            (Self::Alphanumeric(left), Self::Alphanumeric(right)) => left.cmp(right),
        }
    }
}

impl PartialOrd for Identifier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A parsed version. Ordering is SemVer precedence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<Identifier>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidVersion;

impl Version {
    pub fn parse(text: &str) -> Result<Self, InvalidVersion> {
        if text.is_empty() || text.len() > MAX_VERSION_LENGTH || text.contains('+') {
            return Err(InvalidVersion);
        }
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut parts = core.split('.');
        let major = numeric(parts.next())?;
        let minor = numeric(parts.next())?;
        let patch = numeric(parts.next())?;
        if parts.next().is_some() {
            return Err(InvalidVersion);
        }
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => pre
                .split('.')
                .map(pre_release_identifier)
                .collect::<Result<_, _>>()?,
        };
        Ok(Self {
            major,
            minor,
            patch,
            pre,
        })
    }

    #[must_use]
    pub fn is_pre_release(&self) -> bool {
        !self.pre.is_empty()
    }
}

fn numeric(part: Option<&str>) -> Result<u64, InvalidVersion> {
    let part = part.ok_or(InvalidVersion)?;
    if part.is_empty()
        || !part.bytes().all(|byte| byte.is_ascii_digit())
        || (part.len() > 1 && part.starts_with('0'))
    {
        return Err(InvalidVersion);
    }
    part.parse().map_err(|_| InvalidVersion)
}

fn pre_release_identifier(part: &str) -> Result<Identifier, InvalidVersion> {
    if part.is_empty()
        || !part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(InvalidVersion);
    }
    if part.bytes().all(|byte| byte.is_ascii_digit()) {
        numeric(Some(part)).map(Identifier::Numeric)
    } else {
        Ok(Identifier::Alphanumeric(part.to_owned()))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // A pre-release has lower precedence than the release.
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)?;
        for (index, identifier) in self.pre.iter().enumerate() {
            formatter.write_str(if index == 0 { "-" } else { "." })?;
            match identifier {
                Identifier::Numeric(value) => write!(formatter, "{value}")?,
                Identifier::Alphanumeric(value) => formatter.write_str(value)?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_versions_parse_and_print_unchanged() {
        for text in [
            "0.1.0",
            "1.0.18",
            "10.20.30",
            "1.0.0-alpha",
            "1.0.0-rc.1",
            "1.0.0-x-y.7",
        ] {
            assert_eq!(Version::parse(text).expect(text).to_string(), text);
        }
    }

    #[test]
    fn loose_versions_are_refused() {
        for text in [
            "",
            "1",
            "1.0",
            "1.0.0.0",
            "v1.0.0",
            "01.0.0",
            "1.00.0",
            "1.0.0-",
            "1.0.0-01",
            "1.0.0-a..b",
            "1.0.0+build",
            " 1.0.0",
            "1.0.0 ",
            "1.0.0-é",
            "-1.0.0",
            "1.0.x",
            "99999999999999999999.0.0",
        ] {
            assert_eq!(Version::parse(text), Err(InvalidVersion), "{text:?}");
        }
    }

    #[test]
    fn precedence_follows_semver() {
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "2.0.0",
        ];
        for pair in ordered.windows(2) {
            let (low, high) = (
                Version::parse(pair[0]).unwrap(),
                Version::parse(pair[1]).unwrap(),
            );
            assert!(low < high, "{} < {}", pair[0], pair[1]);
        }
    }
}
