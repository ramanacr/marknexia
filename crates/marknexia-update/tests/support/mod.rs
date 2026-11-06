//! Shared test support. Everything here is in memory or writes plain-text
//! files only: no test downloads anything, starts an installer, or writes
//! PE bytes. ZIPs built here contain short text payloads.
#![allow(dead_code)]

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

use ed25519_compact::{KeyPair, Seed};
use marknexia_update::{
    activation::{Clock, LaunchedChild, Launcher, Nonce},
    keys::KeySet,
    manifest::{Architecture, HttpsUrl, ManifestDocument, SignatureDocument, artifact_name},
    payload::{AllowedExecutable, PayloadPolicy, Signer},
    stage::{FetchError, Fetcher, StagePlatform},
    version::Version,
};

/// The TEST signing seed (public on purpose; see `keys::TEST_PUBLIC_KEY_HEX`).
pub const TEST_SEED: [u8; 32] = *b"marknexia-update-TEST-ONLY-seed!";
/// A second, unrelated TEST seed ("wrong key" cases).
/// The seed the tests sign with, as an ordinary (non-TEST-id) key.
pub const SIGNING_TEST_SEED: [u8; 32] = *b"marknexia-update-SIGNING-TEST-k!";
pub const OTHER_TEST_SEED: [u8; 32] = *b"marknexia-update-OTHER-TEST-key!";
pub const KEY_ID: &str = "test-release-key";
pub const HOST: &str = "updates.example.test";

pub fn key_pair(seed: [u8; 32]) -> KeyPair {
    KeyPair::from_seed(Seed::new(seed))
}

/// A key set holding the signing seed's public key under [`KEY_ID`] (not the
/// committed TEST key id, so the verifier treats it as an ordinary key).
pub fn keys() -> KeySet {
    let mut keys = KeySet::new();
    let public: [u8; 32] = *key_pair(SIGNING_TEST_SEED).pk;
    keys.insert(KEY_ID, public).expect("valid key");
    keys
}

pub fn hosts() -> Vec<String> {
    vec![HOST.to_owned()]
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    marknexia_update::hex::encode(&sha2::Sha256::digest(bytes))
}

/// A valid manifest for `version` on x64 whose artifact is `artifact`.
pub fn document(version: &str, artifact: &[u8]) -> ManifestDocument {
    let parsed = Version::parse(version).expect("version");
    let name = artifact_name(&parsed, Architecture::X64);
    ManifestDocument {
        schema_version: "update-manifest-v1".to_owned(),
        version: version.to_owned(),
        channel: "portable".to_owned(),
        architecture: "x64".to_owned(),
        url: format!("https://{HOST}/releases/v{version}/{name}"),
        sha256: sha256_hex(artifact),
        byte_length: artifact.len() as u64,
        minimum_os_build: 19041,
        artifact_id: name,
        artifact_kind: "portable-zip".to_owned(),
        key_id: KEY_ID.to_owned(),
    }
}

/// Signs `bytes` with `seed`; returns the canonical signature document.
pub fn sign(bytes: &[u8], seed: [u8; 32], key_id: &str) -> Vec<u8> {
    let signature = key_pair(seed).sk.sign(bytes, None);
    SignatureDocument::new(key_id, &signature).to_canonical_json()
}

pub fn signed(document: &ManifestDocument) -> (Vec<u8>, Vec<u8>) {
    let bytes = document.to_canonical_json();
    let signature = sign(&bytes, SIGNING_TEST_SEED, KEY_ID);
    (bytes, signature)
}

// ---------------------------------------------------------------------------
// ZIP builder (text payloads only).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ZipEntry {
    pub name: String,
    pub data: Vec<u8>,
    pub deflate: bool,
    /// Overrides the recorded uncompressed size.
    pub declared_size: Option<u32>,
    pub made_by: u16,
    pub external: u32,
    pub flags: u16,
    /// Overrides the recorded CRC-32.
    pub crc: Option<u32>,
}

