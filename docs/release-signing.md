# Release signing (owner runbook)

Marknexia release files are code-signed on the owner's PC with the Certum cloud certificate
through SimplySign Desktop. GitHub never holds a signing key, a password or a one-time code.

## 1. Facts

| Item | Value |
|---|---|
| Certificate | `CN=Open Source Developer Ramana Reddy Chamakura, O=Open Source Developer, L=Hyderabad, S=Telangana, C=IN` |
| Issuer | `CN=Certum Code Signing 2021 CA` |
| SHA-1 thumbprint (selects the certificate) | `68572404AE99971D76B907D9452001DCEE7766F1` |
| SHA-256 of the certificate | `191db43c8e8c546a131a755aae71d55d9bc1a724acfdf9cb3c7647ef6d22589f` |
| Valid until | 2027-11-25 |
| Timestamp server | `http://time.certum.pl` (RFC 3161, SHA-256) |
| Configuration | `packaging/signing/release-signing.json` (public values only) |

Do not use the old self-signed `CN=Marknexia` certificate. The scripts refuse any other certificate.

## 2. How the flow works

The setup executables embed the application, so the application files are signed first and
the installers are built from the signed files.

| Stage | Where | What |
|---|---|---|
| 1 | GitHub Actions | CI builds the unsigned application files and uploads them as an artifact. |
| 2 | Owner's PC | `Invoke-ReleaseSigning.ps1 -Stage Binaries` signs them and, with `-Publish`, uploads them to a private **draft** release `signing-handoff-<product>-<run id>`. |
| 3 | GitHub Actions | The `Signed packages` workflow (manual) verifies every signature and builds the ZIP, setup and MSIX from the signed files. It signs and publishes nothing. |
| 4 | Owner's PC | `Invoke-ReleaseSigning.ps1 -Stage Packages` signs the setup executables and MSIX packages, regenerates the checksums and, with `-Publish`, creates the GitHub release. |
| 5 | Owner's PC | `Invoke-ReleaseSigning.ps1 -Stage Cleanup -Publish` removes the draft. |

Why a draft release carries the signed files from stage 2 to stage 3: a workflow cannot accept
files as inputs, and only a workflow run can create an artifact. A draft release is visible only
to people with write access, is never returned by `/releases/latest` (which the .NET updater
reads), creates no tag and triggers no workflow. Stage 3 refuses a handoff that is not a draft.

What is signed:

| Product, architecture | Stage 2 (application files) | Stage 4 (packages) |
|---|---|---|
| Rust, x64 and ARM64 | `marknexia-win32.exe` | `marknexia-rust-preview-setup-<v>-<arch>.exe`, `marknexia-rust-preview-msix-<v>-<arch>.msix` |
| .NET, x64 and ARM64 | every `.exe` and `.dll` in the app output that has no signature (`Marknexia.App.exe`, the `Marknexia.*.dll` assemblies and unsigned third-party DLLs) | `Marknexia-v<v>-win-<arch>-setup.exe`, `Marknexia-v<v>-win-<arch>.msix` |

Files that already have a valid signature (the Microsoft-signed `WebView2Loader.dll`, Windows App
SDK and WebView2 files) keep it and are not re-signed. ZIP archives and SBOMs are not signed; the
ZIPs contain the signed files and are covered by the checksums.

Checks on the way: the signed files must equal the CI build except for the signature (same PE
content digest), carry a valid, timestamped signature by the thumbprint above, and the packages
must contain exactly those signed files.

## 3. Before you start

1. Open SimplySign Desktop and log in with your one-time code. Keep it logged in for the whole stage.
2. Log in to the GitHub CLI once: `gh auth login` (write access to `ramanacr/marknexia`).
3. Use PowerShell 7 in a checkout of `main`: `cd D:\Practice\marknexia`.
4. Each stage first checks the certificate and makes one test signature on a throw-away script.
   If that fails, log in to SimplySign Desktop again.

Every stage is a **dry run** unless you add `-Publish`. A dry run leaves the signed files in
`%LOCALAPPDATA%\Marknexia\release-signing\<product>-<stage>-<run id>` (change it with `-OutputRoot`).

## 4. Rust edition (pre-release `rust-preview-*`)

1. Wait for a green `Rust Feasibility` run on `main`. Note its run id (`gh run list --workflow "Rust Feasibility" --branch main`).
2. Sign the shells (dry run first, then upload):
   ```powershell
   pwsh scripts/Invoke-ReleaseSigning.ps1 -Product Rust -Stage Binaries -RunId <ci run id>
   pwsh scripts/Invoke-ReleaseSigning.ps1 -Product Rust -Stage Binaries -RunId <ci run id> -Publish
   ```
3. Start stage 3 with the command the script prints:
   ```powershell
   gh workflow run signed-packages.yml --repo ramanacr/marknexia --ref main -f product=rust -f source_run_id=<ci run id> -f handoff_tag=signing-handoff-rust-<ci run id>
   ```
