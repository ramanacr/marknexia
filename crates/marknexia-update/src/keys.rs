//! The embedded Ed25519 public keys that may sign `update-manifest-v1`.
//!
//! Only public keys are ever embedded. A build uses the committed TEST key
//! unless a release key set is supplied at build time through
//! `MARKNEXIA_UPDATE_PUBLIC_KEYS` (see `config`). The TEST key's private
//! seed is public (it is in the test sources), so the updater never treats
//! a build that only knows the TEST key as configured.

use ed25519_compact::{PublicKey, Signature};

/// Key id of the committed TEST key. Never a release key.
pub const TEST_KEY_ID: &str = "marknexia-update-TEST-ONLY";

/// Public half of the TEST key: Ed25519 from the 32-byte seed
/// `b"marknexia-update-TEST-ONLY-seed!"` (published in
/// `tests/support/mod.rs`). Anyone can sign with it; it exists so that
/// default builds and tests have a key, not to protect anything.
pub const TEST_PUBLIC_KEY_HEX: &str =
    "b5112fcf9ef3858f563af28366b13c1a522ce13d36bfb2237c7f013498c15ff9";

/// Longest accepted key id.
pub const MAX_KEY_ID_LENGTH: usize = 64;

/// The keys a manifest signature may name, by `keyId`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeySet {
    keys: Vec<(String, [u8; 32])>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// The key id is empty, too long, or not `[A-Za-z0-9._-]`.
    InvalidId,
    /// Not 64 lower-case hex characters, or not a valid Ed25519 point.
    InvalidKey,
    DuplicateId,
}

impl KeySet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The committed TEST key only.
    #[must_use]
    pub fn test_only() -> Self {
        let mut keys = Self::new();
        keys.insert_hex(TEST_KEY_ID, TEST_PUBLIC_KEY_HEX)
            .expect("the committed TEST key is valid");
        keys
    }

    pub fn insert(&mut self, id: &str, key: [u8; 32]) -> Result<(), KeyError> {
        if !is_valid_key_id(id) {
            return Err(KeyError::InvalidId);
        }
        // A noncanonical encoding, a non-point or a small-order (weak) point
        // is refused before it is stored.
        if PublicKey::from_slice(&key)
            .and_then(|key| key.validate())
            .is_err()
        {
            return Err(KeyError::InvalidKey);
        }
        if self.keys.iter().any(|(known, _)| known == id) {
            return Err(KeyError::DuplicateId);
        }
        self.keys.push((id.to_owned(), key));
        Ok(())
    }

    pub fn insert_hex(&mut self, id: &str, hex: &str) -> Result<(), KeyError> {
        let key = crate::hex::decode::<32>(hex).ok_or(KeyError::InvalidKey)?;
        self.insert(id, key)
    }

    /// Parses `id=hex[,id=hex…]` (the build-time variable format).
    pub fn parse_list(text: &str) -> Result<Self, KeyError> {
        let mut keys = Self::new();
        for item in text.split(',') {
            let (id, hex) = item.trim().split_once('=').ok_or(KeyError::InvalidKey)?;
            keys.insert_hex(id.trim(), hex.trim())?;
        }
        Ok(keys)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.keys.iter().any(|(known, _)| known == id)
    }

    /// True when the TEST key is the only key, or is present at all: a
    /// release build must not trust a key whose private half is public.
    #[must_use]
    pub fn contains_test_key(&self) -> bool {
        self.contains(TEST_KEY_ID)
            || self
                .keys
                .iter()
                .any(|(_, key)| crate::hex::encode(key) == TEST_PUBLIC_KEY_HEX)
    }

    /// Verifies `signature` over `message` with the key named `id`.
    pub(crate) fn verify(&self, id: &str, message: &[u8], signature: &[u8; 64]) -> Option<bool> {
        let (_, key) = self.keys.iter().find(|(known, _)| known == id)?;
        let key = PublicKey::from_slice(key).ok()?;
        Some(key.verify(message, &Signature::new(*signature)).is_ok())
    }
}

#[must_use]
pub fn is_valid_key_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_KEY_ID_LENGTH
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
