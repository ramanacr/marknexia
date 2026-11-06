//! `update-manifest-v1`: strict parsing, Ed25519 verification and the
//! semantic checks of `docs/superpowers/specs/2026-09-12-rust-update-contract-decision.md`.
//!
//! The manifest bytes are signed exactly as published, and they must be the
//! RFC 8785 (JCS) canonical JSON of the manifest: one object, keys in
//! ascending order, no white space, minimal string escapes and integers in
//! plain decimal. Every string field is limited to printable ASCII without
//! `"` and `\`, so canonical strings need no escapes at all. A manifest is
//! accepted only if re-serializing the parsed fields reproduces the signed
//! bytes exactly, which also refuses duplicate keys, a byte-order mark,
//! white space, noncanonical numbers (`1.0`, `1e3`, `01`) and key order
//! differences. Unknown fields are refused.
//!
//! The detached signature document `update-manifest-v1.json.ed25519` is
//! canonical JSON too:
//! `{"algorithm":"Ed25519","keyId":"<id>","signature":"<128 lower-case hex>"}`.
//! The signature is checked before the manifest is parsed, so an unsigned
//! document never reaches the manifest parser.

use serde::Deserialize;

use crate::{
    hex,
    keys::{KeySet, is_valid_key_id},
    version::Version,
};

pub const SCHEMA_VERSION: &str = "update-manifest-v1";
pub const SIGNATURE_ALGORITHM: &str = "Ed25519";
pub const CHANNEL_PORTABLE: &str = "portable";
pub const ARTIFACT_KIND_PORTABLE_ZIP: &str = "portable-zip";
/// Largest accepted manifest document.
pub const MAX_MANIFEST_BYTES: usize = 8 * 1024;
/// Largest accepted signature document.
pub const MAX_SIGNATURE_BYTES: usize = 1024;
/// Longest accepted URL in a manifest.
pub const MAX_URL_LENGTH: usize = 2048;
/// Longest accepted `artifactId`.
pub const MAX_ARTIFACT_ID_LENGTH: usize = 128;

/// A Windows architecture the portable channel serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Architecture {
    X64,
    Arm64,
}

impl Architecture {
    /// The process architecture, or `None` on an unsupported target.
    #[must_use]
    pub fn current() -> Option<Self> {
        if cfg!(target_arch = "x86_64") {
            Some(Self::X64)
        } else if cfg!(target_arch = "aarch64") {
            Some(Self::Arm64)
        } else {
            None
        }
    }

    /// The manifest spelling (`x64`, `ARM64`).
    #[must_use]
    pub const fn manifest_name(self) -> &'static str {
        match self {
            Self::X64 => "x64",
            Self::Arm64 => "ARM64",
        }
    }

    /// The release asset token (.NET `CurrentArchitectureAssetToken`).
    #[must_use]
    pub const fn asset_token(self) -> &'static str {
        match self {
            Self::X64 => "x64",
            Self::Arm64 => "arm64",
        }
    }

    /// `IMAGE_FILE_MACHINE_*` of a native executable for this architecture.
    #[must_use]
    pub const fn pe_machine(self) -> u16 {
        match self {
            Self::X64 => 0x8664,
            Self::Arm64 => 0xAA64,
        }
    }

    fn from_manifest(name: &str) -> Option<Self> {
        match name {
            "x64" => Some(Self::X64),
            "ARM64" => Some(Self::Arm64),
            _ => None,
        }
    }
}

/// The exact `artifactId` of a portable ZIP (.NET asset naming).
#[must_use]
pub fn artifact_name(version: &Version, architecture: Architecture) -> String {
    format!(
        "Marknexia-v{version}-win-{}.zip",
        architecture.asset_token()
    )
}

// ---------------------------------------------------------------------------
// HTTPS URLs.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlError {
    TooLong,
    /// Not `https://host/path` in lower-case scheme and host, or it has a
    /// port, user information, query, fragment, percent escape, back slash,
    /// non-ASCII text, an empty or dot path segment.
    Malformed,
    /// The host is not on the configured allowlist.
    UntrustedHost,
}

