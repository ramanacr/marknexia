#requires -Version 7.0
<#
.SYNOPSIS
CI only (stage 3 of docs/release-signing.md): verifies the binaries that the owner signed
locally for one architecture, then lays out the package input folder with them.

.DESCRIPTION
Runs only on GitHub Actions runners: it copies release executables.

1. The handoff manifest (signing-manifest.json from the draft release) must name this
   product, the source run and commit, and the release certificate in
   packaging/signing/release-signing.json.
2. The PE files in the source run's unsigned artifact (UnsignedRoot) are planned again with
   the same rules as the local stage. The set the owner signed must equal the manifest's
   files, and the set kept with a third-party signature must equal the manifest's kept list.
3. For every signed file: the handoff copy has the manifest SHA-256; the unsigned copy has
   the manifest's unsigned SHA-256; both have the same PE content digest (equal except for
   the signature); the Authenticode signature is valid, by the expected thumbprint, and
   timestamped; signtool verify /pa passes.
4. DestinationRoot receives the unsigned tree with the signed files in place. Every PE in it
   is then either signed by the release certificate or kept with its valid third-party
   signature, and nothing is unsigned.
Any failure stops the job.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('rust', 'dotnet')][string]$Product,
    [Parameter(Mandatory)][ValidateSet('x64', 'arm64')][string]$Architecture,
    [Parameter(Mandatory)][string]$ManifestPath,
    [Parameter(Mandatory)][string]$HandoffDir,
    [Parameter(Mandatory)][string]$UnsignedRoot,
    [Parameter(Mandatory)][string]$DestinationRoot,
    [Parameter(Mandatory)][ValidatePattern('^\d+$')][string]$SourceRunId,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{40}$')][string]$SourceCommit,
    [string]$EvidencePath
)

$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') {
    throw 'Test-SignedHandoff.ps1 runs only on GitHub Actions runners. It copies release executables.'
}
Import-Module (Join-Path $PSScriptRoot 'signing\MarknexiaSigning.psm1') -Force
$config = Get-ReleaseSigningConfig
$spec = Get-ProductSpec -Product $(if ($Product -eq 'rust') { 'Rust' } else { 'DotNet' })
$signTool = $null
try { $signTool = Find-SignTool } catch { throw "signtool is required on the runner: $_" }
$tools = New-SigningTools -SignToolPath $signTool

$manifest = Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json
if ($manifest.schemaVersion -cne 'marknexia-signing-handoff-v1') { throw 'Unknown handoff manifest schema.' }
if ($manifest.product -cne $spec.Product) { throw "The handoff is for $($manifest.product), not $($spec.Product)." }
if ("$($manifest.sourceRunId)" -cne $SourceRunId) { throw "The handoff is for run $($manifest.sourceRunId), not $SourceRunId." }
if ($manifest.sourceCommit -cne $SourceCommit) { throw "The handoff is for commit $($manifest.sourceCommit), not $SourceCommit." }
if ($manifest.signer.thumbprint -cne $config.thumbprint -or $manifest.signer.certificateSha256 -cne $config.certificateSha256) {
    throw 'The handoff names another certificate than packaging/signing/release-signing.json.'
}

$entries = @($manifest.files | Where-Object { $_.architecture -ceq $Architecture })
$keptEntries = @($manifest.kept | Where-Object { $_.architecture -ceq $Architecture })
if ($entries.Count -eq 0) { throw "The handoff has no signed files for $Architecture." }

function Join-Ordinal([string[]]$Items) { $copy = [string[]]@($Items); [Array]::Sort($copy, [StringComparer]::Ordinal); $copy -join '|' }

# 2. Plan the unsigned artifact again; the owner's choices must match.
$plan = @(Get-SigningPlan -Tools $tools -Config $config -Spec $spec -Root $UnsignedRoot)
if (@($plan | Where-Object Action -eq 'AlreadySigned').Count -gt 0) { throw 'The unsigned CI artifact already holds release-signed files.' }
$toSign = @($plan | Where-Object Action -eq 'Sign' | ForEach-Object RelativePath)
$toKeep = @($plan | Where-Object Action -eq 'Keep' | ForEach-Object RelativePath)
if ((Join-Ordinal $toSign) -cne (Join-Ordinal @($entries | ForEach-Object path))) {
    throw "Signed files differ from the unsigned PE files.`nExpected: $(Join-Ordinal $toSign)`nHandoff:  $(Join-Ordinal @($entries | ForEach-Object path))"
}
if ((Join-Ordinal $toKeep) -cne (Join-Ordinal @($keptEntries | ForEach-Object path))) { throw 'Kept third-party files differ from the handoff manifest.' }

