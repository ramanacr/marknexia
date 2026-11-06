//! Build-time update configuration.
//!
//! All values come from environment variables read by the compiler
//! (`option_env!`) when the crate is built; nothing is read at run time.
//! A default build sets none of them, so the updater reports
//! [`NOT_CONFIGURED_MESSAGE`] and never contacts a server.
//!
//! | Variable | Value |
//! | --- | --- |
//! | `MARKNEXIA_UPDATE_MANIFEST_URL` | `https://<host>/<path>/update-manifest-v1.json`; `{arch}` is replaced by `x64` or `arm64`. The signature is the same URL plus `.ed25519`. |
//! | `MARKNEXIA_UPDATE_ALLOWED_HOSTS` | Comma-separated lower-case host names (exact match) for the manifest, the artifact and every redirect. |
//! | `MARKNEXIA_UPDATE_PUBLISHER` | The release publisher's Authenticode leaf name (an extra check). |
//! | `MARKNEXIA_UPDATE_PUBLISHER_SHA256` | Comma-separated SHA-256 thumbprints (64 lower-case hex) of the release signing certificate(s), DER: the publisher identity. |
//! | `MARKNEXIA_UPDATE_PUBLIC_KEYS` | `keyId=<64 hex>[,…]`: release Ed25519 PUBLIC keys. Unset: the TEST key. |
//!
//! A configuration that trusts the TEST key (whose private seed is public)
//! is refused: such a build stays "not configured".

use crate::{
    keys::{KeyError, KeySet},
    manifest::{Architecture, HttpsUrl},
    payload::PublisherPin,
};

/// Shown when the build has no update channel (the default).
pub const NOT_CONFIGURED_MESSAGE: &str = "Updates are not configured for this build.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotConfigured {
    MissingManifestUrl,
    MissingHosts,
    MissingPublisher,
    /// No valid certificate thumbprint pins the publisher.
    MissingPublisherPin,
    /// Only the TEST key (or the TEST key among others) would be trusted.
    TestKey,
    InvalidKeys(KeyError),
    InvalidUrl,
    UnsupportedArchitecture,
}

impl NotConfigured {
    #[must_use]
    pub fn message(self) -> &'static str {
        NOT_CONFIGURED_MESSAGE
    }
}

/// A complete update channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateConfig {
    pub manifest_url: HttpsUrl,
    pub signature_url: HttpsUrl,
    pub allowed_hosts: Vec<String>,
    pub publisher: PublisherPin,
    pub keys: KeySet,
}

/// The raw build-time values.
#[derive(Clone, Copy, Debug, Default)]
pub struct BuildValues<'a> {
    pub manifest_url: Option<&'a str>,
    pub allowed_hosts: Option<&'a str>,
    pub publisher: Option<&'a str>,
    pub publisher_thumbprints: Option<&'a str>,
    pub public_keys: Option<&'a str>,
}

impl BuildValues<'static> {
    /// The values this crate was compiled with.
    #[must_use]
    pub fn embedded() -> Self {
        Self {
            manifest_url: option_env!("MARKNEXIA_UPDATE_MANIFEST_URL"),
            allowed_hosts: option_env!("MARKNEXIA_UPDATE_ALLOWED_HOSTS"),
            publisher: option_env!("MARKNEXIA_UPDATE_PUBLISHER"),
            publisher_thumbprints: option_env!("MARKNEXIA_UPDATE_PUBLISHER_SHA256"),
            public_keys: option_env!("MARKNEXIA_UPDATE_PUBLIC_KEYS"),
        }
    }
}

fn present(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

impl UpdateConfig {
    /// This build's channel for the running architecture.
    pub fn embedded() -> Result<Self, NotConfigured> {
        let architecture = Architecture::current().ok_or(NotConfigured::UnsupportedArchitecture)?;
        Self::from_values(&BuildValues::embedded(), architecture)
    }

    pub fn from_values(
        values: &BuildValues<'_>,
        architecture: Architecture,
    ) -> Result<Self, NotConfigured> {
        let template = present(values.manifest_url).ok_or(NotConfigured::MissingManifestUrl)?;
        let allowed_hosts: Vec<String> = present(values.allowed_hosts)
            .ok_or(NotConfigured::MissingHosts)?
            .split(',')
            .map(|host| host.trim().to_owned())
            .filter(|host| !host.is_empty())
            .collect();
        if allowed_hosts.is_empty() {
            return Err(NotConfigured::MissingHosts);
        }
        let name = present(values.publisher)
            .ok_or(NotConfigured::MissingPublisher)?
            .to_owned();
        let thumbprints = present(values.publisher_thumbprints)
            .and_then(PublisherPin::parse_thumbprints)
            .ok_or(NotConfigured::MissingPublisherPin)?;
        let publisher = PublisherPin { name, thumbprints };
        let keys = match present(values.public_keys) {
            Some(list) => KeySet::parse_list(list).map_err(NotConfigured::InvalidKeys)?,
            None => KeySet::test_only(),
        };
        if keys.contains_test_key() {
            return Err(NotConfigured::TestKey);
        }
        let manifest = template.replace("{arch}", architecture.asset_token());
        let manifest_url =
            HttpsUrl::parse(&manifest, &allowed_hosts).map_err(|_| NotConfigured::InvalidUrl)?;
        let signature_url = HttpsUrl::parse(&format!("{manifest}.ed25519"), &allowed_hosts)
            .map_err(|_| NotConfigured::InvalidUrl)?;
        Ok(Self {
            manifest_url,
            signature_url,
            allowed_hosts,
            publisher,
            keys,
        })
    }
}