/// A manifest or artifact location: `https://<host>/<path>` with a host on
/// the allowlist. Deliberately narrower than general URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpsUrl {
    host: String,
    path: String,
}

impl HttpsUrl {
    pub fn parse(url: &str, trusted_hosts: &[String]) -> Result<Self, UrlError> {
        if url.len() > MAX_URL_LENGTH {
            return Err(UrlError::TooLong);
        }
        if !url.bytes().all(|byte| byte.is_ascii_graphic())
            || url
                .bytes()
                .any(|byte| matches!(byte, b'\\' | b'%' | b'?' | b'#' | b'@' | b'"'))
        {
            return Err(UrlError::Malformed);
        }
        let rest = url.strip_prefix("https://").ok_or(UrlError::Malformed)?;
        let (host, path) = rest.split_at(rest.find('/').ok_or(UrlError::Malformed)?);
        let host_valid = !host.is_empty()
            && host.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'-'
            })
            && !host.starts_with(['.', '-'])
            && !host.ends_with(['.', '-'])
            && !host.contains("..");
        if !host_valid {
            return Err(UrlError::Malformed);
        }
        let segments: Vec<&str> = path[1..].split('/').collect();
        if segments
            .iter()
            .any(|segment| segment.is_empty() || *segment == "." || *segment == "..")
        {
            return Err(UrlError::Malformed);
        }
        if !trusted_hosts.iter().any(|trusted| trusted == host) {
            return Err(UrlError::UntrustedHost);
        }
        Ok(Self {
            host: host.to_owned(),
            path: path.to_owned(),
        })
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The last path segment.
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or_default()
    }
}

impl std::fmt::Display for HttpsUrl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "https://{}{}", self.host, self.path)
    }
}

// ---------------------------------------------------------------------------
// Documents.
// ---------------------------------------------------------------------------

/// The manifest fields as published. Signing tools build this, call
/// [`ManifestDocument::to_canonical_json`] and sign the result.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ManifestDocument {
    pub schema_version: String,
    pub version: String,
    pub channel: String,
    pub architecture: String,
    pub url: String,
    pub sha256: String,
    pub byte_length: u64,
    pub minimum_os_build: u64,
    pub artifact_id: String,
    pub artifact_kind: String,
    pub key_id: String,
}

impl ManifestDocument {
    /// RFC 8785 canonical JSON of these fields. Strings outside the
    /// accepted character set still serialize (JSON-escaped), but such a
    /// document is refused on verification.
    #[must_use]
    pub fn to_canonical_json(&self) -> Vec<u8> {
        canonical_object(&[
            ("architecture", Field::Text(&self.architecture)),
            ("artifactId", Field::Text(&self.artifact_id)),
            ("artifactKind", Field::Text(&self.artifact_kind)),
            ("byteLength", Field::Number(self.byte_length)),
            ("channel", Field::Text(&self.channel)),
            ("keyId", Field::Text(&self.key_id)),
            ("minimumOsBuild", Field::Number(self.minimum_os_build)),
            ("schemaVersion", Field::Text(&self.schema_version)),
            ("sha256", Field::Text(&self.sha256)),
            ("url", Field::Text(&self.url)),
            ("version", Field::Text(&self.version)),
        ])
    }
}

/// The detached signature document.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SignatureDocument {
    pub algorithm: String,
    pub key_id: String,
    pub signature: String,
}

impl SignatureDocument {
    #[must_use]
    pub fn new(key_id: &str, signature: &[u8; 64]) -> Self {
        Self {
            algorithm: SIGNATURE_ALGORITHM.to_owned(),
            key_id: key_id.to_owned(),
            signature: hex::encode(signature),
        }
    }

