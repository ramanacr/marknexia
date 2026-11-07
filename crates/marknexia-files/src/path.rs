//! Lexical Windows path validation and component-wise repository containment.

const MAX_PATH_BYTES: usize = 32 * 1024;

fn is_reserved_component(segment: &str) -> bool {
    if segment.ends_with([' ', '.']) {
        return true;
    }
    // Win32 strips trailing spaces and dots (U+0020, U+002E only) before the
    // extension: "COM1 .md" is COM1, "COM1\u{a0}.md" is not.
    let base = segment
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(['\u{20}', '.']);
    let upper = base.to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&upper.as_str()) {
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
        "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
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

/// The volume part of a [`GatedPath`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PathVolume {
    /// A drive letter, folded to ASCII lowercase.
    Drive(char),
    /// A UNC share; server and share are ASCII and folded to lowercase.
    Unc { server: String, share: String },
}

/// A path that passed the lexical gate for local files: a drive path or a
/// UNC share path (owner decision: UNC is allowed, as in .NET).
///
/// Accepted forms:
/// - a drive path `C:\...` or its verbatim form `\\?\C:\...`;
/// - a UNC path `\\server\share\...` or its verbatim form
///   `\\?\UNC\server\share\...`.
///
/// Refused before anything else: any `.` or `..` component, in every form,
/// so Win32 normalization cannot move a path onto another share (for
/// example `\\server\..\pipe\x`). Refused forms: `\\.\` and all other
/// `\\?\` forms (GLOBALROOT, volume GUIDs, pipes, devices), the `pipe` and
/// `mailslot` pseudo-shares (also with trailing dots or spaces, which Win32
/// strips), the `IPC$` share (also with trailing dots or spaces), server or
/// share names with non-ASCII characters, server or share names ending in
/// `.` or space, and the explicit WebDAV forms: a server name with `@`
/// (`@SSL` / `@port`) and the `DavWWWRoot` share. Server and share names may
/// not contain separators, wildcards, `:` or control characters. The
/// components after the drive root or after `server\share` must pass
/// [`CanonicalPath`] (no reserved device names such as `CON`, `NUL`, `COM1`,
/// no `:` so no alternate data streams, no wildcard or control characters).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatedPath {
    volume: PathVolume,
    /// The components below the volume, as a `c:/...` canonical path.
    below: CanonicalPath,
}

impl GatedPath {
    pub fn new(text: &str) -> Result<Self, PathError> {
        let (body, unc) = if let Some(rest) = text.strip_prefix(r"\\?\") {
            if is_drive_path(rest) {
                (rest, false)
            } else {
                match rest.get(..4) {
                    Some(prefix) if prefix.eq_ignore_ascii_case(r"UNC\") => (&rest[4..], true),
                    _ => return Err(PathError::NetworkOrDevice),
                }
            }
        } else if let Some(rest) = text.strip_prefix(r"\\").or_else(|| text.strip_prefix("//")) {
            (rest, true)
        } else {
            (text, false)
        };
        let components: Vec<&str> = body.split(['\\', '/']).collect();
        if components
            .iter()
            .any(|component| matches!(*component, "." | ".."))
        {
            return Err(PathError::Traversal);
        }
        if !unc {
            let path = CanonicalPath::new(body)?;
            let drive = path.0.chars().next().unwrap_or('c');
            let below = CanonicalPath(format!("c:{}", &path.0[2..]));
            return Ok(Self {
                volume: PathVolume::Drive(drive),
                below,
            });
        }
        let (server, share, rest) = match components.as_slice() {
            [server, share, rest @ ..] => (*server, *share, rest),
            _ => return Err(PathError::NetworkOrDevice),
        };
        // Server and share names must be ASCII, matching the ASCII-only case
        // folding used for path containment.
        let bad_name = |name: &str| {
            name.is_empty()
                || !name.is_ascii()
                || name
                    .chars()
                    .any(|c| c.is_control() || matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*'))
        };
        // Win32 strips trailing dots and spaces from a name, so "pipe." or
        // "mailslot ." would reach those pseudo-shares: refuse such names, and
        // compare the stripped name as defense in depth.
        let stripped_share = share.trim_end_matches(['.', '\u{20}']);
        if bad_name(server)
            || bad_name(share)
            || server.ends_with(['.', '\u{20}'])
            || share.ends_with(['.', '\u{20}'])
            || server.contains('@')
            || stripped_share.eq_ignore_ascii_case("pipe")
            || stripped_share.eq_ignore_ascii_case("mailslot")
            || stripped_share.eq_ignore_ascii_case("DavWWWRoot")
            // The inter-process communication share carries named pipes, not files.
            || stripped_share.eq_ignore_ascii_case("IPC$")
        {
            return Err(PathError::NetworkOrDevice);
        }
        // The components below the share follow the drive-path rules; server
        // and share are names, not files, so the device-name rules do not apply.
        let below = CanonicalPath::new(&format!("C:/{}", rest.join("/")))?;
        Ok(Self {
            volume: PathVolume::Unc {
                server: server.to_ascii_lowercase(),
                share: share.to_ascii_lowercase(),
            },
            below,
        })
    }

    pub fn volume(&self) -> &PathVolume {
        &self.volume
    }

    /// True when `self` is `root` or below it: the same volume and the
    /// component-wise [`RepositoryScope`] rule for the rest of the path.
    pub fn is_within(&self, root: &GatedPath) -> bool {
        self.volume == root.volume
            && RepositoryScope {
                root: root.below.clone(),
            }
            .contains(&self.below)
    }

    /// The number of components below the volume.
    pub fn depth(&self) -> usize {
        // `below` is `c:/...`; its first component is the placeholder drive.
        self.below.components().count().saturating_sub(1)
    }
}

fn is_drive_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}
