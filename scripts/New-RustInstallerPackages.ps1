#requires -Version 7.0
<#
.SYNOPSIS
CI only: builds and verifies the unsigned per-user setup executable and the unsigned MSIX
package of Marknexia (Rust preview) for one architecture. Nothing is signed or published.

.DESCRIPTION
Runs only on GitHub Actions runners, after New-RustPortablePackage.ps1 (it reuses that
script's staged, verified files). It refuses to run anywhere else: it embeds and packs a
real release executable (local antivirus policy, see the production plan).

Setup (marknexia-rust-UNSIGNED-PREVIEW-setup-<version>-<arch>.exe):
1. Copies the four staged portable files (shell, pinned loader, README, notices) and writes
   an SPDX SBOM that describes the setup; the payload folder is those files plus the SBOM
   as marknexia-sbom.spdx.json.
2. Builds crates/marknexia-setup twice, from the two clean checkouts, with
   MARKNEXIA_SETUP_PAYLOAD_DIR set, and requires byte-identical executables
   (Compare-RustBuilds.ps1) with the expected PE machine (Test-RustArchitecture.ps1).
3. Finds the payload container in the executable with an independent parser
   (Test-RustSetupPayload.ps1): exactly the five expected files, every SHA-256 equal to the
   payload folder. Extracts it and validates the SBOM against the extracted files
   (Test-RustSbom.ps1) and the setup program's own crates against the SBOM.
MSIX (marknexia-rust-UNSIGNED-PREVIEW-msix-<version>-<arch>.msix):
4. Stages the four files, an SBOM that describes the MSIX, AppxManifest.xml from
   packaging/rust-msix (version and architecture filled in) and three logos.
5. makeappx pack (schema validation of the manifest), makeappx unpack, then checks the
   unpacked identity, the exact file set and every hash, and the SBOM.
6. Writes SHA256SUMS and installer-verification.json next to each artifact.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc')][string]$Target,
    [Parameter(Mandatory)][string]$FirstSource,
    [Parameter(Mandatory)][string]$SecondSource,
    [Parameter(Mandatory)][string]$PortableOutputRoot,
    [Parameter(Mandatory)][string]$LoaderPackagePath,
    [Parameter(Mandatory)][string]$MetadataPath,
    [Parameter(Mandatory)][string]$HostMetadataPath,
    [Parameter(Mandatory)][string]$OutputRoot,
    [string]$SourceCommit = 'NOASSERTION'
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
if ($env:GITHUB_ACTIONS -ne 'true') {
    throw 'New-RustInstallerPackages.ps1 runs only on GitHub Actions runners. Do not build installers on a local machine.'
}
if (-not $env:SOURCE_DATE_EPOCH) { throw 'Set SOURCE_DATE_EPOCH to the source commit time.' }

$architecture = if ($Target -eq 'x86_64-pc-windows-msvc') { 'x64' } else { 'ARM64' }
$archToken = $architecture.ToLowerInvariant()
$repoRoot = Split-Path -Parent $PSScriptRoot
$utf8 = [Text.UTF8Encoding]::new($false)
$shellName = 'marknexia-win32.exe'
$loaderName = 'WebView2Loader.dll'
$setupBinary = 'marknexia-rust-preview-setup.exe'
$runtimeFiles = @('README.txt', 'THIRD-PARTY-NOTICES.txt', $loaderName, $shellName)
$loaderLockPath = Join-Path $repoRoot 'packaging\webview\loader.lock.json'
$loaderLock = Get-Content -LiteralPath $loaderLockPath -Raw | ConvertFrom-Json -AsHashtable

function Get-Digest([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Get-OrdinalSorted([string[]]$Items) { $copy = [string[]]$Items.Clone(); [Array]::Sort($copy, [StringComparer]::Ordinal); , $copy }
function New-Folder([string]$Path) {
    if (Test-Path -LiteralPath $Path) { throw "Folder must not exist yet: $Path" }
    [IO.Directory]::CreateDirectory($Path).FullName
}
function Copy-RuntimeFiles([string]$Destination) {
    foreach ($name in $runtimeFiles) { Copy-Item -LiteralPath (Join-Path $staging $name) -Destination (Join-Path $Destination $name) }
}
function Write-Sums([string]$Folder, [string[]]$Names) {
    $lines = foreach ($name in (Get-OrdinalSorted $Names)) { "$(Get-Digest (Join-Path $Folder $name))  $name" }
    [IO.File]::WriteAllText((Join-Path $Folder 'SHA256SUMS'), ($lines -join "`n") + "`n", $utf8)
}

# --- Inputs ---------------------------------------------------------------------
$metadata = Get-Content -LiteralPath $MetadataPath -Raw | ConvertFrom-Json -AsHashtable
$members = @($metadata.packages | Where-Object { $metadata.workspace_members -contains $_.id })
$shellCrate = @($members | Where-Object name -ceq 'marknexia-win32')
$setupCrate = @($members | Where-Object name -ceq 'marknexia-setup')
if ($shellCrate.Count -ne 1 -or $setupCrate.Count -ne 1) { throw 'cargo metadata must hold marknexia-win32 and marknexia-setup once each.' }
$version = $shellCrate[0].version
if ($setupCrate[0].version -cne $version) { throw "marknexia-setup $($setupCrate[0].version) differs from marknexia-win32 $version." }
if ($version -notmatch '^(\d+)\.(\d+)\.(\d+)$') { throw "MSIX needs a numeric version; marknexia-win32 is $version." }
$msixVersion = "$($Matches[1]).$($Matches[2]).$($Matches[3]).0"

$staging = Join-Path $PortableOutputRoot "staging\marknexia-win32-$version-$archToken-unsigned-preview"
$stagedNames = Get-OrdinalSorted @(Get-ChildItem -LiteralPath $staging -File | ForEach-Object Name)
if (($stagedNames -join '|') -cne ((Get-OrdinalSorted $runtimeFiles) -join '|')) { throw "Unexpected portable staging: $($stagedNames -join ', ')" }
if ((Get-Digest (Join-Path $staging $loaderName)) -ne $loaderLock.loaders[$architecture].sha256) { throw 'Staged loader is not the pinned loader.' }

# The SDK license, read from the pinned package as in New-RustPortablePackage.ps1, so the
# notices are byte-identical to the portable ZIP's.
$output = [IO.Path]::GetFullPath($OutputRoot)
$work = New-Folder (Join-Path $output 'work')
$loaderLicense = $null
$package = [IO.Compression.ZipFile]::OpenRead([IO.Path]::GetFullPath($LoaderPackagePath))
try {
    $entry = @($package.Entries | Where-Object { $_.FullName -match '^(?i)licen[cs]e(\.txt|\.md)?$' } | Sort-Object FullName) | Select-Object -First 1
    if ($entry) {
        $reader = [IO.StreamReader]::new($entry.Open())
        try { $loaderLicense = Join-Path $work 'webview2-sdk-license.txt'; [IO.File]::WriteAllText($loaderLicense, $reader.ReadToEnd()) } finally { $reader.Dispose() }
    }
} finally { $package.Dispose() }

function New-Sbom([string]$PackageRoot, [string]$OutputPath, [string]$DistributionFileName, [string]$Description, [string]$Kind) {
    $arguments = @{
        MetadataPath = $MetadataPath; HostMetadataPath = $HostMetadataPath; PackageRoot = $PackageRoot; Architecture = $architecture
        OutputPath = $OutputPath; DistributionFileName = $DistributionFileName; SourceCommit = $SourceCommit; LoaderLockPath = $loaderLockPath
        DistributionDescription = $Description; DistributionKind = $Kind
    }
    if ($loaderLicense) { $arguments.LoaderLicensePath = $loaderLicense }
    & (Join-Path $PSScriptRoot 'Generate-RustSbom.ps1') @arguments | Out-Host
    $notices = Join-Path $PackageRoot 'THIRD-PARTY-NOTICES.txt'
    if ((Get-Digest $notices) -ne (Get-Digest (Join-Path $staging 'THIRD-PARTY-NOTICES.txt'))) { throw "Notices for $DistributionFileName differ from the portable ZIP's." }
}
function Test-Sbom([string]$SbomPath, [string]$PackageRoot, [string]$DistributionFileName) {
    & (Join-Path $PSScriptRoot 'Test-RustSbom.ps1') -SbomPath $SbomPath -PackageRoot $PackageRoot -MetadataPath $MetadataPath -HostMetadataPath $HostMetadataPath `
        -Architecture $architecture -LoaderLockPath $loaderLockPath -DistributionFileNamePattern ('^' + [regex]::Escape($DistributionFileName) + '$') | Out-Host
}

# --- 1. Setup payload ----------------------------------------------------------------
$setupBase = "marknexia-rust-UNSIGNED-PREVIEW-setup-$version-$archToken"
$setupName = "$setupBase.exe"
$setupSbomRoot = New-Folder (Join-Path $work 'setup-sbom-root')
Copy-RuntimeFiles $setupSbomRoot
$setupSbom = Join-Path $work "$setupBase.spdx.json"
New-Sbom $setupSbomRoot $setupSbom $setupName 'per-user setup executable' 'setup payload'
$payloadDir = New-Folder (Join-Path $work 'setup-payload')
Copy-RuntimeFiles $payloadDir
Copy-Item -LiteralPath $setupSbom -Destination (Join-Path $payloadDir 'marknexia-sbom.spdx.json')

# --- 2. Build twice and compare -------------------------------------------------------
$env:CARGO_INCREMENTAL = '0'
$env:MARKNEXIA_SETUP_PAYLOAD_DIR = $payloadDir
$builds = foreach ($source in @($FirstSource, $SecondSource)) {
    $sourceFull = (Resolve-Path -LiteralPath $source).Path
    $env:RUSTFLAGS = "-C link-arg=/Brepro --cfg ammonia_unstable --remap-path-prefix=$sourceFull=/workspace"
    cargo build --manifest-path (Join-Path $sourceFull 'Cargo.toml') -p marknexia-setup --release --locked --target $Target | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "cargo build of marknexia-setup failed in $sourceFull" }
    Join-Path $sourceFull "target\$Target\release"
}
Remove-Item Env:\MARKNEXIA_SETUP_PAYLOAD_DIR
$setupOut = New-Folder (Join-Path $output 'setup')
& (Join-Path $PSScriptRoot 'Compare-RustBuilds.ps1') -FirstRoot $builds[0] -SecondRoot $builds[1] -OutputPath (Join-Path $setupOut 'setup-reproducibility.json') -RequiredRelativePaths @($setupBinary)
$builtSetup = Join-Path $builds[0] $setupBinary
if ((Get-Digest $builtSetup) -ne (Get-Digest (Join-Path $builds[1] $setupBinary))) { throw 'The two setup builds differ.' }
& (Join-Path $PSScriptRoot 'Test-RustArchitecture.ps1') -Path $builtSetup -Expected $architecture
$setupPath = Join-Path $setupOut $setupName
Copy-Item -LiteralPath $builtSetup -Destination $setupPath
Copy-Item -LiteralPath $setupSbom -Destination (Join-Path $setupOut "$setupBase.spdx.json")

# --- 3. Verify the embedded payload ----------------------------------------------------
$extracted = Join-Path $work 'setup-extracted'
$container = & (Join-Path $PSScriptRoot 'Test-RustSetupPayload.ps1') -SetupPath $setupPath -ExpectedRoot $payloadDir -ExtractTo $extracted
if ((Get-Digest (Join-Path $extracted 'marknexia-sbom.spdx.json')) -ne (Get-Digest $setupSbom)) { throw 'The embedded SBOM is not the setup SBOM.' }
if ((Get-Digest (Join-Path $extracted $shellName)) -ne (Get-Digest (Join-Path $staging $shellName))) {
    throw 'The embedded shell is not the verified portable shell.'
}
$extractedRuntime = New-Folder (Join-Path $work 'setup-extracted-runtime')
foreach ($name in $runtimeFiles) { Copy-Item -LiteralPath (Join-Path $extracted $name) -Destination (Join-Path $extractedRuntime $name) }
Test-Sbom $setupSbom $extractedRuntime $setupName

# The setup program's own crates: every one except marknexia-setup must be in the SBOM
# (the shell's graph); the setup crate itself is recorded in the evidence (rust-installers.md).
$setupTree = @(cargo tree --manifest-path (Join-Path $FirstSource 'Cargo.toml') --locked -e normal,build -p marknexia-setup --target $Target --prefix none --format '{p}' |
    ForEach-Object { if ($_ -cmatch '^(\S+) v(\S+)') { "$($Matches[1])@$($Matches[2])" } } | Select-Object -Unique)
$sbomCrates = @((Get-Content -LiteralPath $setupSbom -Raw | ConvertFrom-Json).packages |
    Where-Object { $_.SPDXID -clike 'SPDXRef-Cargo-*' -or $_.SPDXID -clike 'SPDXRef-Workspace-*' } | ForEach-Object { "$($_.name)@$($_.versionInfo)" })
$notCovered = @($setupTree | Where-Object { $_ -cne "marknexia-setup@$version" -and $sbomCrates -cnotcontains $_ })
if ($setupTree.Count -eq 0 -or $notCovered.Count -gt 0) { throw "Setup crates missing from the SBOM: $($notCovered -join ', ')" }

Write-Sums $setupOut @($setupName, "$setupBase.spdx.json")
$setupBytes = (Get-Item -LiteralPath $setupPath).Length
[ordered]@{
    schemaVersion = 'rust-unsigned-preview-setup-v1'
    target = $Target; architecture = $architecture; version = $version; sourceCommit = $SourceCommit
    signed = $false; published = $false
    setup = [ordered]@{ name = $setupName; sha256 = (Get-Digest $setupPath); bytes = $setupBytes; reproducible = $true; withinPhase7Limit15MB = ($setupBytes -le 15MB) }
    sbom = [ordered]@{ name = "$setupBase.spdx.json"; format = 'SPDX-2.3'; sha256 = (Get-Digest $setupSbom); embeddedAs = 'marknexia-sbom.spdx.json' }
    payload = [ordered]@{ containerSha256 = $container.trailerSha256; containerBytes = $container.containerBytes; files = $container.files }
    setupProgramCrates = [ordered]@{ count = $setupTree.Count; coveredBySbom = $setupTree.Count - 1; notInSbom = @("marknexia-setup@$version") }
} | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $setupOut 'installer-verification.json') -Encoding utf8NoBOM

# --- 4. MSIX staging --------------------------------------------------------------------
$msixBase = "marknexia-rust-UNSIGNED-PREVIEW-msix-$version-$archToken"
$msixName = "$msixBase.msix"
$msixSbomRoot = New-Folder (Join-Path $work 'msix-sbom-root')
Copy-RuntimeFiles $msixSbomRoot
$msixSbom = Join-Path $work "$msixBase.spdx.json"
New-Sbom $msixSbomRoot $msixSbom $msixName 'MSIX package' 'MSIX payload'
$msixRoot = New-Folder (Join-Path $work 'msix-root')
Copy-RuntimeFiles $msixRoot
Copy-Item -LiteralPath $msixSbom -Destination (Join-Path $msixRoot 'marknexia-sbom.spdx.json')
$assets = [IO.Directory]::CreateDirectory((Join-Path $msixRoot 'Assets')).FullName
$logos = @('Square150x150Logo.png', 'Square44x44Logo.png', 'StoreLogo.png')
foreach ($logo in $logos) { Copy-Item -LiteralPath (Join-Path $repoRoot "store-submission\assets\$logo") -Destination (Join-Path $assets $logo) }
$manifestText = [IO.File]::ReadAllText((Join-Path $repoRoot 'packaging\rust-msix\AppxManifest.xml'))
$manifestText = $manifestText.Replace('@VERSION@', $msixVersion).Replace('@ARCHITECTURE@', $archToken)
if ($manifestText -cmatch '@[A-Z]+@') { throw "Unreplaced manifest token: $($Matches[0])" }
[IO.File]::WriteAllText((Join-Path $msixRoot 'AppxManifest.xml'), $manifestText, $utf8)

function Test-Manifest([string]$Path) {
    [xml]$xml = Get-Content -LiteralPath $Path -Raw
    $ns = [Xml.XmlNamespaceManager]::new($xml.NameTable)
    $ns.AddNamespace('m', 'http://schemas.microsoft.com/appx/manifest/foundation/windows10')
    $ns.AddNamespace('uap', 'http://schemas.microsoft.com/appx/manifest/uap/windows10')
    $identity = $xml.SelectSingleNode('/m:Package/m:Identity', $ns)
    $application = $xml.SelectSingleNode('/m:Package/m:Applications/m:Application', $ns)
    if ($identity.GetAttribute('Name') -cne 'Marknexia.RustPreview') { throw 'MSIX identity must be Marknexia.RustPreview (not the .NET RRCLabs.Marknexia).' }
    if ($identity.GetAttribute('ProcessorArchitecture') -cne $archToken) { throw "MSIX architecture must be $archToken." }
    if ($identity.GetAttribute('Version') -cne $msixVersion) { throw "MSIX version must be $msixVersion." }
    if ($identity.GetAttribute('Publisher') -cnotlike '*OID.2.25.311729368913984317654407730594956997722=1') { throw 'The unsigned MSIX must carry the unsigned-publisher OID.' }
    if ($application.GetAttribute('Executable') -cne $shellName -or $application.GetAttribute('Id') -cne 'MarknexiaRustPreview') { throw 'MSIX application entry is wrong.' }
    $types = @($xml.SelectNodes('//uap:FileType', $ns) | ForEach-Object InnerText)
    if (($types -join ',') -cne '.md,.markdown,.mdown,.mkdn') { throw "Unexpected file types: $($types -join ',')" }
}
Test-Manifest (Join-Path $msixRoot 'AppxManifest.xml')

# --- 5. makeappx pack and unpack -----------------------------------------------------
$kits = 'C:\Program Files (x86)\Windows Kits\10\bin'
$makeAppx = @(Get-ChildItem -LiteralPath $kits -Directory | Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' } |
    Sort-Object { [version]$_.Name } -Descending | ForEach-Object { Join-Path $_.FullName 'x64\makeappx.exe' } | Where-Object { Test-Path -LiteralPath $_ }) | Select-Object -First 1
if (-not $makeAppx) { throw 'makeappx.exe was not found in the Windows SDK on this runner.' }
Write-Output "makeappx: $makeAppx"
$msixOut = New-Folder (Join-Path $output 'msix')
$msixPath = Join-Path $msixOut $msixName
& $makeAppx pack /o /v /h SHA256 /d $msixRoot /p $msixPath
if ($LASTEXITCODE -ne 0) { throw "makeappx pack failed ($LASTEXITCODE)." }
$unpacked = Join-Path $work 'msix-unpacked'
& $makeAppx unpack /o /v /p $msixPath /d $unpacked
if ($LASTEXITCODE -ne 0) { throw "makeappx unpack failed ($LASTEXITCODE)." }

$stagedFiles = @(Get-ChildItem -LiteralPath $msixRoot -File -Recurse | ForEach-Object { [IO.Path]::GetRelativePath($msixRoot, $_.FullName) })
$unpackedFiles = @(Get-ChildItem -LiteralPath $unpacked -File -Recurse | ForEach-Object { [IO.Path]::GetRelativePath($unpacked, $_.FullName) })
$generated = @('AppxBlockMap.xml', '[Content_Types].xml')
$expectedFiles = Get-OrdinalSorted (@($stagedFiles) + $generated)
if (((Get-OrdinalSorted $unpackedFiles) -join '|') -cne ($expectedFiles -join '|')) {
    throw "The MSIX holds other files than staged: $((Get-OrdinalSorted $unpackedFiles) -join ', ')"
}
$entries = foreach ($relative in (Get-OrdinalSorted $stagedFiles)) {
    $digest = Get-Digest (Join-Path $unpacked $relative)
    if ($relative -cne 'AppxManifest.xml' -and $digest -ne (Get-Digest (Join-Path $msixRoot $relative))) { throw "MSIX entry $relative differs from staging." }
    [ordered]@{ path = $relative.Replace('\', '/'); sha256 = $digest; bytes = (Get-Item -LiteralPath (Join-Path $unpacked $relative)).Length }
}
Test-Manifest (Join-Path $unpacked 'AppxManifest.xml')
$msixRuntime = New-Folder (Join-Path $work 'msix-unpacked-runtime')
foreach ($name in $runtimeFiles) { Copy-Item -LiteralPath (Join-Path $unpacked $name) -Destination (Join-Path $msixRuntime $name) }
Test-Sbom $msixSbom $msixRuntime $msixName
if ((Get-Digest (Join-Path $unpacked $shellName)) -ne (Get-Digest (Join-Path $staging $shellName))) {
    throw 'The MSIX shell is not the verified portable shell.'
}
Copy-Item -LiteralPath $msixSbom -Destination (Join-Path $msixOut "$msixBase.spdx.json")
Write-Sums $msixOut @($msixName, "$msixBase.spdx.json")
[ordered]@{
    schemaVersion = 'rust-unsigned-preview-msix-v1'
    target = $Target; architecture = $architecture; version = $version; msixVersion = $msixVersion; sourceCommit = $SourceCommit
    signed = $false; published = $false
    identity = 'Marknexia.RustPreview'
    makeappx = $makeAppx
    msix = [ordered]@{ name = $msixName; sha256 = (Get-Digest $msixPath); bytes = (Get-Item -LiteralPath $msixPath).Length }
    sbom = [ordered]@{ name = "$msixBase.spdx.json"; format = 'SPDX-2.3'; sha256 = (Get-Digest $msixSbom); embeddedAs = 'marknexia-sbom.spdx.json' }
    entries = @($entries)
} | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $msixOut 'installer-verification.json') -Encoding utf8NoBOM

# For the install round trip and the SPDX reference validation steps.
[ordered]@{ setupPath = $setupPath; payloadDir = $payloadDir; setupSbom = $setupSbom; msixSbom = $msixSbom } |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $output 'installer-paths.json') -Encoding utf8NoBOM
Write-Output "Unsigned preview setup verified: $setupPath"
Write-Output "Unsigned preview MSIX verified: $msixPath"