    #[must_use]
    pub fn to_canonical_json(&self) -> Vec<u8> {
        canonical_object(&[
            ("algorithm", Field::Text(&self.algorithm)),
            ("keyId", Field::Text(&self.key_id)),
            ("signature", Field::Text(&self.signature)),
        ])
    }
}

enum Field<'a> {
    Text(&'a str),
    Number(u64),
}

/// JCS for a flat object of strings and non-negative integers. `fields`
/// must already be in ascending key order (ASCII keys: byte order equals
/// the UTF-16 order JCS requires).
fn canonical_object(fields: &[(&str, Field<'_>)]) -> Vec<u8> {
    debug_assert!(fields.windows(2).all(|pair| pair[0].0 < pair[1].0));
    let mut out = String::from("{");
    for (index, (key, value)) in fields.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(key).unwrap_or_default());
        out.push(':');
        match value {
            Field::Text(text) => out.push_str(&serde_json::to_string(text).unwrap_or_default()),
            Field::Number(number) => out.push_str(&number.to_string()),
        }
    }
    out.push('}');
    out.into_bytes()
}

// ---------------------------------------------------------------------------
// Verification.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestError {
    /// A document is larger than its limit.
    TooLarge,
    /// Not JSON of the expected shape: a missing, unknown or duplicate
    /// field, or a value of the wrong type.
    Malformed,
    /// Valid JSON, but not the canonical bytes of its fields.
    NotCanonical,
    UnsupportedAlgorithm,
    /// The signature names a key that is not embedded in this build.
    UnknownKey,
    /// The signature does not verify with the named key.
    BadSignature,
    /// The manifest `keyId` differs from the signature's.
    KeyMismatch,
    UnsupportedSchema,
    InvalidVersion,
    /// The version is not greater than the running version (downgrade or
    /// replay).
    NotNewer,
    /// A pre-release version on the stable `portable` channel.
    PreRelease,
    /// Older than a version this installation already saw in a verified
    /// manifest (a frozen or partially replayed channel).
    Superseded,
    WrongChannel,
    /// Unknown architecture, or not the running architecture.
    WrongArchitecture,
    /// `minimumOsBuild` is above the running Windows build.
    OsTooOld,
    Url(UrlError),
    /// `artifactId` is not the exact versioned ZIP name, or the URL does
    /// not end in it.
    ArtifactMismatch,
    UnsupportedArtifactKind,
    InvalidHash,
    /// `byteLength` is zero or above the limit.
    InvalidLength,
}

/// What a manifest is checked against.
#[derive(Clone, Debug)]
pub struct Expectations<'a> {
    pub current_version: &'a Version,
    pub architecture: Architecture,
    /// The running Windows build number.
    pub os_build: u32,
    pub trusted_hosts: &'a [String],
    pub max_byte_length: u64,
    /// The highest version seen in an earlier verified manifest
    /// (`activation::read_highest_seen`); nothing lower is offered.
    pub highest_seen: Option<&'a Version>,
}

/// A verified manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub version: Version,
    pub architecture: Architecture,
    pub url: HttpsUrl,
    pub sha256: [u8; 32],
    pub byte_length: u64,
    pub minimum_os_build: u32,
    pub artifact_id: String,
    pub key_id: String,
}

