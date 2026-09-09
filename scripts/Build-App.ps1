#requires -Version 7.0
[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')][string]$Configuration = 'Release',
    [ValidateSet('x64', 'ARM64')][string]$Platform = 'x64',
    [ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version = '',
    [switch]$NoRestore,
    [switch]$Rebuild
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
$project = Join-Path $repoRoot 'src/Marknexia.App/Marknexia.App.csproj'
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere)) {
    throw 'Visual Studio Installer (vswhere) is required. Install Visual Studio Build Tools with Windows app development support.'
}

# dotnet MSBuild does not ship the Visual Studio PRI packaging tasks. Select
# an installation with both MSBuild and those tasks, not simply the newest IDE.
$installations = @(& $vswhere -all -products '*' -sort -property installationPath)
if ($LASTEXITCODE -ne 0) { throw "Visual Studio discovery failed ($LASTEXITCODE)." }
$msbuild = $null
foreach ($installation in $installations) {
    $candidate = Join-Path $installation 'MSBuild/Current/Bin/amd64/MSBuild.exe'
    $tasksRoot = Join-Path $installation 'MSBuild/Microsoft/VisualStudio'
    if (-not (Test-Path -LiteralPath $candidate) -or -not (Test-Path -LiteralPath $tasksRoot)) { continue }
    $priTasks = Get-ChildItem -LiteralPath $tasksRoot -Directory | Where-Object {
        Test-Path -LiteralPath (Join-Path $_.FullName 'AppxPackage/Microsoft.Build.Packaging.Pri.Tasks.dll')
    }
    if ($priTasks) { $msbuild = $candidate; break }
}
if (-not $msbuild) {
    throw 'No Visual Studio MSBuild installation has the Windows PRI packaging tasks. Install the Windows app development build components; do not disable resource generation.'
}

$target = if ($Rebuild) { 'Rebuild' } else { 'Build' }
$buildArgs = @($project, "/t:$target", "/p:Configuration=$Configuration", "/p:Platform=$Platform", '/verbosity:minimal', '/nr:false')
if (-not [string]::IsNullOrWhiteSpace($Version)) {
    $versionParts = $Version.Split('.')
    $buildArgs += "/p:Version=$Version"
    $buildArgs += "/p:PackageVersion=$Version"
    $buildArgs += "/p:VersionMajor=$($versionParts[0])"
    $buildArgs += "/p:VersionMinor=$($versionParts[1])"
    $buildArgs += "/p:VersionPatch=$($versionParts[2])"
    $buildArgs += '/p:VersionOverride=true'
}
if (-not $NoRestore) { $buildArgs += '/restore' }
Write-Host "Building Marknexia $Configuration/$Platform with $msbuild"
& $msbuild @buildArgs
if ($LASTEXITCODE -ne 0) { throw "Marknexia build failed ($LASTEXITCODE)." }

[xml]$projectXml = Get-Content -LiteralPath $project -Raw
$framework = $projectXml.Project.PropertyGroup.TargetFramework | Where-Object { $_ } | Select-Object -First 1
$outputDirectory = Join-Path $repoRoot "src/Marknexia.App/bin/$Platform/$Configuration/$framework"
& (Join-Path $PSScriptRoot 'Test-AppOutput.ps1') -OutputDirectory $outputDirectory
