param(
    [string]$Version,
    [switch]$BumpPatch,
    [switch]$BumpMinor,
    [switch]$BumpMajor,
    [switch]$SyncManifestOnly
)

$propsPath = Join-Path $PSScriptRoot "..\Directory.Build.props"
$manifestPath = Join-Path $PSScriptRoot "..\src\Marknexia.App\Package.appxmanifest"

[xml]$propsXml = Get-Content $propsPath
$nodeMajor = $propsXml.SelectSingleNode("//VersionMajor")
$nodeMinor = $propsXml.SelectSingleNode("//VersionMinor")
$nodePatch = $propsXml.SelectSingleNode("//VersionPatch")

$major = if ($nodeMajor) { [int]$nodeMajor.InnerText } else { 1 }
$minor = if ($nodeMinor) { [int]$nodeMinor.InnerText } else { 0 }
$patch = if ($nodePatch) { [int]$nodePatch.InnerText } else { 0 }

if ($BumpMajor) {
    $major++
    $minor = 0
    $patch = 0
} elseif ($BumpMinor) {
    $minor++
    $patch = 0
} elseif ($BumpPatch) {
    $patch++
} elseif ($Version) {
    $parts = $Version.TrimStart('v').Split('.')
    $major = [int]$parts[0]
    $minor = [int]$parts[1]
    $patch = if ($parts.Length -gt 2) { [int]$parts[2] } else { 0 }
}

$commitCount = (git rev-list --count HEAD).Trim()
if (-not $commitCount) { $commitCount = "0" }

if ($nodeMajor) { $nodeMajor.InnerText = "$major" }
if ($nodeMinor) { $nodeMinor.InnerText = "$minor" }
if ($nodePatch) { $nodePatch.InnerText = "$patch" }

if (-not $SyncManifestOnly) {
    $propsXml.Save($propsPath)
}

# Sync Package.appxmanifest
if (Test-Path $manifestPath) {
    $manifestContent = Get-Content $manifestPath -Raw
    $appxVersion = "$major.$minor.$commitCount.0"
    $manifestContent = [System.Text.RegularExpressions.Regex]::Replace(
        $manifestContent,
        '(<Identity[^>]*Version=")[^"]*(")',
        "`${1}$appxVersion`${2}"
    )
    Set-Content -Path $manifestPath -Value $manifestContent -NoNewline
    Write-Host "Updated Package.appxmanifest Identity to Version=$appxVersion"
}

$currentVersion = "$major.$minor.$patch"
Write-Host "Current SemVer: $currentVersion"
Write-Host "Current Package/Build Version: $major.$minor.$commitCount"
return $currentVersion