/// Verifies the signature document over the manifest bytes, then parses
/// and checks the manifest. Nothing in the manifest is trusted before the
/// signature verifies.
pub fn verify(
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
    keys: &KeySet,
    expect: &Expectations<'_>,
) -> Result<Manifest, ManifestError> {
    if manifest_bytes.len() > MAX_MANIFEST_BYTES || signature_bytes.len() > MAX_SIGNATURE_BYTES {
        return Err(ManifestError::TooLarge);
    }
    let envelope: SignatureDocument =
        serde_json::from_slice(signature_bytes).map_err(|_| ManifestError::Malformed)?;
    if !printable(&envelope.algorithm)
        || !printable(&envelope.key_id)
        || !printable(&envelope.signature)
    {
        return Err(ManifestError::Malformed);
    }
    if envelope.to_canonical_json() != signature_bytes {
        return Err(ManifestError::NotCanonical);
    }
    if envelope.algorithm != SIGNATURE_ALGORITHM {
        return Err(ManifestError::UnsupportedAlgorithm);
    }
    if !is_valid_key_id(&envelope.key_id) || !keys.contains(&envelope.key_id) {
        return Err(ManifestError::UnknownKey);
    }
    let signature = hex::decode::<64>(&envelope.signature).ok_or(ManifestError::Malformed)?;
    if keys.verify(&envelope.key_id, manifest_bytes, &signature) != Some(true) {
        return Err(ManifestError::BadSignature);
    }
    let document: ManifestDocument =
        serde_json::from_slice(manifest_bytes).map_err(|_| ManifestError::Malformed)?;
    let texts = [
        &document.schema_version,
        &document.version,
        &document.channel,
        &document.architecture,
        &document.url,
        &document.sha256,
        &document.artifact_id,
        &document.artifact_kind,
        &document.key_id,
    ];
    if !texts.iter().all(|text| printable(text)) {
        return Err(ManifestError::Malformed);
    }
    if document.to_canonical_json() != manifest_bytes {
        return Err(ManifestError::NotCanonical);
    }
    if document.key_id != envelope.key_id {
        return Err(ManifestError::KeyMismatch);
    }
    check(&document, expect)
}

/// Printable ASCII without `"` and `\`: canonical without escapes.
fn printable(text: &str) -> bool {
    text.bytes()
        .all(|byte| (0x20..0x7f).contains(&byte) && byte != b'"' && byte != b'\\')
}

fn check(
    document: &ManifestDocument,
    expect: &Expectations<'_>,
) -> Result<Manifest, ManifestError> {
    if document.schema_version != SCHEMA_VERSION {
        return Err(ManifestError::UnsupportedSchema);
    }
    if document.channel != CHANNEL_PORTABLE {
        return Err(ManifestError::WrongChannel);
    }
    if document.artifact_kind != ARTIFACT_KIND_PORTABLE_ZIP {
        return Err(ManifestError::UnsupportedArtifactKind);
    }
    let version = Version::parse(&document.version).map_err(|_| ManifestError::InvalidVersion)?;
    if version.is_pre_release() {
        return Err(ManifestError::PreRelease);
    }
    if version <= *expect.current_version {
        return Err(ManifestError::NotNewer);
    }
    if expect
        .highest_seen
        .is_some_and(|highest| version < *highest)
    {
        return Err(ManifestError::Superseded);
    }
    let architecture = Architecture::from_manifest(&document.architecture)
        .filter(|architecture| *architecture == expect.architecture)
        .ok_or(ManifestError::WrongArchitecture)?;
    let minimum_os_build =
        u32::try_from(document.minimum_os_build).map_err(|_| ManifestError::OsTooOld)?;
    if minimum_os_build > expect.os_build {
        return Err(ManifestError::OsTooOld);
    }
    let sha256 = hex::decode::<32>(&document.sha256).ok_or(ManifestError::InvalidHash)?;
    if document.byte_length == 0 || document.byte_length > expect.max_byte_length {
        return Err(ManifestError::InvalidLength);
    }
    let url = HttpsUrl::parse(&document.url, expect.trusted_hosts).map_err(ManifestError::Url)?;
    if document.artifact_id.len() > MAX_ARTIFACT_ID_LENGTH
        || document.artifact_id != artifact_name(&version, architecture)
        || url.file_name() != document.artifact_id
    {
        return Err(ManifestError::ArtifactMismatch);
    }
    Ok(Manifest {
        version,
        architecture,
        url,
        sha256,
        byte_length: document.byte_length,
        minimum_os_build,
        artifact_id: document.artifact_id.clone(),
        key_id: document.key_id.clone(),
    })
}