# 3. Every signed file.
$evidence = [Collections.Generic.List[object]]::new()
foreach ($entry in $entries) {
    $signed = Join-Path $HandoffDir $entry.handoffAsset
    $unsigned = Join-Path $UnsignedRoot $entry.path
    if (-not (Test-Path -LiteralPath $signed)) { throw "Handoff asset $($entry.handoffAsset) is missing." }
    if ((Get-FileSha256 $signed) -cne $entry.signedSha256) { throw "$($entry.handoffAsset) does not have the manifest SHA-256." }
    if ((Get-FileSha256 $unsigned) -cne $entry.unsignedSha256) { throw "$($entry.path) in the source artifact does not have the manifest SHA-256." }
    $signedContent = Get-PeContentDigest -Path $signed
    if ($signedContent -cne (Get-PeContentDigest -Path $unsigned) -or $signedContent -cne $entry.contentSha256) {
        throw "$($entry.path) differs from the CI build beyond its signature."
    }
    Assert-ReleaseSignature -Tools $tools -Config $config -Path $signed
    $evidence.Add([ordered]@{ path = $entry.path; signedSha256 = $entry.signedSha256; unsignedSha256 = $entry.unsignedSha256; contentSha256 = $signedContent })
}
foreach ($entry in $keptEntries) {
    if ((Get-FileSha256 (Join-Path $UnsignedRoot $entry.path)) -cne $entry.sha256) { throw "Kept file $($entry.path) differs from the manifest." }
}

# 4. Destination: the unsigned tree with the signed files in place.
if (Test-Path -LiteralPath $DestinationRoot) {
    if (@(Get-ChildItem -LiteralPath $DestinationRoot -Force).Count -gt 0) { throw "$DestinationRoot must be empty." }
} else { [IO.Directory]::CreateDirectory($DestinationRoot) | Out-Null }
$unsignedFull = [IO.Path]::GetFullPath($UnsignedRoot)
foreach ($file in Get-ChildItem -LiteralPath $unsignedFull -File -Recurse) {
    $relative = [IO.Path]::GetRelativePath($unsignedFull, $file.FullName)
    $target = Join-Path $DestinationRoot $relative
    [IO.Directory]::CreateDirectory((Split-Path $target -Parent)) | Out-Null
    Copy-Item -LiteralPath $file.FullName -Destination $target
}
foreach ($entry in $entries) { Copy-Item -LiteralPath (Join-Path $HandoffDir $entry.handoffAsset) -Destination (Join-Path $DestinationRoot $entry.path) -Force }
$final = @(Get-SigningPlan -Tools $tools -Config $config -Spec $spec -Root $DestinationRoot)
$unsignedLeft = @($final | Where-Object Action -eq 'Sign')
if ($unsignedLeft.Count -gt 0) { throw "Unsigned PE files remain: $(($unsignedLeft | ForEach-Object RelativePath) -join ', ')" }
if ((Join-Ordinal @($final | Where-Object Action -eq 'AlreadySigned' | ForEach-Object RelativePath)) -cne (Join-Ordinal @($entries | ForEach-Object path))) {
    throw 'The release-signed files in the destination differ from the handoff.'
}

$summary = [ordered]@{
    schemaVersion = 'marknexia-signed-handoff-check-v1'; product = $spec.Product; architecture = $Architecture
    sourceRunId = $SourceRunId; sourceCommit = $SourceCommit; signerThumbprint = $config.thumbprint
    signed = $evidence.ToArray(); kept = @($keptEntries | ForEach-Object { [ordered]@{ path = $_.path; signerSubject = $_.signerSubject } })
}
if ($EvidencePath) { $summary | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $EvidencePath -Encoding utf8NoBOM }
Write-Output "Signed handoff verified for $($spec.Product) $Architecture`: $($entries.Count) release-signed, $($keptEntries.Count) kept."
