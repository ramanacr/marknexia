#requires -Version 7.0
<#
.SYNOPSIS
Signs Marknexia release files on the owner's PC with the Certum cloud certificate
(SimplySign Desktop). See docs/release-signing.md for the full procedure.

.DESCRIPTION
Stages (run in this order for one release):

  Binaries  Stage 2. Downloads the unsigned binaries of a green CI run (-RunId), signs them,
            verifies them and writes them to <OutputRoot>\signed. With -Publish it uploads
            them to a private DRAFT release named signing-handoff-<product>-<run id>, which
            the "Signed packages" workflow (stage 3) reads. It prints that workflow command.
  Packages  Stage 4. Downloads the packages that the "Signed packages" run (-RunId) built,
            checks them against the handoff, signs the setup executables and the MSIX
            packages, verifies them and regenerates the checksums in <OutputRoot>\release.
            With -Publish it creates the GitHub release: a rust-preview-* pre-release
            (-Tag is required) or the v* release of the .NET edition (the tag that built it).
  Cleanup   Removes the handoff draft of source run -RunId (only with -Publish).

Without -Publish every stage is a dry run: nothing leaves the PC and nothing on GitHub
changes. Every stage can be run again after a failure; finished files are skipped.

Before Binaries or Packages: open SimplySign Desktop and log in. The script checks the
certificate and makes one test signature on a throw-away script before it signs anything.
It never asks for, stores or prints a password, a one-time code or a token. gh must be
logged in (gh auth login) with write access to the repository.

.EXAMPLE
pwsh scripts/Invoke-ReleaseSigning.ps1 -Product Rust -Stage Binaries -RunId 42400000000
.EXAMPLE
pwsh scripts/Invoke-ReleaseSigning.ps1 -Product Rust -Stage Packages -RunId 42400000001 -Tag rust-preview-0.2.0 -Publish
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('Rust', 'DotNet')][string]$Product,
    [Parameter(Mandatory)][ValidateSet('Binaries', 'Packages', 'Cleanup')][string]$Stage,
    [Parameter(Mandatory)][ValidatePattern('^\d+$')][string]$RunId,
    [string]$Tag,
    [ValidatePattern('^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$')][string]$Repository = 'ramanacr/marknexia',
    [string]$OutputRoot,
    [switch]$Publish,
    [switch]$SkipSessionProbe
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'signing\MarknexiaSigning.psm1') -Force

# Argument checks come first: nothing is signed for a command that would be refused.
if ($Tag -and $Stage -ne 'Packages') { throw '-Tag is used only by the Packages stage.' }
if ($Stage -eq 'Packages' -and $Product -eq 'Rust' -and -not $Tag) { throw 'Give -Tag rust-preview-<version> for the Rust edition.' }
$config = Get-ReleaseSigningConfig
$spec = Get-ProductSpec -Product $Product
if (-not $OutputRoot) {
    $OutputRoot = Join-Path $env:LOCALAPPDATA "Marknexia\release-signing\$($spec.Product)-$($Stage.ToLowerInvariant())-$RunId"
}
$OutputRoot = [IO.Path]::GetFullPath($OutputRoot)

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { throw 'GitHub CLI (gh) is required. Install it and run gh auth login.' }
$PSNativeCommandUseErrorActionPreference = $false
gh auth status --hostname github.com *> $null
if ($LASTEXITCODE -ne 0) { throw 'gh is not logged in. Run gh auth login first.' }

$signTool = $null
if ($Stage -ne 'Cleanup') {
    $signTool = Find-SignTool
    Write-Host "signtool: $signTool"
}
$tools = New-SigningTools -SignToolPath $signTool
if ($Stage -ne 'Cleanup') {
    Assert-SigningCertificate -Config $config
    if (-not $SkipSessionProbe) { Test-SigningSession -Tools $tools -Config $config -WorkDirectory (Join-Path $OutputRoot 'probe') }
    Write-Host "Release certificate ready: $($config.subject) ($($config.thumbprint))"
}

$context = @{ Tools = $tools; Config = $config; Spec = $spec; Repository = $Repository; OutputRoot = $OutputRoot; Publish = [bool]$Publish }
Write-Host "Product $($spec.Product), stage $Stage, run $RunId, output $OutputRoot$(if (-not $Publish) { ' (dry run)' })"
switch ($Stage) {
    'Binaries' {
        $result = Invoke-BinariesStage -Context $context -SourceRunId $RunId
        if ($result.Uploaded) { Write-Host "`nNext: $($result.NextCommand)" }
    }
    'Packages' { Invoke-PackagesStage -Context $context -PackagingRunId $RunId -Tag $Tag | Out-Null }
    'Cleanup' { Invoke-CleanupStage -Context $context -SourceRunId $RunId | Out-Null }
}
