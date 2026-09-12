# Comprehensive Software Release & Packaging Protocol

When tasked with creating, preparing, or updating a software release (GitHub Release, Store package, or binary distribution), you MUST adhere to the following end-to-end release protocol. Do not skip any phase.

## 1. Multi-Target Version Synchronization
- **Single Source of Truth**: Update the canonical version across ALL manifest and metadata layers simultaneously:
  - Project configuration (e.g., `Directory.Build.props`, `package.json`, `Cargo.toml`, `pyproject.toml`)
  - Platform package manifests (e.g., `Package.appxmanifest`, `Info.plist`, Android manifests)
  - Binary assembly metadata (`ProductVersion`, `FileVersion`, `Company`, `Copyright`)
- **Git Tagging**: Create an annotated Git tag matching the SemVer format (e.g., `v1.0.2`). Ensure tags are pushed to the remote repository (`git push origin main --tags`).

## 2. Pre-Release Verification & Sanity Checks
- Run the complete local test suite (`test` / `check`) and ensure 100% pass rate with zero regression errors before initiating any packaging.
- Check architecture constraints: Always explicitly specify the target platform architecture (e.g., `x64`, `arm64`) rather than generic defaults like `Any CPU` when building self-contained or platform-native applications.
- Verify runtime limits and payload boundaries (e.g., payload size limits, inlined resource bloat, embedded browser buffer limits).

## 3. Lean Artifact Staging & Hygiene
- **Zero Artifact Bloat**: Before compressing or packaging release binaries:
  - Exclude or strip debug symbols (`*.pdb`, `*.dSYM`) from production packages unless explicitly requested.
  - Eliminate nested build intermediates, staging folders, or duplicate publish directories.
  - Never package temporary caches or development configuration files.
- **Dual-Distribution Format**:
  - Always generate a portable, zero-install archive (`.zip` / `.tar.gz`) for direct extraction.
  - Generate the platform-native package/installer (`.msix`, `.exe`, `.pkg`, `.deb`) for standard OS deployment.
- **Integrity Verification**:
  - Calculate and generate SHA-256 checksum files (`.sha256`) for EVERY generated release binary.

## 4. Complete Asset Delivery Contract
When publishing the release (via GitHub CLI `gh release create`, API, or CI/CD workflow), you MUST upload all 4 critical categories of artifacts:
1. Portable distribution archives (`<App>-v<Version>-<arch>.zip`)
2. Portable checksum files (`<App>-v<Version>-<arch>.zip.sha256`)
3. Signed/Platform package (`<App>-v<Version>-<arch>.msix` or installer)
4. Package checksum files (`<App>-v<Version>-<arch>.msix.sha256`)

Never create a release containing only source tarballs (`Source code.zip`). An agent's release job is incomplete until all compiled binaries and their checksums are verified on the release page.

## 5. Structured Release Notes
Draft clear, human-readable release notes containing:
- **Summary**: High-level value of the release.
- **Root Cause & Fixes**: What bug occurred, why it occurred, and how it was solved technically.
- **Artifact Manifest**: A table listing each binary name, target architecture, file size, and SHA-256 hash.
- **Target OS & Compatibility**: Minimum supported OS versions and prerequisites.