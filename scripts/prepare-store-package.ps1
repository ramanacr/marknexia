param(
    [string]$PackageName = "RRCLabs.Marknexia",
    [string]$PublisherId = "CN=4BC6AFD6-49C8-46B2-A096-9AF3F2B78CB8",
    [string]$PublisherDisplayName = "RRC Labs",
    [string]$Version = "1.0.9.0"
)

$publishDir = Join-Path $PSScriptRoot "..\src\Marknexia.App\bin\x64\Release\net10.0-windows10.0.19041.0"
if (-not (Test-Path (Join-Path $publishDir "Marknexia.App.exe"))) {
    $publishDir = Join-Path $PSScriptRoot "..\src\Marknexia.App\bin\Release\net10.0-windows10.0.19041.0"
    if (Test-Path (Join-Path $publishDir "win-x64\Marknexia.App.exe")) {
        $publishDir = Join-Path $publishDir "win-x64"
    }
}
$manifestPath = Join-Path $PSScriptRoot "..\src\Marknexia.App\Package.appxmanifest"
$outputDir = Join-Path $PSScriptRoot "..\store-submission"
$msixStaging = Join-Path $outputDir "msix-staging"
$outputMsix = Join-Path $outputDir "Marknexia-Store-Ready.msix"

Write-Host "Preparing Microsoft Store package with Publisher: $PublisherId" -ForegroundColor Cyan

# Create staging directory
if (Test-Path $msixStaging) { Remove-Item $msixStaging -Recurse -Force }
New-Item -ItemType Directory -Force -Path $msixStaging | Out-Null
Copy-Item "$publishDir\*" -Destination "$msixStaging\" -Recurse
Get-ChildItem -Path $msixStaging -Recurse -Include "*.pdb" | Remove-Item -Force -ErrorAction SilentlyContinue
if (Test-Path (Join-Path $msixStaging "win-x64")) { Remove-Item (Join-Path $msixStaging "win-x64") -Recurse -Force }
if (Test-Path (Join-Path $msixStaging "publish")) { Remove-Item (Join-Path $msixStaging "publish") -Recurse -Force }

# Update manifest identity
[xml]$manifest = Get-Content $manifestPath
$identityNode = $manifest.SelectSingleNode("//*[local-name()='Identity']")
$publisherNode = $manifest.SelectSingleNode("//*[local-name()='PublisherDisplayName']")

if ($identityNode) {
    $identityNode.SetAttribute("Name", $PackageName)
    $identityNode.SetAttribute("Publisher", $PublisherId)
    if ($Version) {
        $identityNode.SetAttribute("Version", $Version)
    }
}
if ($publisherNode) {
    $publisherNode.InnerText = $PublisherDisplayName
}

$manifest.Save((Join-Path $msixStaging "AppxManifest.xml"))

# Run MakeAppx
$makeAppx = (Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin" -Recurse -Filter "makeappx.exe" | Where-Object { $_.FullName -match "x64" } | Select-Object -First 1).FullName
& "$makeAppx" pack /d "$msixStaging" /p "$outputMsix" /o

Write-Host "Store Package successfully generated: $outputMsix" -ForegroundColor Green
