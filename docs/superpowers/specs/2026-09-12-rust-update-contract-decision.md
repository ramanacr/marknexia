# Rust portable update contract decision

Status: **contract approved for implementation, deployment blocked**. This document does not create signing keys, enable an updater, or change the existing .NET updater. Native verification and key custody need separate security approval.

## `update-manifest-v1`

Publish an immutable UTF-8 `update-manifest-v1.json` and a detached `update-manifest-v1.json.ed25519` signature for each release. The signed bytes are RFC 8785 canonical JSON of the manifest with no embedded signature field. Reject duplicate JSON keys, unknown fields, noncanonical numbers, unsupported algorithms, and signatures whose `keyId` is not in the embedded public-key set. The initial production public key and custody process are deliberately unset until the security approval; no unsigned fallback is allowed.

Required manifest fields are `schemaVersion` (`update-manifest-v1`), `version` (strict SemVer), `channel` (`portable`), `architecture` (`x64` or `ARM64`), `url` (HTTPS release asset on the configured trusted origin), `sha256` (64 lowercase hex characters), `byteLength` (positive integer), `minimumOsBuild` (Windows build integer), `artifactId` (exact versioned ZIP filename), `artifactKind` (`portable-zip`), and `keyId`. A release has one manifest per architecture. Bind the signature to all fields, including URL and artifact identity. Reject redirects outside the trusted origin, downgrade or equal version, an architecture or OS mismatch, and a mismatch between URL basename and `artifactId`.

After Ed25519 verification, download into a nonexecuting temporary file with a byte limit equal to `byteLength`; verify final length and SHA-256 before unpacking. Validate ZIP entries against traversal, absolute paths, links, duplicate names, and unexpected executables. Validate the staged native executable's PE architecture and the Authenticode signature/publisher of every signed executable and DLL against the release publisher policy. Hash and Authenticode checks are both mandatory. A checksum file alone never authorizes an update.

## Activation and rollback

Stage verified payloads under a versioned, architecture-specific directory without modifying the active version. Close files, atomically switch a small active-version pointer, and start the new native shell with `--post-update-health <nonce>` supplied through a protected local channel. Accept the nonce-bound acknowledgement only from that launch after shell readiness, WebView2 readiness, and a bundled offline document render have succeeded. The deadline is 60 seconds from process launch. A crash, timeout, wrong nonce, incomplete probe, missing runtime, or interrupted switch restores the previous pointer and records a diagnostic. Keep the previous known-good payload until the next successful update; cleanup must not erase the only rollback target. Recovery on the next launch resolves an interrupted transaction before starting either version.

Store/MSIX installation and updates remain Store or package-manager managed. The portable updater must refuse to run from an MSIX identity and must never mutate Store-managed files. The user must be able to dismiss or defer an available portable update. Download, signature failure, staged-file tampering, interruption, health timeout, rollback, and native ARM64 behavior are required executable tests before enabling this channel.

## Implementation notes (2026-11-07, proposed — requires Task 8 approval)

The implementation and its differences are in `compat/decisions/portable-updater.md` (UPD-1 to UPD-14). Open points against this contract:

* **No expiry (freeze gap).** `update-manifest-v1` has no expiry, so a party that keeps serving an old, validly signed manifest can freeze an installation on that version. The implementation refuses pre-releases on the `portable` channel and keeps a highest-seen floor, so a partial replay cannot offer a version older than one already seen, but it cannot detect a freeze. Proposal for `update-manifest-v2`: a signed `expires` field (UTC, RFC 3339); a manifest past it is refused.
* **Nonce channel.** The nonce is passed on the command line (`--post-update-health <nonce>`), not through a protected local channel. Only its SHA-256 is stored, and the acknowledgement must come from the launched process. Task 8 accepts this or requires a private channel (for example an inherited anonymous pipe).
* **Publisher policy.** The release publisher is pinned by the SHA-256 thumbprints of its signing certificate, supplied at build time; the leaf name is checked in addition.