4. Wait until that run is green. Note its run id (`gh run list --workflow "Signed packages"`).
5. Sign the packages and check them (dry run), then publish:
   ```powershell
   pwsh scripts/Invoke-ReleaseSigning.ps1 -Product Rust -Stage Packages -RunId <packages run id> -Tag rust-preview-<version>
   pwsh scripts/Invoke-ReleaseSigning.ps1 -Product Rust -Stage Packages -RunId <packages run id> -Tag rust-preview-<version> -Publish
   ```
   The release is a pre-release (`--prerelease --latest=false`), so the .NET updater never offers it.
6. Remove the draft: `pwsh scripts/Invoke-ReleaseSigning.ps1 -Product Rust -Stage Cleanup -RunId <ci run id> -Publish`.

## 5. .NET edition (release `v*`)

Pushing a `v*` tag no longer publishes a release by itself: `release.yml` now tests, builds and
uploads the unsigned app output only (stage 1). The release appears in step 5 below.

1. Push the version tag, for example `git tag v1.0.19` and `git push origin v1.0.19`. Wait for the green `Package & Release` run and note its id.
2. Sign the application files:
   ```powershell
   pwsh scripts/Invoke-ReleaseSigning.ps1 -Product DotNet -Stage Binaries -RunId <release run id>
   pwsh scripts/Invoke-ReleaseSigning.ps1 -Product DotNet -Stage Binaries -RunId <release run id> -Publish
   ```
3. Start stage 3: `gh workflow run signed-packages.yml --repo ramanacr/marknexia --ref main -f product=dotnet -f source_run_id=<release run id> -f handoff_tag=signing-handoff-dotnet-<release run id>`
4. Wait until that run is green and note its id.
5. Sign, check, then publish (the tag is taken from the release run):
   ```powershell
   pwsh scripts/Invoke-ReleaseSigning.ps1 -Product DotNet -Stage Packages -RunId <packages run id>
   pwsh scripts/Invoke-ReleaseSigning.ps1 -Product DotNet -Stage Packages -RunId <packages run id> -Publish
   ```
   This creates the `v*` release (marked latest) on the existing tag, with the same asset names
   as before: `Marknexia-v<v>-win-<arch>-setup.exe`, `.zip`, `.msix` and a `.sha256` for each.
6. Remove the draft: `pwsh scripts/Invoke-ReleaseSigning.ps1 -Product DotNet -Stage Cleanup -RunId <release run id> -Publish`.

## 6. MSIX publisher change

Windows signs an MSIX only when the manifest `Publisher` equals the certificate subject exactly.
So the **GitHub** MSIX packages now use the certificate subject as publisher:

* .NET: `scripts/prepare-store-package.ps1 -PublisherId <subject>` in stage 3. The package name
  stays `RRCLabs.Marknexia`.
* Rust: `packaging/rust-msix/AppxManifest.xml` takes the publisher as a token;
  `New-RustInstallerPackages.ps1 -Signed` fills in the subject. The unsigned CI evidence keeps the
  unsigned-publisher OID.

The publisher is part of the package family name, so the family name changes. A user who
sideloaded an earlier GitHub MSIX must uninstall it and install the new one; Windows treats them
as different apps. Settings in the old package's private folder are not carried over.

The **Microsoft Store** package is not changed: `prepare-store-package.ps1` still defaults to the
Store identity (`CN=4BC6AFD6-49C8-46B2-A096-9AF3F2B78CB8`), `store-submission/` is untouched, and
the Store re-signs what it distributes.

## 7. Verify by hand

```powershell
$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" | Sort-Object FullName -Descending | Select-Object -First 1
& $signtool verify /pa /v <file>          # "Successfully verified", signer and timestamp
Get-AuthenticodeSignature <file> | Format-List Status, SignerCertificate, TimeStamperCertificate
Get-FileHash <file> -Algorithm SHA256     # compare with SHA256SUMS.txt or <file>.sha256
```

The signer thumbprint must be `68572404AE99971D76B907D9452001DCEE7766F1` and a timestamp must be present.

## 8. Recover from a failed or partial run

1. Run the same command again with the same `-RunId` (and `-OutputRoot`, if you set one). Files
   that are already signed and timestamped are skipped; downloads are reused.
2. `signtool failed 3 times`: log in to SimplySign Desktop again (the session may have timed
   out), check that `http://time.certum.pl` answers, then run again.
3. "differs beyond its signature" or "changed after download": delete the output folder and run again.
4. Stage 3 failed: fix the cause, run stage 2 again with `-Publish` if the handoff must change
   (assets are replaced), then start the workflow again.
5. A file was signed but the release was not created: run stage 4 again with `-Publish`. It
   refuses to overwrite an existing release or tag; if a broken release exists, delete it on
   GitHub (for a `v*` release keep the tag), then run again.
6. Never publish the handoff draft by hand. If that happened, delete that release and its
   `signing-handoff-*` tag on GitHub.
7. When the certificate is renewed, update `packaging/signing/release-signing.json` (thumbprint,
   SHA-256, subject, validity). A new subject changes the MSIX publisher again.

## 9. Tests

`tests/signing/release_signing.tests.ps1` (run in Rust CI) tests the flow with signtool, gh and
Get-AuthenticodeSignature replaced by script blocks; it writes no executable and no archive.
