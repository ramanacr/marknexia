# Decides which CI job groups run (docs/ci.md). Used by the "changes" job of
# .github/workflows/ci.yml and .github/workflows/rust-feasibility.yml.
#
# - pull_request: classifies `git diff --name-only --no-renames HEAD^1 HEAD`. On a pull
#   request, actions/checkout checks out the test merge commit, whose first parent is the
#   base branch tip, so the diff is exactly what the pull request changes.
# - Every other event (push, merge_group, schedule, workflow_dispatch): full run.
# - Any Git failure: full run.
#
# Writes full, dotnet, bridge, rust, docs_only, areas and reason to GITHUB_OUTPUT and a table
# to GITHUB_STEP_SUMMARY when those files are set. -Path classifies a given list instead of
# Git (for local checks).
[CmdletBinding()]
param(
    [string]$EventName = $env:GITHUB_EVENT_NAME,
    [string]$Base = 'HEAD^1',
    [string]$Head = 'HEAD',
    [string[]]$Path,
    [string]$OutputPath = $env:GITHUB_OUTPUT,
    [string]$SummaryPath = $env:GITHUB_STEP_SUMMARY
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'ChangedAreas.psm1') -Force

if ($PSBoundParameters.ContainsKey('Path')) {
    $plan = Get-CiChangePlan -Path $Path
} elseif ($EventName -ne 'pull_request') {
    $plan = Get-CiChangePlan -Full -Reason "event '$EventName' always runs everything"
} else {
    $changed = @(& git -c core.quotePath=false diff --name-only --no-renames $Base $Head)
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "git diff $Base $Head failed with exit code $LASTEXITCODE."
        $plan = Get-CiChangePlan -Full -Reason "git diff $Base $Head failed"
    } else {
        $plan = Get-CiChangePlan -Path ([string[]]$changed)
    }
}

$bool = { param($value) if ($value) { 'true' } else { 'false' } }
$lines = @(
    "full=$(& $bool $plan.full)"
    "dotnet=$(& $bool $plan.dotnet)"
    "bridge=$(& $bool $plan.bridge)"
    "rust=$(& $bool $plan.rust)"
    "docs_only=$(& $bool $plan.docs_only)"
    "areas=$($plan.areas -join ',')"
    "reason=$($plan.reason -replace '[\r\n]+', ' ')"
)
$lines | ForEach-Object { Write-Host $_ }
if ($OutputPath) { $lines | Add-Content -LiteralPath $OutputPath -Encoding utf8 }

if ($SummaryPath) {
    $summary = [Collections.Generic.List[string]]::new()
    $summary.Add('### Changed areas')
    $summary.Add('')
    $summary.Add("$($plan.reason)")
    $summary.Add('')
    $summary.Add('| Job group | Runs |')
    $summary.Add('| --- | --- |')
    foreach ($group in 'dotnet', 'bridge', 'rust') { $summary.Add("| $group | $(& $bool $plan.$group) |") }
    if ($plan.files.Count) {
        $summary.Add('')
        $summary.Add('<details><summary>Classified files</summary>')
        $summary.Add('')
        $summary.Add('| File | Area |')
        $summary.Add('| --- | --- |')
        foreach ($entry in $plan.files.GetEnumerator() | Select-Object -First 300) { $summary.Add("| ``$($entry.Key)`` | $($entry.Value) |") }
        $summary.Add('')
        $summary.Add('</details>')
    }
    $summary | Add-Content -LiteralPath $SummaryPath -Encoding utf8
}
