# Headless tests for the adaptive CI change classifier (scripts/ci/ChangedAreas.psm1,
# scripts/ci/Get-ChangedAreas.ps1) and the gate jobs of the adaptive workflows (docs/ci.md).
# Uses only plain text files and a throwaway Git repository in the temp directory.
[CmdletBinding()]
param([string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $RepositoryRoot 'scripts/ci/ChangedAreas.psm1') -Force
$entry = Join-Path $RepositoryRoot 'scripts/ci/Get-ChangedAreas.ps1'
$failures = [Collections.Generic.List[string]]::new()
function Assert-That([bool]$Condition, [string]$Message) { if (-not $Condition) { $failures.Add($Message) } }
function Assert-Plan([string]$Name, [string[]]$Paths, [bool]$Dotnet, [bool]$Bridge, [bool]$Rust, [bool]$Full = $false, [bool]$DocsOnly = $false) {
    $plan = Get-CiChangePlan -Path $Paths
    $actual = "full=$($plan.full) dotnet=$($plan.dotnet) bridge=$($plan.bridge) rust=$($plan.rust) docs_only=$($plan.docs_only)"
    $expected = "full=$Full dotnet=$Dotnet bridge=$Bridge rust=$Rust docs_only=$DocsOnly"
    Assert-That ($actual -ceq $expected) "${Name}: expected '$expected', got '$actual' ($($plan.reason))"
}

$work = Join-Path ([IO.Path]::GetTempPath()) "marknexia-ci-tests-$([Guid]::NewGuid().ToString('N'))"
[IO.Directory]::CreateDirectory($work) | Out-Null
try {
    # --- Single paths ---------------------------------------------------------------------------
    $areas = [ordered]@{
        '.github/workflows/ci.yml'                               = 'ci'
        'scripts/ci/ChangedAreas.psm1'                           = 'ci'
        'tests/ci/changed_areas.tests.ps1'                       = 'ci'
        '.gitattributes'                                         = 'repo'
        'src/Marknexia.Rendering/Assets/bridge.js'               = 'web-assets'
        'src/Marknexia.Rendering/MarkdownRenderer.cs'            = 'dotnet'
        'src/Marknexia.App/README.md'                            = 'dotnet'
        'tests/Marknexia.Bridge.Tests/bridge.test.mjs'           = 'bridge'
        'tests/Marknexia.Parity.Tests/ParityExporterTests.cs'    = 'dotnet'
        'tools/Marknexia.ParityExporter/ParityExporter.cs'       = 'dotnet'
        'Marknexia.slnx'                                         = 'dotnet'
        'Directory.Packages.props'                               = 'dotnet'
        'scripts/Test-Sbom.ps1'                                  = 'dotnet'
        'compat/decisions/rendering-differences.md'              = 'compat'
        'compat/fixtures/v1/manifest.json'                       = 'compat'
        'test-fixtures/markdown/gfm/features.md'                 = 'fixtures'
        'crates/marknexia-core/src/lib.rs'                       = 'rust'
        'crates/marknexia-rendering/tests/oracle/README.md'      = 'rust'
        'fuzz/Cargo.toml'                                        = 'rust'
        'Cargo.lock'                                             = 'rust'
        '.cargo/config.toml'                                     = 'rust'
        'rust-toolchain.toml'                                    = 'rust'
        'deny.toml'                                              = 'rust'
        'tools/rust-tools.lock'                                  = 'rust'
        'tests/perf/rust_sbom.tests.ps1'                         = 'rust'
        'tests/rust/workflow_contract.ps1'                       = 'rust'
        'scripts/New-RustPortablePackage.ps1'                    = 'rust'
        'scripts/Stage-WebView2Loader.ps1'                       = 'rust'
        'LICENSE'                                                = 'rust'
        'packaging/rust-msix/AppxManifest.xml'                   = 'packaging'
        'packaging/webview/README.md'                            = 'packaging'
        'packaging/signing/release-signing.json'                 = 'packaging'
        'store-submission/assets/logo.png'                       = 'packaging'
        'scripts/signing/MarknexiaSigning.psm1'                  = 'packaging'
        'tests/signing/release_signing.tests.ps1'                = 'packaging'
        'scripts/Invoke-ReleaseSigning.ps1'                      = 'packaging'
        'docs/ci.md'                                             = 'docs'
        'docs/index.html'                                        = 'docs'
        'implementation-docs/09-testing.md'                      = 'docs'
        'branding-docs/README.md'                                = 'docs'
        'README.md'                                              = 'docs'
        'artifacts/rust-feasibility/manifest.json'               = 'unknown'
        'newtool/notes.md'                                       = 'unknown'
        'scripts/build-assets.js'                                = 'unknown'
        'scripts/Some-NewScript.ps1'                             = 'unknown'
    }
    foreach ($item in $areas.GetEnumerator()) {
        $actual = Get-CiPathArea $item.Key
        Assert-That ($actual -ceq $item.Value) "Area of $($item.Key): expected $($item.Value), got $actual."
    }
    Assert-That ((Get-CiPathArea '.\docs\ci.md') -ceq 'docs') 'Windows separators and a leading ./ are normalised.'
    Assert-That ((Get-CiPathArea 'Src/Marknexia.Core/X.cs') -ceq 'dotnet') 'Matching ignores case.'

    # --- Plans --------------------------------------------------------------------------------
    #          name                 paths                                                         dotnet bridge rust   full   docsOnly
    Assert-Plan 'docs-only'         @('docs/release-signing.md', 'README.md', 'docs/assets/a.png') $false $false $false $false $true
    Assert-Plan 'rust-only'         @('crates/marknexia-core/src/lib.rs', 'Cargo.lock')            $false $false $true  $false $false
    Assert-Plan 'dotnet-only'       @('src/Marknexia.Core/Document.cs', 'tests/Marknexia.Core.Tests/A.cs') $true $false $false $false $false
    Assert-Plan 'bridge-only'       @('tests/Marknexia.Bridge.Tests/bridge.test.mjs')              $false $true  $false $false $false
    Assert-Plan 'web assets'        @('src/Marknexia.Rendering/Assets/github-markdown.css')        $true  $true  $true  $false $false
    Assert-Plan 'compat record'     @('compat/decisions/math-differences.md')                      $true  $false $true  $false $false
    Assert-Plan 'parity fixture'    @('test-fixtures/markdown/navigation/README.md')               $true  $false $true  $false $false
    Assert-Plan 'signing'           @('scripts/signing/MarknexiaSigning.psm1')                     $false $false $true  $false $false
    Assert-Plan 'workflow change'   @('.github/workflows/rust-feasibility.yml', 'docs/ci.md')      $true  $true  $true  $true  $false
    Assert-Plan 'ci script change'  @('scripts/ci/Get-ChangedAreas.ps1')                           $true  $true  $true  $true  $false
    Assert-Plan 'unknown path'      @('docs/ci.md', 'newtool/run.ps1')                             $true  $true  $true  $true  $false
    Assert-Plan 'git config'        @('.gitattributes')                                            $true  $true  $true  $true  $false
    Assert-Plan 'docs and rust'     @('docs/ci.md', 'crates/marknexia-files/src/path.rs')          $false $false $true  $false $false
    Assert-Plan 'empty diff'        @()                                                            $true  $true  $true  $true  $false

    $forced = Get-CiChangePlan -Path @('newtool/run.ps1')
    Assert-That ($forced.reason -match 'newtool/run\.ps1 \(unknown\)') "A forced run names the path that forced it ($($forced.reason))."

    # --- Entry script -------------------------------------------------------------------------
    $out = Join-Path $work 'output.txt'
    $summary = Join-Path $work 'summary.md'
    foreach ($event in 'push', 'merge_group', 'schedule', 'workflow_dispatch') {
        Set-Content -LiteralPath $out -Value $null
        & $entry -EventName $event -OutputPath $out -SummaryPath $summary 6>$null | Out-Null
        $text = Get-Content -LiteralPath $out -Raw
        Assert-That ($text -match '(?m)^full=true\r?$' -and $text -match '(?m)^rust=true\r?$' -and $text -match '(?m)^dotnet=true\r?$' -and $text -match '(?m)^bridge=true\r?$') "Event $event must run everything: $text"
    }
    Set-Content -LiteralPath $out -Value $null
    & $entry -Path @('docs/ci.md') -OutputPath $out -SummaryPath $summary 6>$null | Out-Null
    $text = Get-Content -LiteralPath $out -Raw
    Assert-That ($text -match '(?m)^full=false\r?$' -and $text -match '(?m)^docs_only=true\r?$' -and $text -match '(?m)^rust=false\r?$' -and $text -match '(?m)^areas=docs\r?$') "Entry script output for docs-only: $text"
    Assert-That ((Get-Content -LiteralPath $summary -Raw) -match '\| rust \| false \|') 'The step summary lists the job groups.'

    # --- Pull request diff in a throwaway repository ------------------------------------------
    $repo = Join-Path $work 'repo'
    [IO.Directory]::CreateDirectory($repo) | Out-Null
    $git = { & git -C $repo -c user.name=ci -c user.email=ci@example.invalid -c commit.gpgsign=false @args 2>&1 | Out-Null; if ($LASTEXITCODE -ne 0) { throw "git $args failed" } }
    & $git init -q
    [IO.Directory]::CreateDirectory((Join-Path $repo 'docs')) | Out-Null
    [IO.Directory]::CreateDirectory((Join-Path $repo 'crates/x')) | Out-Null
    Set-Content -LiteralPath (Join-Path $repo 'docs/a.md') -Value 'a'
    Set-Content -LiteralPath (Join-Path $repo 'crates/x/old.txt') -Value 'x'
    & $git add -A
    & $git commit -q -m base
    Set-Content -LiteralPath (Join-Path $repo 'docs/a.md') -Value 'b'
    & $git commit -q -am docs
    Push-Location $repo
    try {
        Set-Content -LiteralPath $out -Value $null
        & $entry -EventName pull_request -OutputPath $out -SummaryPath $summary 6>$null | Out-Null
        $text = Get-Content -LiteralPath $out -Raw
        Assert-That ($text -match '(?m)^docs_only=true\r?$' -and $text -match '(?m)^full=false\r?$') "A docs-only pull request diff skips the heavy jobs: $text"

        # A rename out of crates/ must still count the old path (--no-renames).
        & $git mv crates/x/old.txt docs/old.txt
        & $git commit -q -m move
        Set-Content -LiteralPath $out -Value $null
        & $entry -EventName pull_request -OutputPath $out -SummaryPath $summary 6>$null | Out-Null
        $text = Get-Content -LiteralPath $out -Raw
        Assert-That ($text -match '(?m)^rust=true\r?$') "A file moved out of crates/ still runs Rust: $text"

        Set-Content -LiteralPath $out -Value $null
        & $entry -EventName pull_request -Base 'no-such-revision' -OutputPath $out -SummaryPath $summary 6>$null 2>$null 3>$null | Out-Null
        $text = Get-Content -LiteralPath $out -Raw
        Assert-That ($text -match '(?m)^full=true\r?$') "A failed diff runs everything: $text"
    } finally {
        Pop-Location
    }

    # --- Gate jobs ----------------------------------------------------------------------------
    foreach ($workflow in @(
            @{ File = 'ci.yml'; Gate = 'ci-result'; Name = 'CI result' },
            @{ File = 'rust-feasibility.yml'; Gate = 'rust-ci-result'; Name = 'Rust CI result' })) {
        $lines = Get-Content -LiteralPath (Join-Path $RepositoryRoot ".github/workflows/$($workflow.File)")
        $start = [Array]::IndexOf($lines, 'jobs:')
        $jobs = @($lines[($start + 1)..($lines.Count - 1)] | Where-Object { $_ -match '^  ([A-Za-z0-9_-]+):\s*$' } | ForEach-Object { $Matches[1] })
        Assert-That ($jobs -contains 'changes') "$($workflow.File) has a changes job."
        Assert-That ($jobs -contains $workflow.Gate) "$($workflow.File) has the $($workflow.Gate) gate job."
        $gateIndex = [Array]::IndexOf($lines, "  $($workflow.Gate):")
        $gate = ($lines[$gateIndex..([Math]::Min($gateIndex + 12, $lines.Count - 1))] -join "`n")
        Assert-That ($gate -match "name: $([regex]::Escape($workflow.Name))") "$($workflow.Gate) is named '$($workflow.Name)'."
        Assert-That ($gate -match 'if: always\(\)') "$($workflow.Gate) runs always."
        $needs = if ($gate -match 'needs: \[([^\]]+)\]') { @($Matches[1] -split ',' | ForEach-Object { $_.Trim() }) } else { @() }
        foreach ($job in $jobs | Where-Object { $_ -ne $workflow.Gate }) {
            Assert-That ($needs -contains $job) "$($workflow.Gate) in $($workflow.File) must need $job."
        }
        $text = $lines -join "`n"
        Assert-That ($text -match 'cancel-in-progress: \$\{\{ github\.event_name == ''pull_request'' \}\}') "$($workflow.File) cancels superseded runs for pull requests only."
        Assert-That ($text -match '(?m)^  merge_group:' -and $text -match '(?m)^  schedule:' -and $text -match '(?m)^  workflow_dispatch:') "$($workflow.File) runs for the merge queue, on a schedule and on demand."
    }

    # --- Informational: tracked files that would force a full run -----------------------------
    $tracked = @(& git -C $RepositoryRoot ls-files 2>$null)
    if ($LASTEXITCODE -eq 0 -and $tracked.Count) {
        $unknown = @($tracked | Where-Object { (Get-CiPathArea $_) -eq 'unknown' })
        if ($unknown.Count) { Write-Host "Tracked paths without an area (a change runs everything): $($unknown -join ', ')" }
    }
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failures.Count -gt 0) { throw "Changed-area tests failed:`n$($failures -join "`n")" }
Write-Host 'Changed-area tests passed: path areas, plans (docs-only, Rust-only, .NET-only, bridge, web assets, compat, fixtures, signing, workflow, unknown, empty), entry script events, pull request diffs and gate jobs.'
