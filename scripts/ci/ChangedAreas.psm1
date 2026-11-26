# Change classification for the adaptive CI (docs/ci.md).
#
# Each changed path maps to exactly one area: the first rule that matches wins. Each area
# maps to the job groups that must run. Areas "ci", "repo" and "unknown" force a full run:
# a path that no rule knows is never assumed to be harmless.
#
# The rules follow what the code reads, not only where it lives:
# - The Rust tests read compat/fixtures/** and every compat/decisions/*.md
#   (crates/marknexia-markdown/tests/parity_v1.rs), and the .NET parity tests read
#   compat/fixtures/v1 (tests/Marknexia.Parity.Tests). compat/** therefore runs both.
# - test-fixtures/** is read by the .NET navigation tests and by the Rust release build
#   (features.md digest). It runs both.
# - src/Marknexia.Rendering/Assets/** is read by the .NET renderer, by the Playwright bridge
#   tests and by the Rust build (crates/marknexia-webview/build.rs embeds mermaid.min.js;
#   crates/marknexia-rendering/tests/support/asset_patches.rs compares the assets).
# - LICENSE is read by scripts/Generate-RustSbom.ps1; store-submission/** by
#   scripts/New-RustInstallerPackages.ps1.
# - Markdown files are documentation only at the repository root and under docs/,
#   branding-docs/ and implementation-docs/. Markdown anywhere else follows its directory.

Set-StrictMode -Version 3.0

# Ordered rules: the first match wins. Patterns are matched case-insensitively against
# repository-relative paths with forward slashes.
$script:Rules = @(
    # CI definitions and the scripts and tests of CI itself: always a full run.
    @{ Area = 'ci';         Pattern = '^\.github/' }
    @{ Area = 'ci';         Pattern = '^scripts/ci/' }
    @{ Area = 'ci';         Pattern = '^tests/ci/' }
    # Repository-wide Git configuration changes line endings and tracked files for every
    # build and digest (for example packaging/webview/mermaid.lock.json, parity fixtures).
    @{ Area = 'repo';       Pattern = '^\.gitattributes$' }
    @{ Area = 'repo';       Pattern = '^\.gitignore$' }
    # Web assets shared by the .NET renderer, the bridge tests and the Rust build.
    @{ Area = 'web-assets'; Pattern = '^src/Marknexia\.Rendering/Assets/' }
    # Playwright bridge tests.
    @{ Area = 'bridge';     Pattern = '^tests/Marknexia\.Bridge\.Tests/' }
    # Parity records and fixtures that both test suites read.
    @{ Area = 'compat';     Pattern = '^compat/' }
    @{ Area = 'fixtures';   Pattern = '^test-fixtures/' }
    # .NET edition.
    @{ Area = 'dotnet';     Pattern = '^src/' }
    @{ Area = 'dotnet';     Pattern = '^tests/Marknexia\.[^/]+\.Tests/' }
    @{ Area = 'dotnet';     Pattern = '^tools/Marknexia\.ParityExporter/' }
    @{ Area = 'dotnet';     Pattern = '^[^/]+\.slnx?$' }
    @{ Area = 'dotnet';     Pattern = '^Directory\.(Build|Packages)\.(props|targets)$' }
    @{ Area = 'dotnet';     Pattern = '^global\.json$' }
    @{ Area = 'dotnet';     Pattern = '^nuget\.config$' }
    @{ Area = 'dotnet';     Pattern = '^scripts/(Build-App|Test-AppOutput|Generate-Sbom|Test-Sbom|set-version|Generate-MarkdownFileIcons|Test-UiContracts)\.ps1$' }
    # Rust edition.
    @{ Area = 'rust';       Pattern = '^crates/' }
    @{ Area = 'rust';       Pattern = '^fuzz/' }
    @{ Area = 'rust';       Pattern = '^Cargo\.(toml|lock)$' }
    @{ Area = 'rust';       Pattern = '^\.cargo/' }
    @{ Area = 'rust';       Pattern = '^rust-toolchain(\.toml)?$' }
    @{ Area = 'rust';       Pattern = '^deny\.toml$' }
    @{ Area = 'rust';       Pattern = '^tools/rust-tools\.lock$' }
    @{ Area = 'rust';       Pattern = '^tests/(rust|perf)/' }
    @{ Area = 'rust';       Pattern = '^scripts/[^/]*Rust[^/]*\.ps1$' }
    @{ Area = 'rust';       Pattern = '^scripts/Stage-WebView2Loader\.ps1$' }
    @{ Area = 'rust';       Pattern = '^LICENSE$' }
    # Packaging and signing. tests/signing runs in the Rust quality job; packaging/ and
    # store-submission/ feed the Rust packages built in the Rust release build job.
    @{ Area = 'packaging';  Pattern = '^packaging/' }
    @{ Area = 'packaging';  Pattern = '^store-submission/' }
    @{ Area = 'packaging';  Pattern = '^scripts/signing/' }
    @{ Area = 'packaging';  Pattern = '^tests/signing/' }
    @{ Area = 'packaging';  Pattern = '^scripts/(Invoke-ReleaseSigning|Test-SignedHandoff|prepare-store-package|Build-Installer|Test-Installer|Test-MsixPackage)\.ps1$' }
    # Documentation that no test, build or package reads.
    @{ Area = 'docs';       Pattern = '^docs/' }
    @{ Area = 'docs';       Pattern = '^branding-docs/' }
    @{ Area = 'docs';       Pattern = '^implementation-docs/' }
    @{ Area = 'docs';       Pattern = '^[^/]+\.md$' }
)

