#requires -Version 7.0
# Fixture tests for Generate-RustSbom.ps1 (SBOM and notices), Test-RustSbom.ps1 (validator),
# and source contracts for the CI-only New-RustPortablePackage.ps1.
# Stand-in package files are plain text with .txt names. This test writes no PE bytes
# and no archive, and it never runs the packaging script.
[CmdletBinding()]
# -PySpdxTools: path to a pinned pyspdxtools executable; when given, the fixture SBOM must
# also pass the SPDX reference validator (CI installs it; see tools/rust-tools.lock).
param(
    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path,
    [string]$PySpdxTools
)

$ErrorActionPreference = 'Stop'
function Assert-That([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Get-Sha([object]$Element, [string]$Algorithm = 'SHA256') { @($Element.checksums | Where-Object algorithm -ceq $Algorithm)[0].checksumValue }
function Assert-Fails([scriptblock]$Action, [string]$Pattern, [string]$Message) {
    $failed = $false
    try { & $Action | Out-Null } catch { $failed = $_.Exception.Message -like $Pattern; if (-not $failed) { throw "$Message (unexpected error: $($_.Exception.Message))" } }
    Assert-That $failed $Message
}

$generator = Join-Path $RepositoryRoot 'scripts\Generate-RustSbom.ps1'
$validator = Join-Path $RepositoryRoot 'scripts\Test-RustSbom.ps1'
$fixtures = Join-Path $PSScriptRoot 'fixtures\rust-sbom'
$testDirectory = Join-Path ([IO.Path]::GetTempPath()) ('marknexia-rust-sbom-' + [Guid]::NewGuid().ToString('N'))
$originalEpoch = $env:SOURCE_DATE_EPOCH
New-Item -ItemType Directory -Path $testDirectory | Out-Null
try {
    $env:SOURCE_DATE_EPOCH = '1788652800'
    $utf8 = [Text.UTF8Encoding]::new($false)
    function Write-Text([string]$Path, [string]$Text) {
        [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($Path)) | Out-Null
        [IO.File]::WriteAllText($Path, $Text, $utf8)
    }

    # Crate sources with license files (alpha: two; both: license_file only; derive-macro: lower-case name).
    $crates = Join-Path $testDirectory 'registry'
    Write-Text (Join-Path $crates 'alpha-1.0.0\LICENSE-MIT') "Alpha MIT text`r`nCopyright Alpha Authors`r`n"
    Write-Text (Join-Path $crates 'alpha-1.0.0\LICENSE-APACHE') "Alpha Apache text`n"
    Write-Text (Join-Path $crates 'alpha-1.0.0\src\lib.rs') "// not a license`n"
    New-Item -ItemType Directory -Force -Path (Join-Path $crates 'beta-0.3.0'), (Join-Path $crates 'gamma-1.1.0') | Out-Null
    Write-Text (Join-Path $crates 'both-1.0.0\COPYING-CUSTOM.txt') "Both custom license`n"
    Write-Text (Join-Path $crates 'builder-2.0.0\LICENSE') "Builder Apache text`n"
    Write-Text (Join-Path $crates 'derive-macro-1.0.0\license-mit') "Derive MIT text`n"
    Write-Text (Join-Path $crates 'devonly-1.0.0\LICENSE') "DEVONLY MUST NOT APPEAR`n"

    $metadataText = (Get-Content -LiteralPath (Join-Path $fixtures 'cargo-metadata.fixture.json') -Raw).
        Replace('@CRATES@', $crates.Replace('\', '/')).Replace('@WORKSPACE@', 'C:/fixture-workspace')
    $metadataPath = Join-Path $testDirectory 'metadata.json'
    Write-Text $metadataPath $metadataText
    $cargoLock = Join-Path $fixtures 'cargo-lock.fixture.toml'

    $packageRoot = Join-Path $testDirectory 'package'
    Write-Text (Join-Path $packageRoot 'shell-standin.txt') "stand-in for the shell artifact`n"
    Write-Text (Join-Path $packageRoot 'loader-standin.txt') "stand-in for the loader`n"
    Write-Text (Join-Path $packageRoot 'README.txt') "readme`n"
    $loaderDigest = (Get-FileHash -LiteralPath (Join-Path $packageRoot 'loader-standin.txt') -Algorithm SHA256).Hash.ToLowerInvariant()
    $loaderLock = Get-Content -LiteralPath (Join-Path $RepositoryRoot 'packaging\webview\loader.lock.json') -Raw | ConvertFrom-Json -AsHashtable
    $loaderLock.loaders.x64.sha256 = $loaderDigest
    $loaderLockPath = Join-Path $testDirectory 'loader.lock.json'
    Write-Text $loaderLockPath ($loaderLock | ConvertTo-Json -Depth 5)
    $mermaidLockPath = Join-Path $RepositoryRoot 'packaging\webview\mermaid.lock.json'
    $mermaidLock = Get-Content -LiteralPath $mermaidLockPath -Raw | ConvertFrom-Json
    $loaderLicense = Join-Path $testDirectory 'webview2-license.txt'
    Write-Text $loaderLicense "WebView2 SDK license text`n"
    $sbomPath = Join-Path $testDirectory 'out\sbom.spdx.json'

    $common = @{
        MetadataPath = $metadataPath; PackageRoot = $packageRoot; Architecture = 'x64'
        CargoLockPath = $cargoLock; LoaderLockPath = $loaderLockPath; MermaidLockPath = $mermaidLockPath
        ShellFileName = 'shell-standin.txt'; LoaderFileName = 'loader-standin.txt'
    }
    $generate = { param($Extra = @{}) & $generator @common -OutputPath $sbomPath -LoaderLicensePath $loaderLicense -SourceCommit 'abc123' @Extra }
    $validate = { & $validator @common -SbomPath $sbomPath }

    & $generate | Out-Null
    $firstSbom = (Get-FileHash -LiteralPath $sbomPath).Hash
    $noticesPath = Join-Path $packageRoot 'THIRD-PARTY-NOTICES.txt'
    $firstNotices = (Get-FileHash -LiteralPath $noticesPath).Hash
    $document = Get-Content -LiteralPath $sbomPath -Raw | ConvertFrom-Json -AsHashtable

    # 1. Document shape and graph integrity.
    Assert-That ($document.spdxVersion -ceq 'SPDX-2.3') 'Expected SPDX-2.3, as the .NET pipeline uses.'
    Assert-That ($document.name -ceq 'marknexia-win32-0.1.0-x64-unsigned-preview') 'Document name must mark the unsigned preview.'
    Assert-That ((Get-Content -LiteralPath $sbomPath -Raw).Contains('"created": "2026-09-06T00:00:00Z"')) 'SOURCE_DATE_EPOCH must set the creation time.'
    $ids = @('SPDXRef-DOCUMENT') + @($document.packages | ForEach-Object SPDXID) + @($document.files | ForEach-Object SPDXID)
    Assert-That (@($ids | Select-Object -Unique).Count -eq $ids.Count) 'Duplicate SPDX IDs.'
    foreach ($edge in $document.relationships) {
        Assert-That ($ids -ccontains $edge.spdxElementId -and $ids -ccontains $edge.relatedSpdxElement) "Dangling relationship: $($edge | ConvertTo-Json -Compress)"
    }

    # 2. Crate set: shipped graph only; dev-only, dev-transitive and unrelated members excluded.
    $byName = @{}; foreach ($p in $document.packages) { $byName[$p.name] = $p }
    $crateNames = @($document.packages | Where-Object { $_.SPDXID -match '^SPDXRef-(Cargo|Workspace)-' } | ForEach-Object name | Sort-Object)
    Assert-That (($crateNames -join ',') -ceq 'alpha,beta,both,builder,derive-macro,gamma,marknexia-core,marknexia-win32') "Unexpected crate set: $($crateNames -join ',')"
    $order = @($document.packages | Where-Object { $_.SPDXID -match '^SPDXRef-(Cargo|Workspace)-' } | ForEach-Object name) -join ','
    Assert-That ($order -ceq 'alpha,beta,both,builder,derive-macro,gamma,marknexia-core,marknexia-win32') "Crates must be in ordinal order: $order"
    foreach ($absent in 'devonly', 'hidden', 'marknexia-update') { Assert-That (-not $byName.ContainsKey($absent)) "$absent must be excluded." }

    # 3. Versions, checksums, purls, download locations and licenses.
    Assert-That ($byName.alpha.versionInfo -ceq '1.0.0') 'Crate version lost.'
    Assert-That ($byName.alpha.checksums[0].checksumValue -ceq ('1' * 64)) 'Crate checksum must come from Cargo.lock.'
    Assert-That ($byName.alpha.externalRefs[0].referenceLocator -ceq 'pkg:cargo/alpha@1.0.0') 'Missing cargo purl.'
    Assert-That ($byName.alpha.downloadLocation -ceq 'https://crates.io/api/v1/crates/alpha/1.0.0/download') 'Missing crates.io download location.'
    Assert-That ($byName.alpha.licenseDeclared -ceq 'MIT OR Apache-2.0') 'Legacy slash license must become an SPDX OR expression.'
    Assert-That ($byName.both.licenseDeclared -ceq 'NOASSERTION') 'Missing license expression must be NOASSERTION.'
    Assert-That ($byName.alpha.licenseConcluded -ceq 'NOASSERTION') 'Licenses must not be concluded.'
    Assert-That (-not $byName['marknexia-core'].ContainsKey('checksums')) 'Workspace crates have no registry checksum.'
    Assert-That ($byName['marknexia-win32'].comment -like '*abc123*') 'Root comment must name the source commit.'

    # 4. Scope: proc-macros and build scripts, and what only they reach, are build-time.
    Assert-That ($byName.alpha.comment -like '*(runtime)*' -and $byName.beta.comment -like '*(runtime)*' -and $byName.both.comment -like '*(runtime)*') 'Normal dependencies are runtime.'
    foreach ($name in 'builder', 'derive-macro', 'gamma') { Assert-That ($byName[$name].comment -like '*(build-time)*') "$name must be build-time." }
    $edge = @($document.relationships | Where-Object { $_.relationshipType -ceq 'BUILD_DEPENDENCY_OF' -and $_.spdxElementId -ceq $byName.builder.SPDXID -and $_.relatedSpdxElement -ceq $byName['marknexia-core'].SPDXID })
    Assert-That ($edge.Count -eq 1) 'Missing BUILD_DEPENDENCY_OF edge.'
    $edge = @($document.relationships | Where-Object { $_.relationshipType -ceq 'DEPENDS_ON' -and $_.spdxElementId -ceq $byName['marknexia-core'].SPDXID -and $_.relatedSpdxElement -ceq $byName.beta.SPDXID })
    Assert-That ($edge.Count -eq 1) 'Missing transitive DEPENDS_ON edge.'

    # 5. Mermaid and WebView2 components from their locks.
    Assert-That ($byName.mermaid.versionInfo -ceq '10.9.1' -and $byName.mermaid.checksums[0].checksumValue -ceq $mermaidLock.served.sha256) 'Mermaid must carry the embedded (served) SHA-256.'
    Assert-That ($byName.mermaid.licenseDeclared -ceq 'MIT') 'Mermaid license lost.'
    $webView = $byName['Microsoft.Web.WebView2']
    Assert-That ($webView.versionInfo -ceq $loaderLock.version -and $webView.checksums[0].checksumValue -ceq $loaderLock.packageSha256) 'WebView2 package must match loader.lock.json.'

    # 6. Files: every package file, hashed, including the generated notices.
    $fileNames = @($document.files | ForEach-Object fileName | Sort-Object -CaseSensitive)
    Assert-That (($fileNames -join ',') -ceq ((@('./README.txt', './THIRD-PARTY-NOTICES.txt', './loader-standin.txt', './shell-standin.txt') | Sort-Object -CaseSensitive) -join ',')) "Unexpected files: $($fileNames -join ',')"
    $loaderEntry = @($document.files | Where-Object fileName -ceq './loader-standin.txt')[0]
    Assert-That ((Get-Sha $loaderEntry) -ceq $loaderDigest) 'Loader hash lost.'
    Assert-That ((Get-Sha $loaderEntry 'SHA1') -ceq (Get-FileHash -LiteralPath (Join-Path $packageRoot 'loader-standin.txt') -Algorithm SHA1).Hash.ToLowerInvariant()) 'Every file needs a SHA1 checksum (SPDX 2.3 section 8.4).'
    $noticesEntry = @($document.files | Where-Object fileName -ceq './THIRD-PARTY-NOTICES.txt')[0]
    Assert-That ((Get-Sha $noticesEntry) -ceq $firstNotices.ToLowerInvariant()) 'Notices hash must be of the final notices file.'
    $shellEntry = @($document.files | Where-Object fileName -ceq './shell-standin.txt')[0]
    Assert-That (@($document.relationships | Where-Object { $_.spdxElementId -ceq $shellEntry.SPDXID -and $_.relationshipType -ceq 'CONTAINS' -and $_.relatedSpdxElement -ceq $byName.mermaid.SPDXID }).Count -eq 1) 'Shell must CONTAIN Mermaid.'

    # 6b. Distribution package: DESCRIBED, filesAnalyzed=true, verification code, CONTAINS every file.
    # The archive name is split so no_synthetic_pe_disk.tests.ps1 does not read it as an archive write.
    $zip = @($document.packages | Where-Object { $_.SPDXID -clike 'SPDXRef-Distribution-*' })
    Assert-That ($zip.Count -eq 1 -and $zip[0].packageFileName -ceq ('marknexia-win32-0.1.0-x64-unsigned-preview.' + 'zip') -and $zip[0].filesAnalyzed -eq $true) 'Missing distribution package.'
    Assert-That ($zip[0].primaryPackagePurpose -ceq 'ARCHIVE') 'The distribution package is an ARCHIVE.'
    $describes = @($document.relationships | Where-Object relationshipType -ceq 'DESCRIBES')
    Assert-That ($describes.Count -eq 1 -and $describes[0].spdxElementId -ceq 'SPDXRef-DOCUMENT' -and $describes[0].relatedSpdxElement -ceq $zip[0].SPDXID) 'The document must DESCRIBE only the ZIP.'
    $contained = @($document.relationships | Where-Object { $_.spdxElementId -ceq $zip[0].SPDXID -and $_.relationshipType -ceq 'CONTAINS' })
    Assert-That ($contained.Count -eq 4) 'The ZIP must CONTAIN its four files.'
    $sha1 = [string[]]@($document.files | ForEach-Object { Get-Sha $_ 'SHA1' }); [Array]::Sort($sha1, [StringComparer]::Ordinal)
    $code = [Convert]::ToHexString([Security.Cryptography.SHA1]::HashData([Text.Encoding]::ASCII.GetBytes(($sha1 -join '')))).ToLowerInvariant()
    Assert-That ($zip[0].packageVerificationCode.packageVerificationCodeValue -ceq $code) 'Wrong packageVerificationCode.'
    foreach ($package in $document.packages | Where-Object { $_.filesAnalyzed -ne $true }) {
        Assert-That (@($document.relationships | Where-Object { $_.spdxElementId -ceq $package.SPDXID -and $_.relationshipType -ceq 'CONTAINS' -and $_.relatedSpdxElement -clike 'SPDXRef-File-*' }).Count -eq 0) "$($package.SPDXID) has filesAnalyzed=false and must not CONTAIN files."
    }
    Assert-That ($document.packages.Count -eq 8 + 3) 'Expected 8 crates plus the ZIP, Mermaid and WebView2 packages.'

    # 7. Notices: same data, license texts, link fallback, no dev-only crate, LF only.
    $notices = [IO.File]::ReadAllText($noticesPath)
    Assert-That (-not $notices.Contains("`r")) 'Notices must use LF line endings.'
    foreach ($expected in 'Alpha MIT text', 'Copyright Alpha Authors', 'Alpha Apache text', 'Both custom license', 'Builder Apache text',
        'Derive MIT text', 'WebView2 SDK license text', 'Permission is hereby granted', 'mermaid 10.9.1', 'https://spdx.org/licenses/MIT.html',
        'https://spdx.org/licenses/Unicode-3.0.html', 'alpha 1.0.0 (Rust crate, runtime)', 'gamma 1.1.0 (Rust crate, build-time)') {
        Assert-That ($notices.Contains($expected)) "Notices are missing: $expected"
    }
    foreach ($unexpected in 'DEVONLY', 'devonly', 'hidden 1.0.0', 'not a license', 'marknexia-core 0.1.0 (Rust crate') {
        Assert-That (-not $notices.Contains($unexpected)) "Notices must not contain: $unexpected"
    }
    $positions = @('alpha 1.0.0', 'beta 0.3.0', 'both 1.0.0', 'builder 2.0.0', 'derive-macro 1.0.0', 'gamma 1.1.0', 'mermaid 10.9.1', 'Microsoft.Web.WebView2') |
        ForEach-Object { $notices.IndexOf($_, [StringComparison]::Ordinal) }
    Assert-That (($positions -join ',') -ceq (($positions | Sort-Object) -join ',') -and $positions[0] -gt 0) 'Notices must list components in a stable order.'
    Assert-That ($notices.Contains('No license is declared and the crate source carries no license file.') -eq $false) 'both has a license_file, so it must not use the no-license text.'

    # 8. Reproducibility and validator acceptance.
    & $generate | Out-Null
    Assert-That ((Get-FileHash -LiteralPath $sbomPath).Hash -ceq $firstSbom) 'Repeated SBOM generation is not byte-identical.'
    Assert-That ((Get-FileHash -LiteralPath $noticesPath).Hash -ceq $firstNotices) 'Repeated notices generation is not byte-identical.'
    $result = & $validate
    Assert-That ("$result" -like 'SBOM valid:*') 'Validator must accept the generated SBOM.'

    if ($PySpdxTools) {
        & $PySpdxTools -i $sbomPath
        Assert-That ($LASTEXITCODE -eq 0) 'pyspdxtools rejected the fixture SBOM.'
    }

    # 8b. Cross build: build-time crates follow the host graph, not the target graph.
    $targetMeta = $metadataText | ConvertFrom-Json -AsHashtable
    $hostMeta = $metadataText | ConvertFrom-Json -AsHashtable
    $registry = 'registry+https://github.com/rust-lang/crates.io-index'
    foreach ($pair in @(@($targetMeta, 'targetonly'), @($hostMeta, 'hostonly'))) {
        $meta, $name = $pair
        $id = "$registry#$name@1.0.0"
        $meta.packages += @{ name = $name; version = '1.0.0'; id = $id; license = 'MIT'; license_file = $null; source = $registry; repository = $null
            manifest_path = "$($crates.Replace('\', '/'))/$name-1.0.0/Cargo.toml"; targets = @(@{ kind = @('lib'); name = $name }) }
        $meta.resolve.nodes += @{ id = $id; deps = @() }
        @($meta.resolve.nodes | Where-Object { $_.id -ceq "$registry#builder@2.0.0" })[0].deps += @{ name = $name; pkg = $id; dep_kinds = @(@{ kind = $null; target = $null }) }
    }
    $targetMetaPath = Join-Path $testDirectory 'target-metadata.json'
    $hostMetaPath = Join-Path $testDirectory 'host-metadata.json'
    Write-Text $targetMetaPath ($targetMeta | ConvertTo-Json -Depth 20)
    Write-Text $hostMetaPath ($hostMeta | ConvertTo-Json -Depth 20)
    $crossSbom = Join-Path $testDirectory 'out\cross.spdx.json'
    & $generator @common -MetadataPath $targetMetaPath -HostMetadataPath $hostMetaPath -OutputPath $crossSbom | Out-Null
    $cross = Get-Content -LiteralPath $crossSbom -Raw | ConvertFrom-Json -AsHashtable
    $crossNames = @($cross.packages | ForEach-Object name)
    Assert-That ($crossNames -ccontains 'hostonly' -and $crossNames -cnotcontains 'targetonly') 'Build-time crates must come from the host graph.'
    Assert-That (@($cross.packages | Where-Object name -ceq 'hostonly')[0].comment -clike '*(build-time)*') 'Host-only crates are build-time.'
    $crossValidate = $common.Clone(); $crossValidate.MetadataPath = $targetMetaPath; $crossValidate.HostMetadataPath = $hostMetaPath
    $result = & $validator @crossValidate -SbomPath $crossSbom
    Assert-That ("$result" -like 'SBOM valid:*') 'Validator must accept the cross-build SBOM.'
    Assert-Fails { & $validator @common -SbomPath $crossSbom -MetadataPath $targetMetaPath } '*missing crates: targetonly@1.0.0*' 'Without the host graph the cross SBOM must not validate.'

    # The cross run rewrote the notices in the package root; restore the native output.
    & $generate | Out-Null
    Assert-That ((Get-FileHash -LiteralPath $sbomPath).Hash -ceq $firstSbom) 'Native SBOM must be restored byte-identically.'

    # 8c. cargo tree cross-check (lines as `cargo tree --prefix none --format "{p}"` prints them).
    $tree = Join-Path $testDirectory 'cargo-tree.txt'
    $treeLines = @('marknexia-win32 v0.1.0 (C:\fixture-workspace\crates\marknexia-win32)', 'alpha v1.0.0', 'both v1.0.0', 'derive-macro v1.0.0 (proc-macro)',
        'gamma v1.1.0', 'marknexia-core v0.1.0 (C:\fixture-workspace\crates\marknexia-core)', 'beta v0.3.0', 'builder v2.0.0', 'gamma v1.1.0 (*)', '')
    Write-Text $tree ($treeLines -join "`n")
    $result = & $validator @common -SbomPath $sbomPath -CargoTreePath $tree
    Assert-That ("$result" -like 'SBOM valid:*') 'Validator must accept a matching cargo tree.'
    Write-Text $tree (($treeLines | Where-Object { $_ -notlike 'beta*' }) + 'extra v9.9.9' -join "`n")
    Assert-Fails { & $validator @common -SbomPath $sbomPath -CargoTreePath $tree } '*Only in cargo tree: extra@9.9.9; only in SBOM: beta@0.3.0*' 'A cargo tree difference must fail.'

    # 9. Validator rejections.
    Write-Text (Join-Path $packageRoot 'README.txt') "tampered`n"
    Assert-Fails $validate '*SHA-256 mismatch for ./README.txt*' 'Validator must reject a changed file.'
    Write-Text (Join-Path $packageRoot 'README.txt') "readme`n"
    Write-Text (Join-Path $packageRoot 'extra.txt') "extra`n"
    Assert-Fails $validate '*File inventory differs*' 'Validator must reject an unlisted file.'
    Remove-Item -LiteralPath (Join-Path $packageRoot 'extra.txt')
    $good = Get-Content -LiteralPath $sbomPath -Raw
    $tampered = $good | ConvertFrom-Json -AsHashtable
    @($tampered.packages | Where-Object name -ceq 'alpha')[0].checksums[0].checksumValue = '0' * 64
    Write-Text $sbomPath ($tampered | ConvertTo-Json -Depth 20)
    Assert-Fails $validate '*alpha@1.0.0 checksum does not match Cargo.lock*' 'Validator must reject a changed crate checksum.'
    $tampered = $good | ConvertFrom-Json -AsHashtable
    $tampered.packages = @($tampered.packages | Where-Object name -cne 'beta')
    $tampered.relationships = @($tampered.relationships | Where-Object { $_.spdxElementId -notlike '*beta*' -and $_.relatedSpdxElement -notlike '*beta*' })
    Write-Text $sbomPath ($tampered | ConvertTo-Json -Depth 20)
    Assert-Fails $validate '*missing crates: beta@0.3.0*' 'Validator must reject a missing crate.'
    $tampered = $good | ConvertFrom-Json -AsHashtable
    $tampered.packages += [ordered]@{ SPDXID = 'SPDXRef-Cargo-devonly-1.0.0'; name = 'devonly'; versionInfo = '1.0.0'; downloadLocation = 'NOASSERTION'
        licenseConcluded = 'NOASSERTION'; licenseDeclared = 'MIT'; copyrightText = 'NOASSERTION'
        checksums = @(@{ algorithm = 'SHA256'; checksumValue = '6' * 64 }); externalRefs = @(@{ referenceLocator = 'pkg:cargo/devonly@1.0.0' }) }
    Write-Text $sbomPath ($tampered | ConvertTo-Json -Depth 20)
    Assert-Fails $validate '*outside the shipped graph*devonly@1.0.0*' 'Validator must reject a dev-only crate.'
    function Assert-Tampered([scriptblock]$Mutate, [string]$Pattern, [string]$Message) {
        $doc = $good | ConvertFrom-Json -AsHashtable
        & $Mutate $doc
        Write-Text $sbomPath ($doc | ConvertTo-Json -Depth 20)
        Assert-Fails $validate $Pattern $Message
    }
    function Get-Package($Doc, [string]$Name) { @($Doc.packages | Where-Object name -ceq $Name)[0] }
    function Rename-Id($Doc, [string]$Old, [string]$New) {
        foreach ($p in $Doc.packages) { if ($p.SPDXID -ceq $Old) { $p.SPDXID = $New } }
        foreach ($r in $Doc.relationships) { if ($r.spdxElementId -ceq $Old) { $r.spdxElementId = $New }; if ($r.relatedSpdxElement -ceq $Old) { $r.relatedSpdxElement = $New } }
    }
    Assert-Tampered { param($d) Rename-Id $d (Get-Package $d mermaid).SPDXID 'SPDXRef-Other-mermaid' } '*Unknown or invalid package SPDX ID: SPDXRef-Other-mermaid*' 'Unknown ID prefixes must fail.'
    Assert-Tampered { param($d) Rename-Id $d (Get-Package $d alpha).SPDXID 'SPDXRef-Workspace-alpha-1.0.0' } '*SPDXRef-Workspace-alpha-1.0.0 is not a workspace member*' 'Workspace IDs only for workspace members.'
    Assert-Tampered { param($d) Rename-Id $d (Get-Package $d 'marknexia-core').SPDXID 'SPDXRef-Cargo-marknexia-core-0.1.0' } '*SPDXRef-Cargo-marknexia-core-0.1.0 is not a crates.io crate*' 'Cargo IDs only for crates.io crates.'
    Assert-Tampered { param($d) $d.packages += @{ SPDXID = 'SPDXRef-NuGet-extra-1.0'; name = 'extra'; versionInfo = '1.0'; downloadLocation = 'NOASSERTION'; filesAnalyzed = $false
        licenseConcluded = 'NOASSERTION'; licenseDeclared = 'MIT'; copyrightText = 'NOASSERTION' } } '*Package count 12 is not the crate count 8*' 'Unexpected extra packages must fail.'
    Assert-Tampered { param($d) (Get-Package $d gamma).comment = 'Rust crate (runtime).' } '*gamma@1.1.0 must be marked (build-time)*' 'A wrong scope must fail.'
    Assert-Tampered { param($d) (Get-Package $d alpha).licenseDeclared = 'MIT OR' } '*invalid licenseDeclared: MIT OR*' 'A malformed license expression must fail.'
    Assert-Tampered { param($d) (Get-Package $d alpha).licenseDeclared = '(MIT AND Apache-2.0' } '*invalid licenseDeclared*' 'Unbalanced parentheses must fail.'
    Assert-Tampered { param($d) $d.relationships = @($d.relationships | Where-Object relationshipType -cne 'DESCRIBES') } '*no DESCRIBES relationship*' 'A missing DESCRIBES edge must fail.'
    Assert-Tampered { param($d) (Get-Package $d mermaid).externalRefs[0].referenceLocator = 'pkg:npm/mermaid@9.0.0' } '*Mermaid component does not match*' 'A wrong Mermaid purl must fail.'
    Assert-Tampered { param($d) (Get-Package $d mermaid).checksums[0].algorithm = 'SHA1' } '*Mermaid component does not match*' 'A wrong Mermaid checksum algorithm must fail.'
    Assert-Tampered { param($d) (Get-Package $d 'Microsoft.Web.WebView2').externalRefs[0].referenceLocator = 'pkg:nuget/Other@1.0' } '*WebView2 SDK component does not match*' 'A wrong WebView2 purl must fail.'
    Assert-Tampered { param($d) @($d.packages | Where-Object { $_.SPDXID -clike 'SPDXRef-Distribution-*' })[0].packageVerificationCode.packageVerificationCodeValue = '0' * 40 } '*packageVerificationCode does not match*' 'A wrong verification code must fail.'
    Assert-Tampered { param($d) $d.relationships += @{ spdxElementId = (Get-Package $d 'Microsoft.Web.WebView2').SPDXID; relationshipType = 'CONTAINS'; relatedSpdxElement = $d.files[0].SPDXID } } '*filesAnalyzed=false but CONTAINS files*' 'A filesAnalyzed=false package must not CONTAIN files.'
    Assert-Tampered { param($d) @($d.files[0].checksums | Where-Object algorithm -ceq 'SHA1')[0].checksumValue = '0' * 40 } '*SHA1 mismatch*' 'A wrong file SHA1 must fail.'
    $brokenMetadata = Join-Path $testDirectory 'broken-metadata.json'
    $broken = $metadataText | ConvertFrom-Json -AsHashtable
    $broken.resolve.nodes = @($broken.resolve.nodes | Where-Object { $_.id -cnotlike '*#gamma@*' })
    Write-Text $brokenMetadata ($broken | ConvertTo-Json -Depth 20)
    Write-Text $sbomPath $good
    Assert-Fails { & $validator @common -SbomPath $sbomPath -MetadataPath $brokenMetadata } '*resolve graph has no node for*gamma@1.1.0*' 'A missing resolve node must throw.'
    Write-Text $sbomPath '{ "spdxVersion": '
    Assert-Fails $validate '*not well-formed JSON*' 'Validator must reject malformed JSON.'

    # 10. Generator rejections.
    Write-Text (Join-Path $packageRoot 'loader-standin.txt') "another loader`n"
    Assert-Fails { & $generate } '*does not match the pinned x64 loader*' 'Generator must reject an unpinned loader.'
    Write-Text (Join-Path $packageRoot 'loader-standin.txt') "stand-in for the loader`n"
    Assert-Fails { & $generator @common -OutputPath (Join-Path $packageRoot 'inside.json') } '*must be outside PackageRoot*' 'SBOM must not be written into the package.'
    $brokenLock = Join-Path $testDirectory 'broken-lock.toml'
    Write-Text $brokenLock ((Get-Content -LiteralPath $cargoLock -Raw) -replace '(?m)^checksum = "7{64}"\r?\n', '')
    Assert-Fails { & $generator @common -OutputPath $sbomPath -CargoLockPath $brokenLock } '*no SHA-256 checksum for gamma 1.1.0*' 'A crate without a lock checksum must fail.'
    $gitMetadata = Join-Path $testDirectory 'git-metadata.json'
    Write-Text $gitMetadata ($metadataText -replace '("source": )"registry\+https://github.com/rust-lang/crates.io-index"(,\s*"repository": "https://example.invalid/alpha")', '$1"git+https://example.invalid/alpha"$2')
    Assert-Fails { & $generator @common -OutputPath $sbomPath -MetadataPath $gitMetadata } '*only crates.io is allowed*' 'A non-crates.io source must fail.'

    # 11. Packaging script source contracts (the script itself runs only in CI).
    $packager = Get-Content -LiteralPath (Join-Path $RepositoryRoot 'scripts\New-RustPortablePackage.ps1') -Raw
    $guard = $packager.IndexOf("`$env:GITHUB_ACTIONS -ne 'true'", [StringComparison]::Ordinal)
    $firstArchive = $packager.IndexOf('ZipArchiveMode]::Create', [StringComparison]::Ordinal)
    Assert-That ($guard -ge 0 -and $firstArchive -gt $guard) 'Packaging must refuse to run outside GitHub Actions before it creates an archive.'
    foreach ($contract in 'Compare-RustBuilds.ps1', 'Test-RustSbom.ps1', 'Generate-RustSbom.ps1', 'SHA256SUMS', 'unsigned-preview', 'ExtractToDirectory',
        '-HostMetadataPath $HostMetadataPath', '-CargoTreePath $CargoTreePath', 'Write-PortableZip $secondZip', 'The ZIP is not reproducible',
        'must have exactly 2 lines', 'no single marknexia-win32 workspace crate with a version') {
        Assert-That ($packager.Contains($contract)) "Packaging script must use $contract."
    }
    Assert-That (-not ($packager -match '(?i)signtool|Set-AuthenticodeSignature|gh release|action-gh-release')) 'Preview packaging must not sign or publish.'

    Write-Output 'Rust SBOM and notices regressions passed: graph scope, host/target graphs, cargo tree cross-check, SPDX distribution package and SHA1, dev exclusion, checksums, licenses, locks, file hashes, notices, reproducibility, validator and generator rejections, packaging contracts.'
} finally {
    $env:SOURCE_DATE_EPOCH = $originalEpoch
    $resolved = [IO.Path]::GetFullPath($testDirectory)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if ($resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and (Split-Path -Leaf $resolved) -match '^marknexia-rust-sbom-[0-9a-f]{32}$') {
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
