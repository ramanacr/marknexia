#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'
$output = (Resolve-Path -LiteralPath $OutputDirectory).Path

# Missing generated XAML resources can produce a successful build followed by
# an InvalidCastException at startup. Check the deliverable, not project text.
foreach ($name in @('Marknexia.App.exe', 'Marknexia.App.dll', 'Marknexia.App.pri', 'marknexia-sbom.spdx.json')) {
    $path = Join-Path $output $name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf) -or (Get-Item -LiteralPath $path).Length -eq 0) {
        throw "Incomplete app output: missing or empty $name in $output"
    }
}

$sbom = Get-Content -LiteralPath (Join-Path $output 'marknexia-sbom.spdx.json') -Raw | ConvertFrom-Json
if ($sbom.spdxVersion -ne 'SPDX-2.3' -or -not ($sbom.packages | Where-Object name -EQ 'Marknexia')) {
    throw 'App output does not contain the Marknexia SPDX 2.3 inventory.'
}
Write-Host "App output checks passed: executable, assembly, generated PRI, and SBOM in $output"