impl ZipEntry {
    pub fn file(name: &str, data: &[u8]) -> Self {
        Self {
            name: name.to_owned(),
            data: data.to_vec(),
            deflate: false,
            declared_size: None,
            made_by: 20,
            external: 0,
            flags: 0,
            crc: None,
        }
    }

    pub fn deflated(name: &str, data: &[u8]) -> Self {
        Self {
            deflate: true,
            ..Self::file(name, data)
        }
    }
}

pub fn zip(entries: &[ZipEntry]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for entry in entries {
        let offset = out.len() as u32;
        let stored = if entry.deflate {
            miniz_oxide::deflate::compress_to_vec(&entry.data, 6)
        } else {
            entry.data.clone()
        };
        let crc = entry
            .crc
            .unwrap_or_else(|| marknexia_update::archive::crc32(&entry.data));
        let size = entry.declared_size.unwrap_or(entry.data.len() as u32);
        let method: u16 = if entry.deflate { 8 } else { 0 };
        let name = entry.name.as_bytes();
        // Local header.
        out.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        out.extend_from_slice(&20_u16.to_le_bytes());
        out.extend_from_slice(&entry.flags.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(stored.len() as u32).to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0_u16.to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(&stored);
        // Central record.
        central.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        central.extend_from_slice(&entry.made_by.to_le_bytes());
        central.extend_from_slice(&20_u16.to_le_bytes());
        central.extend_from_slice(&entry.flags.to_le_bytes());
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&[0; 4]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(stored.len() as u32).to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 8]);
        central.extend_from_slice(&entry.external.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let central_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out
}

/// A non-executable test payload policy: the "shell" and "loader" are
/// text files, so staging tests never write a `.exe` or `.dll`.
pub fn text_policy() -> PayloadPolicy {
    PayloadPolicy {
        required_files: vec!["shell.txt".to_owned(), "loader.txt".to_owned()],
        executables: vec![
            AllowedExecutable {
                name: "shell.txt".to_owned(),
                signer: Signer::Release,
                is_shell: true,
            },
            AllowedExecutable {
                name: "loader.txt".to_owned(),
                signer: Signer::Microsoft,
                is_shell: false,
            },
        ],
    }
}

/// A valid archive for [`text_policy`].
pub fn text_payload() -> Vec<u8> {
    zip(&[
        ZipEntry::deflated("shell.txt", b"TEST shell placeholder, not an executable\n"),
        ZipEntry::file("loader.txt", b"TEST loader placeholder\n"),
        ZipEntry::deflated("docs/README.txt", &b"read me\n".repeat(64)),
    ])
}

// ---------------------------------------------------------------------------
// Fakes.
// ---------------------------------------------------------------------------

/// Serves fixed responses by URL; enforces `max_bytes` like the adapter.
#[derive(Default)]
pub struct FakeFetcher {
    pub responses: HashMap<String, Result<Vec<u8>, FetchError>>,
    pub requests: RefCell<Vec<(String, u64)>>,
}

impl FakeFetcher {
    pub fn serve(mut self, url: &str, response: Result<Vec<u8>, FetchError>) -> Self {
        self.responses.insert(url.to_owned(), response);
        self
    }
}

impl Fetcher for FakeFetcher {
    fn fetch(
        &self,
        url: &HttpsUrl,
        max_bytes: u64,
        _cancel: &AtomicBool,
    ) -> Result<Vec<u8>, FetchError> {
        let url = url.to_string();
        self.requests.borrow_mut().push((url.clone(), max_bytes));
        match self.responses.get(&url) {
            Some(Ok(bytes)) if bytes.len() as u64 > max_bytes => Err(FetchError::TooLarge),
            Some(response) => response.clone(),
            None => Err(FetchError::Http(404)),
        }
    }
}

/// Plain folders, and executable checks decided by name.
#[derive(Default)]
pub struct FakePlatform {
    pub reject: HashMap<String, String>,
    pub checked: RefCell<Vec<(PathBuf, u16, String)>>,
    pub private_dirs: RefCell<Vec<PathBuf>>,
}

impl StagePlatform for FakePlatform {
    fn create_private_dir(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir(path)?;
        self.private_dirs.borrow_mut().push(path.to_path_buf());
        Ok(())
    }

    fn check_executable(
        &self,
        path: &Path,
        executable: &AllowedExecutable,
        machine: u16,
        publisher: &str,
    ) -> Result<(), String> {
        // The check runs on the extracted file.
        assert!(path.is_file(), "{}", path.display());
        self.checked
            .borrow_mut()
            .push((path.to_path_buf(), machine, publisher.to_owned()));
        match self.reject.get(&executable.name) {
            Some(reason) => Err(reason.clone()),
            None => Ok(()),
        }
    }
}

/// A unique folder under the temp folder, removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        use std::hash::BuildHasher;
        let token = std::collections::hash_map::RandomState::new().hash_one(label);
        let path = std::env::temp_dir().join(format!(
            "marknexia-update-test-{label}-{}-{token:016x}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn nonce(byte: u8) -> Nonce {
    Nonce::from_bytes([byte; 32])
}

/// Runs on each fake-clock pause.
pub type PauseHook = RefCell<Option<Box<dyn FnMut(Duration)>>>;
/// Runs when the fake launcher starts a child.
pub type LaunchHook = RefCell<Option<Box<dyn FnMut(&Path, &Nonce, u32)>>>;

/// A fake clock: each pause advances by `step`.
pub struct FakeClock {
    pub now: Cell<Duration>,
    pub step: Duration,
    /// Runs on every pause (simulates the child acting).
    pub on_pause: PauseHook,
}

impl FakeClock {
    pub fn new(step: Duration) -> Self {
        Self {
            now: Cell::new(Duration::ZERO),
            step,
            on_pause: RefCell::new(None),
        }
    }
}

impl Clock for FakeClock {
    fn elapsed(&self) -> Duration {
        self.now.get()
    }

    fn pause(&self) {
        self.now.set(self.now.get() + self.step);
        if let Some(action) = self.on_pause.borrow_mut().as_mut() {
            action(self.now.get());
        }
    }
}

/// A fake child process.
pub struct FakeChild {
    pub pid: u32,
    pub exit: std::rc::Rc<Cell<Option<Option<i32>>>>,
    pub terminated: std::rc::Rc<Cell<bool>>,
}

impl LaunchedChild for FakeChild {
    fn id(&self) -> u32 {
        self.pid
    }

    fn exited(&mut self) -> Option<Option<i32>> {
        self.exit.get()
    }

    fn terminate(&mut self) {
        self.terminated.set(true);
        self.exit.set(Some(Some(1)));
    }
}

/// Launches a [`FakeChild`] and runs `on_launch` (which may write the
/// acknowledgement the way the new shell would).
pub struct FakeLauncher {
    pub pid: u32,
    pub fail: bool,
    pub exit: std::rc::Rc<Cell<Option<Option<i32>>>>,
    pub terminated: std::rc::Rc<Cell<bool>>,
    pub launched: RefCell<Vec<(PathBuf, String)>>,
    pub on_launch: LaunchHook,
}

impl FakeLauncher {
    pub fn new(pid: u32) -> Self {
        Self {
            pid,
            fail: false,
            exit: std::rc::Rc::new(Cell::new(None)),
            terminated: std::rc::Rc::new(Cell::new(false)),
            launched: RefCell::new(Vec::new()),
            on_launch: RefCell::new(None),
        }
    }
}

impl Launcher for FakeLauncher {
    fn launch(&self, shell: &Path, nonce: &Nonce) -> std::io::Result<Box<dyn LaunchedChild>> {
        if self.fail {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        self.launched
            .borrow_mut()
            .push((shell.to_path_buf(), nonce.to_hex()));
        if let Some(action) = self.on_launch.borrow_mut().as_mut() {
            action(shell, nonce, self.pid);
        }
        Ok(Box::new(FakeChild {
            pid: self.pid,
            exit: self.exit.clone(),
            terminated: self.terminated.clone(),
        }))
    }
}