# Job groups that each area runs. 'full' runs every group.
$script:AreaGroups = @{
    'ci'         = @('full')
    'repo'       = @('full')
    'unknown'    = @('full')
    'web-assets' = @('dotnet', 'bridge', 'rust')
    'bridge'     = @('bridge')
    'compat'     = @('dotnet', 'rust')
    'fixtures'   = @('dotnet', 'rust')
    'dotnet'     = @('dotnet')
    'rust'       = @('rust')
    'packaging'  = @('rust')
    'docs'       = @()
}

$script:Groups = @('dotnet', 'bridge', 'rust')

function ConvertTo-CiRepositoryPath {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Path)
    $normalized = $Path.Trim().Trim('"').Replace('\', '/')
    while ($normalized.StartsWith('./')) { $normalized = $normalized.Substring(2) }
    return $normalized.TrimStart('/')
}

function Get-CiPathArea {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$Path)
    $normalized = ConvertTo-CiRepositoryPath $Path
    foreach ($rule in $script:Rules) {
        if ($normalized -match $rule.Pattern) { return $rule.Area }
    }
    return 'unknown'
}

# Returns the plan: which groups run and why. -Full forces every group (events that always
# run everything). An empty change list is treated as a full run: nothing is skipped on the
# strength of a diff that found nothing.
function Get-CiChangePlan {
    [CmdletBinding()]
    param(
        [AllowEmptyCollection()][string[]]$Path = @(),
        [switch]$Full,
        [string]$Reason = ''
    )
    $paths = @($Path | ForEach-Object { ConvertTo-CiRepositoryPath $_ } | Where-Object { $_ })
    $files = [ordered]@{}
    foreach ($p in $paths) { $files[$p] = Get-CiPathArea $p }
    $areas = @($files.Values | Sort-Object -Unique)

    $full = [bool]$Full
    if (-not $full -and $paths.Count -eq 0) {
        $full = $true
        $Reason = 'no changed files were detected'
    }
    $selected = [Collections.Generic.HashSet[string]]::new()
    foreach ($area in $areas) {
        foreach ($group in $script:AreaGroups[$area]) { [void]$selected.Add($group) }
    }
    if ($selected.Contains('full')) {
        $full = $true
        if (-not $Reason) {
            $forcing = @($files.GetEnumerator() | Where-Object { $script:AreaGroups[$_.Value] -contains 'full' } | Select-Object -First 3 | ForEach-Object { "$($_.Key) ($($_.Value))" })
            $Reason = "full run forced by: $($forcing -join ', ')"
        }
    }
    if (-not $Reason) { $Reason = if ($areas.Count) { "changed areas: $($areas -join ', ')" } else { 'no changed areas' } }

    $plan = [ordered]@{
        full      = $full
        dotnet    = $full -or $selected.Contains('dotnet')
        bridge    = $full -or $selected.Contains('bridge')
        rust      = $full -or $selected.Contains('rust')
        docs_only = (-not $full) -and $areas.Count -gt 0 -and (@($areas | Where-Object { $_ -ne 'docs' }).Count -eq 0)
        areas     = $areas
        reason    = $Reason
        files     = $files
    }
    return [pscustomobject]$plan
}

Export-ModuleMember -Function ConvertTo-CiRepositoryPath, Get-CiPathArea, Get-CiChangePlan
