param(
    [string]$PackageName = "Marknexia",
    [string]$PublisherId = "CN=Marknexia",
    [string]$PublisherDisplayName = "Marknexia"
)

$publishDir = Join-Path $PSScriptRoot "..\src\Marknexia.App\bin\Release\net10.0-windows10.0.19041.0"
$manifestPath = Join-Path $PSScriptRoot "..\src\Marknexia.App\Package.appxmanifest"
$outputDir = Join-Path $PSScriptRoot "..\store-submission"
$msixStaging = Join-Path $outputDir "msix-staging"
$outputMsix = Join-Path $outputDir "Marknexia-Store-Ready.msix"

Write-Host "Preparing Microsoft Store package with Publisher: $PublisherId" -ForegroundColor Cyan

# Create staging directory
if (Test-Path $msixStaging) { Remove-Item $msixStaging -Recurse -Force }
New-Item -ItemType Directory -Force -Path $msixStaging | Out-Null
Copy-Item "$publishDir\*" -Destination "$msixStaging\" -Recurse

# Update manifest identity
[xml]$manifest = Get-Content $manifestPath
$identityNode = $manifest.SelectSingleNode("//*[local-name()='Identity']")
$publisherNode = $manifest.SelectSingleNode("//*[local-name()='PublisherDisplayName']")

if ($identityNode) {
    $identityNode.SetAttribute("Name", $PackageName)
    $identityNode.SetAttribute("Publisher", $PublisherId)
}
if ($publisherNode) {
    $publisherNode.InnerText = $PublisherDisplayName
}

$manifest.Save((Join-Path $msixStaging "AppxManifest.xml"))

# Run MakeAppx
$makeAppx = (Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin" -Recurse -Filter "makeappx.exe" | Where-Object { $_.FullName -match "x64" } | Select-Object -First 1).FullName
& "$makeAppx" pack /d "$msixStaging" /p "$outputMsix" /o

Write-Host "Store Package successfully generated: $outputMsix" -ForegroundColor Green
