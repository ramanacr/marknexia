#requires -Version 7.0
<#
.SYNOPSIS
Shared logic of the local Authenticode signing flow (docs/release-signing.md).

.DESCRIPTION
Used by scripts/Invoke-ReleaseSigning.ps1 on the owner's PC (stages Binaries, Packages and
Cleanup) and by scripts/Test-SignedHandoff.ps1 on GitHub Actions (stage 3 verification).

Every external effect goes through the Tools object (New-SigningTools): signtool, gh,
Get-AuthenticodeSignature, the PE content digest, archive and MSIX manifest reads, sleeping
and logging. Tests replace these script blocks, so the flow is tested without a certificate,
without a network and without executables on disk.

This module never reads, stores or prints a password, a one-time code or a token. The
certificate is selected by its public SHA-1 thumbprint; SimplySign Desktop holds the key.
#>
Set-StrictMode -Version 3.0

$script:CodeSigningEku = '1.3.6.1.5.5.7.3.3'
$script:HandoffTitlePrefix = 'INTERNAL signing handoff (draft, never publish)'
$script:ManifestSchema = 'marknexia-signing-handoff-v1'

# --- Configuration and product descriptions ------------------------------------------------

function Get-ReleaseSigningConfig {
    [CmdletBinding()]
    param([string]$Path = (Join-Path $PSScriptRoot '..\..\packaging\signing\release-signing.json'))
    $config = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    if ($config.schemaVersion -cne 'marknexia-release-signing-v1') { throw "Unknown signing configuration schema in $Path." }
    if ($config.thumbprint -cnotmatch '^[0-9A-F]{40}$') { throw 'The signing thumbprint must be 40 upper-case hex characters (SHA-1).' }
    if ($config.certificateSha256 -cnotmatch '^[0-9a-f]{64}$') { throw 'certificateSha256 must be 64 lower-case hex characters.' }
    if ([string]::IsNullOrWhiteSpace($config.subject) -or $config.subject -notmatch '^CN=') { throw 'The certificate subject must be a distinguished name.' }
    if ($config.timestampUrl -notmatch '^https?://[^\s]+$') { throw 'timestampUrl must be an http(s) URL.' }
    $config
}

function Get-ProductSpec {
    [CmdletBinding()]
    param([Parameter(Mandatory)][ValidateSet('Rust', 'DotNet')][string]$Product)
    if ($Product -eq 'Rust') {
        return @{
            Product           = 'rust'
            DisplayName       = 'Marknexia Rust edition (preview)'
            SourceWorkflow    = 'Rust Feasibility'
            SourceRefPattern  = '^main$'
            Architectures     = @('x64', 'arm64')
            UnsignedArtifact  = @{ x64 = 'marknexia-rust-UNSIGNED-binaries-x86_64-pc-windows-msvc'; arm64 = 'marknexia-rust-UNSIGNED-binaries-aarch64-pc-windows-msvc' }
            PeExtensions      = @('.exe', '.dll')
            # Only the shell is ours. The WebView2 loader is not in this artifact: CI stages the
            # Microsoft-signed, hash-pinned loader itself (packaging/webview/loader.lock.json).
            Policy            = 'Explicit'
            OwnerFiles        = @('marknexia-win32.exe')
            MainExecutable    = 'marknexia-win32.exe'
            PackagingWorkflow = 'Signed packages'
            PackagingArtifact = @{ x64 = 'marknexia-rust-SIGNED-INPUT-packages-x86_64-pc-windows-msvc'; arm64 = 'marknexia-rust-SIGNED-INPUT-packages-aarch64-pc-windows-msvc' }
            ReleaseFiles      = @(
                @{ Pattern = 'portable\*.zip'; Sign = $null; ContainsMain = $true },
                @{ Pattern = 'portable\*.spdx.json'; Sign = $null; ContainsMain = $false },
                @{ Pattern = 'setup\*.exe'; Sign = 'Pe'; ContainsMain = $false },
                @{ Pattern = 'setup\*.spdx.json'; Sign = $null; ContainsMain = $false },
                @{ Pattern = 'msix\*.msix'; Sign = 'Msix'; ContainsMain = $true },
                @{ Pattern = 'msix\*.spdx.json'; Sign = $null; ContainsMain = $false }
            )
            Checksums         = 'Sums'
            TagPattern        = '^rust-preview-[0-9A-Za-z.\-]+$'
            Prerelease        = $true
        }
    }
    @{
        Product           = 'dotnet'
        DisplayName       = 'Marknexia (.NET edition)'
        SourceWorkflow    = 'Package & Release'
        SourceRefPattern  = '^v\d+\.\d+\.\d+$'
        Architectures     = @('x64', 'arm64')
        UnsignedArtifact  = @{ x64 = 'marknexia-dotnet-UNSIGNED-app-x64'; arm64 = 'marknexia-dotnet-UNSIGNED-app-arm64' }
        PeExtensions      = @('.exe', '.dll')
        # Every unsigned PE is signed with the release certificate; PEs that already carry a
        # valid signature (Windows App SDK, WebView2, other vendors) are kept unchanged.
        Policy            = 'AllUnsigned'
        OwnerFiles        = @()
        MainExecutable    = 'Marknexia.App.exe'
        PackagingWorkflow = 'Signed packages'
        PackagingArtifact = @{ x64 = 'marknexia-dotnet-SIGNED-INPUT-packages-x64'; arm64 = 'marknexia-dotnet-SIGNED-INPUT-packages-arm64' }
        ReleaseFiles      = @(
            @{ Pattern = 'Marknexia-v*-win-*.zip'; Sign = $null; ContainsMain = $true },
            @{ Pattern = 'Marknexia-v*-win-*.msix'; Sign = 'Msix'; ContainsMain = $true },
            @{ Pattern = 'Marknexia-v*-win-*-setup.exe'; Sign = 'Pe'; ContainsMain = $false }
        )
        Checksums         = 'Sidecar'
        TagPattern        = '^v\d+\.\d+\.\d+$'
        Prerelease        = $false
    }
}

function Get-HandoffTag {
    param([Parameter(Mandatory)][string]$Product, [Parameter(Mandatory)][string]$SourceRunId)
    if ($SourceRunId -notmatch '^\d+$') { throw 'The source run id must be numeric.' }
    "signing-handoff-$($Product.ToLowerInvariant())-$SourceRunId"
}

# --- Tools ---------------------------------------------------------------------------------

function Find-SignTool {
    [CmdletBinding()]
    param([string]$KitsRoot = (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'))
    if (-not (Test-Path -LiteralPath $KitsRoot)) { throw "The Windows SDK was not found ($KitsRoot). Install the Windows SDK signing tools." }
    $hostArch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
    $arches = if ($hostArch -eq 'arm64') { @('arm64', 'x64') } else { @('x64') }
    $versions = @(Get-ChildItem -LiteralPath $KitsRoot -Directory | Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' } | Sort-Object { [version]$_.Name } -Descending)
    foreach ($version in $versions) {
        foreach ($arch in $arches) {
            $candidate = Join-Path $version.FullName "$arch\signtool.exe"
            if (Test-Path -LiteralPath $candidate) { return $candidate }
        }
    }
    throw "signtool was not found under $KitsRoot. Install the Windows SDK signing tools."
}

function New-SigningTools {
    [CmdletBinding()]
    param([string]$SignToolPath)
    $signTool = $SignToolPath
    $runSignTool = if ($signTool) {
        {
            param([string[]]$Arguments)
            $PSNativeCommandUseErrorActionPreference = $false
            $output = & $signTool @Arguments 2>&1 | ForEach-Object { $_.ToString() }
            [pscustomobject]@{ ExitCode = $LASTEXITCODE; Output = (@($output) -join "`n"); Error = '' }
        }.GetNewClosure()
    } else { $null }
    [pscustomobject]@{
        SignTool            = $runSignTool
        Gh                  = {
            param([string[]]$Arguments)
            $PSNativeCommandUseErrorActionPreference = $false
            $errors = [Collections.Generic.List[string]]::new()
            $output = & gh @Arguments 2>&1 | ForEach-Object {
                if ($_ -is [Management.Automation.ErrorRecord]) { $errors.Add($_.ToString()) } else { $_.ToString() }
            }
            [pscustomobject]@{ ExitCode = $LASTEXITCODE; Output = (@($output) -join "`n"); Error = ($errors -join "`n") }
        }
        GetSignature        = {
            param([string]$Path)
            $signature = Get-AuthenticodeSignature -LiteralPath $Path
            $signer = $signature.SignerCertificate
            [pscustomobject]@{
                Status        = [string]$signature.Status
                Thumbprint    = if ($signer) { $signer.Thumbprint } else { $null }
                Subject       = if ($signer) { $signer.Subject } else { $null }
                Timestamped   = $null -ne $signature.TimeStamperCertificate
                StatusMessage = [string]$signature.StatusMessage
            }
        }
        ContentDigest       = { param([string]$Path) Get-PeContentDigest -Path $Path }
        ArchiveEntrySha256  = { param([string]$ArchivePath, [string]$EntryName) Get-ZipEntrySha256 -ArchivePath $ArchivePath -EntryName $EntryName }
        MsixPublisher       = { param([string]$Path) Get-MsixPublisher -Path $Path }
        Sleep               = { param([int]$Seconds) Start-Sleep -Seconds $Seconds }
        Log                 = { param([string]$Message) Write-Host $Message }
    }
}

function Write-SigningLog {
    param([Parameter(Mandatory)]$Tools, [Parameter(Mandatory)][AllowEmptyString()][string]$Message)
    & $Tools.Log $Message
}

function Invoke-Gh {
    param([Parameter(Mandatory)]$Tools, [Parameter(Mandatory)][string[]]$Arguments, [switch]$AllowFailure)
    $result = & $Tools.Gh $Arguments
    if (-not $AllowFailure -and $result.ExitCode -ne 0) {
        throw "gh $($Arguments[0]) $($Arguments[1]) failed (exit $($result.ExitCode)): $($result.Error)"
    }
    $result
}

function Test-GhNotFound([Parameter(Mandatory)]$Result) {
    $Result.ExitCode -ne 0 -and ("$($Result.Error)`n$($Result.Output)" -match '(?i)not found|HTTP 404')
}

# --- Hashes and PE content -----------------------------------------------------------------

function Get-FileSha256([Parameter(Mandatory)][string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-PeContentDigest {
    <#
    SHA-256 of a PE file without the parts that Authenticode signing changes: the optional
    header CheckSum, the certificate-table directory entry and the certificate table itself.
    For a file without a certificate table, zero bytes pad the content to an 8-byte boundary,
    as signtool pads before it appends the table. So an unsigned file and the same file after
    signing have the same digest, and any other change gives a different one.
    #>
    [CmdletBinding(DefaultParameterSetName = 'Path')]
    param(
        [Parameter(Mandatory, ParameterSetName = 'Path')][string]$Path,
        [Parameter(Mandatory, ParameterSetName = 'Bytes')][byte[]]$Bytes
    )
    if ($PSCmdlet.ParameterSetName -eq 'Path') { $Bytes = [IO.File]::ReadAllBytes($Path) }
    $length = $Bytes.Length
    if ($length -lt 0x40 -or $Bytes[0] -ne 0x4D -or $Bytes[1] -ne 0x5A) { throw 'Not a PE file: the MZ header is missing.' }
    $pe = [BitConverter]::ToInt32($Bytes, 0x3C)
    if ($pe -lt 0x40 -or $pe + 24 -gt $length -or [BitConverter]::ToUInt32($Bytes, $pe) -ne 0x00004550) { throw 'Not a PE file: the PE signature is missing.' }
    $optionalSize = [int][BitConverter]::ToUInt16($Bytes, $pe + 20)
    $optional = $pe + 24
    if ($optional + 2 -gt $length) { throw 'Truncated PE optional header.' }
    $magic = [BitConverter]::ToUInt16($Bytes, $optional)
    switch ($magic) {
        0x10B { $countOffset = 92; $directoryOffset = 96 }
        0x20B { $countOffset = 108; $directoryOffset = 112 }
        default { throw ('Unknown PE optional header magic 0x{0:X}.' -f $magic) }
    }
    if ($optional + $directoryOffset -gt $length -or $optional + $directoryOffset -gt $optional + $optionalSize) { throw 'Truncated PE optional header.' }
    $checksum = $optional + 64
    $directoryCount = [BitConverter]::ToUInt32($Bytes, $optional + $countOffset)
    $certificateEntry = $optional + $directoryOffset + 4 * 8
    $hasEntry = $directoryCount -ge 5 -and ($certificateEntry + 8) -le ($optional + $optionalSize) -and ($certificateEntry + 8) -le $length

    $hash = [Security.Cryptography.IncrementalHash]::CreateHash([Security.Cryptography.HashAlgorithmName]::SHA256)
    try {
        $hash.AppendData($Bytes, 0, $checksum)
        $tableOffset = [int64]0; $tableSize = [int64]0
        if ($hasEntry) {
            $hash.AppendData($Bytes, $checksum + 4, $certificateEntry - ($checksum + 4))
            $tableOffset = [int64][BitConverter]::ToUInt32($Bytes, $certificateEntry)
            $tableSize = [int64][BitConverter]::ToUInt32($Bytes, $certificateEntry + 4)
            $after = $certificateEntry + 8
        } else {
            $after = $checksum + 4
        }
        if ($tableSize -gt 0) {
            if ($tableOffset -lt $after -or $tableOffset + $tableSize -gt $length) { throw 'The PE certificate table is outside the file.' }
            $hash.AppendData($Bytes, $after, [int]($tableOffset - $after))
            $tail = $tableOffset + $tableSize
            if ($tail -lt $length) { $hash.AppendData($Bytes, [int]$tail, [int]($length - $tail)) }
        } else {
            $hash.AppendData($Bytes, $after, $length - $after)
            $pad = (8 - ($length % 8)) % 8
            if ($pad -gt 0) { $hash.AppendData([byte[]]::new($pad)) }
        }
        ($hash.GetHashAndReset() | ForEach-Object { $_.ToString('x2') }) -join ''
    } finally { $hash.Dispose() }
}

function Get-ZipEntrySha256 {
    # Reads one entry in memory; nothing is extracted to disk.
    param([Parameter(Mandatory)][string]$ArchivePath, [Parameter(Mandatory)][string]$EntryName)
    $archive = [IO.Compression.ZipFile]::OpenRead([IO.Path]::GetFullPath($ArchivePath))
    try {
        $entries = @($archive.Entries | Where-Object { $_.FullName -ceq $EntryName })
        if ($entries.Count -ne 1) { throw "$ArchivePath must hold exactly one $EntryName at its root." }
        $stream = $entries[0].Open()
        try { ([Security.Cryptography.SHA256]::HashData($stream) | ForEach-Object { $_.ToString('x2') }) -join '' } finally { $stream.Dispose() }
    } finally { $archive.Dispose() }
}

function Get-MsixPublisher {
    param([Parameter(Mandatory)][string]$Path)
    $archive = [IO.Compression.ZipFile]::OpenRead([IO.Path]::GetFullPath($Path))
    try {
        $entry = $archive.GetEntry('AppxManifest.xml')
        if (-not $entry) { throw "$Path has no AppxManifest.xml." }
        $reader = [IO.StreamReader]::new($entry.Open())
        try { [xml]$manifest = $reader.ReadToEnd() } finally { $reader.Dispose() }
    } finally { $archive.Dispose() }
    $identity = $manifest.SelectSingleNode("/*[local-name()='Package']/*[local-name()='Identity']")
    if (-not $identity) { throw "$Path has no package identity." }
    $identity.GetAttribute('Publisher')
}

# --- Certificate and signatures ------------------------------------------------------------

function Assert-SigningCertificate {
    [CmdletBinding()]
    param([Parameter(Mandatory)]$Config, $Certificate, [datetime]$Now = [datetime]::Now)
    if (-not $Certificate) { $Certificate = Get-Item -LiteralPath "Cert:\CurrentUser\My\$($Config.thumbprint)" -ErrorAction SilentlyContinue }
    if (-not $Certificate) {
        throw "The release certificate $($Config.thumbprint) is not in CurrentUser\My. Start SimplySign Desktop and log in; it registers the certificate."
    }
    if ($Certificate.Subject -cne $Config.subject) { throw "Certificate subject '$($Certificate.Subject)' differs from packaging/signing/release-signing.json." }
    $sha256 = ([Security.Cryptography.SHA256]::HashData([byte[]]$Certificate.RawData) | ForEach-Object { $_.ToString('x2') }) -join ''
    if ($sha256 -cne $Config.certificateSha256) { throw 'The certificate in the store is not the configured release certificate (SHA-256 differs).' }
    if ($Certificate.NotBefore -gt $Now) { throw "The release certificate is not valid before $($Certificate.NotBefore)." }
    if ($Certificate.NotAfter -le $Now) { throw "The release certificate expired on $($Certificate.NotAfter). Renew it and update packaging/signing/release-signing.json." }
    if (-not $Certificate.HasPrivateKey) { throw 'Windows reports no private key for the release certificate. Log in to SimplySign Desktop.' }
    $usages = @($Certificate.EnhancedKeyUsageList | ForEach-Object { $_.ObjectId })
    if ($usages -notcontains $script:CodeSigningEku) { throw 'The release certificate has no code-signing usage.' }
    if (($Certificate.NotAfter - $Now).TotalDays -lt 30) { Write-Warning "The release certificate expires on $($Certificate.NotAfter)." }
}

function Get-SignatureState {
    param([Parameter(Mandatory)]$Tools, [Parameter(Mandatory)]$Config, [Parameter(Mandatory)][string]$Path)
    $signature = & $Tools.GetSignature $Path
    $state = switch ($signature.Status) {
        'NotSigned' { 'NotSigned' }
        'Valid' { if ("$($signature.Thumbprint)" -ieq $Config.thumbprint) { 'Owner' } else { 'ThirdParty' } }
        default { 'Invalid' }
    }
    [pscustomobject]@{
        State = $state; Status = $signature.Status; Thumbprint = $signature.Thumbprint; Subject = $signature.Subject
        Timestamped = [bool]$signature.Timestamped; StatusMessage = $signature.StatusMessage
    }
}

function Test-ReleaseSigned([Parameter(Mandatory)]$Tools, [Parameter(Mandatory)]$Config, [Parameter(Mandatory)][string]$Path) {
    $state = Get-SignatureState -Tools $Tools -Config $Config -Path $Path
    $state.State -eq 'Owner' -and $state.Timestamped
}

function Get-SignToolSignArguments {
    param([Parameter(Mandatory)]$Config, [Parameter(Mandatory)][ValidateSet('Pe', 'Msix', 'Probe')][string]$Kind, [Parameter(Mandatory)][string[]]$Paths)
    $arguments = [Collections.Generic.List[string]]::new()
    $arguments.AddRange([string[]]@('sign', '/fd', 'SHA256'))
    # The probe is not timestamped, so it does not depend on the timestamp server.
    if ($Kind -ne 'Probe') { $arguments.AddRange([string[]]@('/tr', $Config.timestampUrl, '/td', 'SHA256')) }
    $arguments.AddRange([string[]]@('/sha1', $Config.thumbprint))
    if ($Kind -eq 'Pe') { $arguments.AddRange([string[]]@('/d', $Config.description, '/du', $Config.descriptionUrl)) }
    $arguments.AddRange($Paths)
    , $arguments.ToArray()
}

function Invoke-SignFiles {
    <#
    Signs files in batches. A failed batch is retried after a pause, but only with the files
    that are not yet signed and timestamped by the release certificate. Run the stage again
    after a failure: finished files are skipped.
    #>
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]$Tools,
        [Parameter(Mandatory)]$Config,
        [Parameter(Mandatory)][string[]]$Paths,
        [ValidateSet('Pe', 'Msix')][string]$Kind = 'Pe',
        [int]$BatchSize = 16,
        [int]$MaxAttempts = 3,
        [int[]]$RetryDelaySeconds = @(20, 60)
    )
    if (-not $Tools.SignTool) { throw 'signtool is not available.' }
    for ($start = 0; $start -lt $Paths.Count; $start += $BatchSize) {
        $batch = @($Paths[$start..([Math]::Min($start + $BatchSize, $Paths.Count) - 1)])
        $attempt = 0
        while ($batch.Count -gt 0) {
            $attempt++
            Write-SigningLog $Tools "Signing $($batch.Count) file(s), attempt $attempt of $MaxAttempts`: $(($batch | ForEach-Object { Split-Path $_ -Leaf }) -join ', ')"
            $result = & $Tools.SignTool (Get-SignToolSignArguments -Config $Config -Kind $Kind -Paths $batch)
            if ($result.ExitCode -eq 0) { break }
            Write-SigningLog $Tools "signtool failed (exit $($result.ExitCode)):`n$($result.Output)"
            $batch = @($batch | Where-Object { -not (Test-ReleaseSigned -Tools $Tools -Config $Config -Path $_) })
            if ($batch.Count -eq 0) { break }
            if ($attempt -ge $MaxAttempts) {
                throw "signtool failed $MaxAttempts times. Check that SimplySign Desktop is logged in and that $($Config.timestampUrl) answers, then run the same command again. Files that are already signed are skipped."
            }
            & $Tools.Sleep $RetryDelaySeconds[[Math]::Min($attempt - 1, $RetryDelaySeconds.Count - 1)]
        }
    }
}

function Assert-ReleaseSignature {
    param([Parameter(Mandatory)]$Tools, [Parameter(Mandatory)]$Config, [Parameter(Mandatory)][string]$Path)
    $state = Get-SignatureState -Tools $Tools -Config $Config -Path $Path
    if ($state.State -ne 'Owner') { throw "$Path is not signed by the release certificate ($($state.Status), signer $($state.Thumbprint)): $($state.StatusMessage)" }
    if (-not $state.Timestamped) { throw "$Path is signed but has no timestamp. Run the stage again to re-sign it." }
    if ($Tools.SignTool) {
        $verify = & $Tools.SignTool @('verify', '/pa', '/v', $Path)
        if ($verify.ExitCode -ne 0) { throw "signtool verify /pa failed for $Path`:`n$($verify.Output)" }
    }
}

function Test-SigningSession {
    <#
    Signs a small, throw-away PowerShell script (not timestamped) to prove that SimplySign
    Desktop is logged in before any release file is touched. The probe file is deleted.
    #>
    param([Parameter(Mandatory)]$Tools, [Parameter(Mandatory)]$Config, [Parameter(Mandatory)][string]$WorkDirectory)
    [IO.Directory]::CreateDirectory($WorkDirectory) | Out-Null
    $probe = Join-Path $WorkDirectory "signing-session-probe-$([Guid]::NewGuid().ToString('N')).ps1"
    [IO.File]::WriteAllText($probe, "# Marknexia signing session probe. Safe to delete.`n", [Text.UTF8Encoding]::new($false))
    try {
        $result = & $Tools.SignTool (Get-SignToolSignArguments -Config $Config -Kind Probe -Paths @($probe))
        $state = Get-SignatureState -Tools $Tools -Config $Config -Path $probe
        if ($result.ExitCode -ne 0 -or $state.State -ne 'Owner') {
            throw "The release certificate cannot sign. Open SimplySign Desktop, log in with your one-time code, then run the command again.`n$($result.Output)"
        }
    } finally { Remove-Item -LiteralPath $probe -Force -ErrorAction SilentlyContinue }
}

# --- Planning ------------------------------------------------------------------------------

function Get-PeFiles {
    param([Parameter(Mandatory)][string]$Root, [Parameter(Mandatory)][string[]]$Extensions)
    $rootFull = [IO.Path]::GetFullPath($Root)
    $files = @(Get-ChildItem -LiteralPath $rootFull -File -Recurse | Where-Object { $Extensions -contains $_.Extension.ToLowerInvariant() } |
        ForEach-Object { [IO.Path]::GetRelativePath($rootFull, $_.FullName).Replace('\', '/') })
    $sorted = [string[]]$files
    [Array]::Sort($sorted, [StringComparer]::Ordinal)
    , $sorted
}

function Get-SigningPlan {
    <#
    Decides, for every PE file under Root, whether it is signed (Sign), already signed by the
    release certificate with a timestamp (AlreadySigned), or kept with its existing valid
    third-party signature (Keep). An invalid signature always stops the run.
    #>
    param([Parameter(Mandatory)]$Tools, [Parameter(Mandatory)]$Config, [Parameter(Mandatory)][hashtable]$Spec, [Parameter(Mandatory)][string]$Root)
    $files = Get-PeFiles -Root $Root -Extensions $Spec.PeExtensions
    if ($Spec.Policy -eq 'Explicit') {
        $expected = [string[]]@($Spec.OwnerFiles)
        [Array]::Sort($expected, [StringComparer]::Ordinal)
        if (($files -join '|') -cne ($expected -join '|')) {
            throw "Expected exactly these executables in $Root`: $($expected -join ', '); found: $($files -join ', ')"
        }
    }
    foreach ($relative in $files) {
        $path = Join-Path $Root $relative
        $state = Get-SignatureState -Tools $Tools -Config $Config -Path $path
        $action = switch ($state.State) {
            'NotSigned' { 'Sign' }
            'Owner' { if ($state.Timestamped) { 'AlreadySigned' } else { 'Sign' } }
            'ThirdParty' {
                if ($Spec.Policy -eq 'Explicit') { throw "$relative carries a third-party signature ($($state.Subject)); expected an unsigned build." }
                'Keep'
            }
            default { throw "$relative has an invalid signature ($($state.Status)): $($state.StatusMessage)" }
        }
        [pscustomobject]@{ RelativePath = $relative; FullPath = $path; Action = $action; Signature = $state }
    }
}

# --- GitHub --------------------------------------------------------------------------------

function Get-WorkflowRun {
    param([Parameter(Mandatory)]$Tools, [Parameter(Mandatory)][string]$Repository, [Parameter(Mandatory)][string]$RunId)
    if ($RunId -notmatch '^\d+$') { throw 'The run id must be numeric.' }
    $result = Invoke-Gh $Tools @('run', 'view', $RunId, '--repo', $Repository, '--json', 'conclusion,status,headSha,headBranch,workflowName,event,databaseId')
    $result.Output | ConvertFrom-Json
}

function Assert-WorkflowRun {
    param([Parameter(Mandatory)]$Run, [Parameter(Mandatory)][string]$WorkflowName, [Parameter(Mandatory)][string]$Event, [string]$RefPattern)
    if ($Run.workflowName -cne $WorkflowName) { throw "Run $($Run.databaseId) is from '$($Run.workflowName)', expected '$WorkflowName'." }
    if ($Run.status -cne 'completed' -or $Run.conclusion -cne 'success') { throw "Run $($Run.databaseId) is not green ($($Run.status)/$($Run.conclusion))." }
    if ($Run.event -cne $Event) { throw "Run $($Run.databaseId) was triggered by '$($Run.event)', expected '$Event'." }
    if ($RefPattern -and $Run.headBranch -cnotmatch $RefPattern) { throw "Run $($Run.databaseId) built '$($Run.headBranch)', which does not match $RefPattern." }
    if ($Run.headSha -cnotmatch '^[0-9a-f]{40}$') { throw "Run $($Run.databaseId) has no commit SHA." }
}

function Save-RunArtifact {
    param([Parameter(Mandatory)]$Tools, [Parameter(Mandatory)][string]$Repository, [Parameter(Mandatory)][string]$RunId,
        [Parameter(Mandatory)][string]$Name, [Parameter(Mandatory)][string]$Destination)
    if (Test-Path -LiteralPath $Destination) { Write-SigningLog $Tools "Reusing the downloaded artifact $Name in $Destination"; return }
    $partial = "$Destination.partial"
    if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Recurse -Force }
    Write-SigningLog $Tools "Downloading artifact $Name from run $RunId"
    Invoke-Gh $Tools @('run', 'download', $RunId, '--repo', $Repository, '--name', $Name, '--dir', $partial) | Out-Null
    if (-not (Test-Path -LiteralPath $partial)) { throw "gh run download wrote nothing for $Name." }
    Move-Item -LiteralPath $partial -Destination $Destination
}

function Get-ReleaseInfo {
    # Returns $null when the release (draft or published) does not exist.
    param([Parameter(Mandatory)]$Tools, [Parameter(Mandatory)][string]$Repository, [Parameter(Mandatory)][string]$Tag)
    $result = Invoke-Gh $Tools @('release', 'view', $Tag, '--repo', $Repository, '--json', 'isDraft,isPrerelease,name,tagName,assets') -AllowFailure
    if ($result.ExitCode -eq 0) { return $result.Output | ConvertFrom-Json }
    if (Test-GhNotFound $result) { return $null }
    throw "gh release view $Tag failed (exit $($result.ExitCode)): $($result.Error)"
}

function Get-RemoteTagCommit {
    # Returns the commit a tag points to, or $null when the tag does not exist.
    param([Parameter(Mandatory)]$Tools, [Parameter(Mandatory)][string]$Repository, [Parameter(Mandatory)][string]$Tag)
    $result = Invoke-Gh $Tools @('api', "repos/$Repository/git/ref/tags/$Tag", '--jq', '.object.type + " " + .object.sha') -AllowFailure
    if ($result.ExitCode -ne 0) {
        if (Test-GhNotFound $result) { return $null }
        throw "Could not read tag $Tag (exit $($result.ExitCode)): $($result.Error)"
    }
    $type, $sha = "$($result.Output)".Trim() -split ' ', 2
    if ($type -eq 'tag') {
        $sha = "$((Invoke-Gh $Tools @('api', "repos/$Repository/git/tags/$sha", '--jq', '.object.sha')).Output)".Trim()
    }
    $sha
}

# --- Files ---------------------------------------------------------------------------------

function Write-JsonFile([Parameter(Mandatory)]$Value, [Parameter(Mandatory)][string]$Path) {
    $Value | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $Path -Encoding utf8NoBOM
}

function ConvertTo-HandoffAssetName([string]$Architecture, [int]$Index, [string]$RelativePath) {
    $leaf = ($RelativePath -split '/')[-1] -replace '[^A-Za-z0-9._-]', '_'
    '{0}-{1:d3}-{2}' -f $Architecture, $Index, $leaf
}

function Write-Sha256Sums {
    param([Parameter(Mandatory)][string]$Folder, [Parameter(Mandatory)][string]$FileName)
    $names = [string[]]@(Get-ChildItem -LiteralPath $Folder -File | Where-Object Name -cne $FileName | ForEach-Object Name)
    [Array]::Sort($names, [StringComparer]::Ordinal)
    $lines = foreach ($name in $names) { "$(Get-FileSha256 (Join-Path $Folder $name))  $name" }
    [IO.File]::WriteAllText((Join-Path $Folder $FileName), ($lines -join "`n") + "`n", [Text.UTF8Encoding]::new($false))
}

function Write-Sha256Sidecar([Parameter(Mandatory)][string]$Path) {
    $name = Split-Path $Path -Leaf
    [IO.File]::WriteAllText("$Path.sha256", "$(Get-FileSha256 $Path)  $name`n", [Text.UTF8Encoding]::new($false))
}

# --- Stage 2: binaries ---------------------------------------------------------------------

function Invoke-BinariesStage {
    [CmdletBinding()]
    param([Parameter(Mandatory)][hashtable]$Context, [Parameter(Mandatory)][string]$SourceRunId)
    $tools = $Context.Tools; $config = $Context.Config; $spec = $Context.Spec; $repo = $Context.Repository; $root = $Context.OutputRoot
    $run = Get-WorkflowRun $tools $repo $SourceRunId
    Assert-WorkflowRun -Run $run -WorkflowName $spec.SourceWorkflow -Event 'push' -RefPattern $spec.SourceRefPattern
    Write-SigningLog $tools "Source run $SourceRunId`: $($run.workflowName), $($run.headBranch) at $($run.headSha)"
    $handoffTag = Get-HandoffTag $spec.Product $SourceRunId
    [IO.Directory]::CreateDirectory($root) | Out-Null

    $records = [Collections.Generic.List[object]]::new()
    $kept = [Collections.Generic.List[object]]::new()
    $toSign = [Collections.Generic.List[string]]::new()
    foreach ($arch in $spec.Architectures) {
        $unsigned = Join-Path $root "unsigned\$arch"
        Save-RunArtifact $tools $repo $SourceRunId $spec.UnsignedArtifact[$arch] $unsigned
        $signedDir = Join-Path $root "signed\$arch"
        if (-not (Test-Path -LiteralPath $signedDir)) {
            [IO.Directory]::CreateDirectory((Split-Path $signedDir -Parent)) | Out-Null
            Copy-Item -LiteralPath $unsigned -Destination $signedDir -Recurse
        }
        foreach ($item in Get-SigningPlan -Tools $tools -Config $config -Spec $spec -Root $signedDir) {
            $unsignedFile = Join-Path $unsigned $item.RelativePath
            if (-not (Test-Path -LiteralPath $unsignedFile)) { throw "$($item.RelativePath) is not in the downloaded artifact." }
            if ($item.Action -eq 'Keep') {
                if ((Get-FileSha256 $item.FullPath) -cne (Get-FileSha256 $unsignedFile)) { throw "$arch/$($item.RelativePath) changed after download. Delete $root and run again." }
                $kept.Add([ordered]@{ architecture = $arch; path = $item.RelativePath; sha256 = (Get-FileSha256 $item.FullPath); signerSubject = $item.Signature.Subject; signerThumbprint = $item.Signature.Thumbprint })
                continue
            }
            if ((& $tools.ContentDigest $item.FullPath) -cne (& $tools.ContentDigest $unsignedFile)) {
                throw "$arch/$($item.RelativePath) differs from the CI build beyond its signature. Delete $root and run again."
            }
            $records.Add([pscustomobject]@{ Architecture = $arch; RelativePath = $item.RelativePath; SignedPath = $item.FullPath; UnsignedPath = $unsignedFile })
            if ($item.Action -eq 'Sign') { $toSign.Add($item.FullPath) }
        }
    }
    if ($records.Count -eq 0) { throw 'Nothing to sign was found in the artifacts.' }
    Write-SigningLog $tools "$($records.Count) file(s) to carry the release signature ($($toSign.Count) to sign now); $($kept.Count) third-party signed file(s) kept."
    if ($toSign.Count -gt 0) { Invoke-SignFiles -Tools $tools -Config $config -Paths $toSign.ToArray() -Kind Pe }

    $handoff = Join-Path $root 'handoff'
    if (Test-Path -LiteralPath $handoff) { Remove-Item -LiteralPath $handoff -Recurse -Force }
    [IO.Directory]::CreateDirectory($handoff) | Out-Null
    $files = [Collections.Generic.List[object]]::new()
    $index = 0
    foreach ($record in $records) {
        Assert-ReleaseSignature -Tools $tools -Config $config -Path $record.SignedPath
        $content = & $tools.ContentDigest $record.SignedPath
        if ($content -cne (& $tools.ContentDigest $record.UnsignedPath)) { throw "$($record.Architecture)/$($record.RelativePath) changed beyond its signature." }
        $index++
        $asset = ConvertTo-HandoffAssetName $record.Architecture $index $record.RelativePath
        Copy-Item -LiteralPath $record.SignedPath -Destination (Join-Path $handoff $asset)
        $files.Add([ordered]@{
                architecture = $record.Architecture; path = $record.RelativePath; handoffAsset = $asset
                unsignedSha256 = (Get-FileSha256 $record.UnsignedPath); signedSha256 = (Get-FileSha256 $record.SignedPath); contentSha256 = $content
            })
    }
    $manifest = [ordered]@{
        schemaVersion = $script:ManifestSchema
        product = $spec.Product
        sourceRunId = $SourceRunId
        sourceWorkflow = $run.workflowName
        sourceRef = $run.headBranch
        sourceCommit = $run.headSha
        handoffTag = $handoffTag
        signer = [ordered]@{ thumbprint = $config.thumbprint; certificateSha256 = $config.certificateSha256; subject = $config.subject }
        timestampUrl = $config.timestampUrl
        createdUtc = [DateTimeOffset]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')
        files = $files.ToArray()
        kept = $kept.ToArray()
    }
    Write-JsonFile $manifest (Join-Path $handoff 'signing-manifest.json')
    Copy-Item -LiteralPath (Join-Path $handoff 'signing-manifest.json') -Destination (Join-Path $root 'signing-manifest.json') -Force
    $assets = @(Get-ChildItem -LiteralPath $handoff -File | ForEach-Object FullName)

    $dispatch = "gh workflow run signed-packages.yml --repo $repo --ref main -f product=$($spec.Product) -f source_run_id=$SourceRunId -f handoff_tag=$handoffTag"
    if (-not $Context.Publish) {
        Write-SigningLog $tools "DRY RUN: signed and verified $($files.Count) file(s) in $root\signed. Nothing was uploaded."
        Write-SigningLog $tools "Run again with -Publish to upload them to the private draft release $handoffTag."
        return [pscustomobject]@{ Manifest = $manifest; HandoffTag = $handoffTag; Uploaded = $false; NextCommand = $dispatch }
    }
    $existing = Get-ReleaseInfo $tools $repo $handoffTag
    if ($existing -and -not $existing.isDraft) { throw "A published release $handoffTag exists. It must stay a draft; remove it by hand after checking it." }
    if ($existing) {
        Invoke-Gh $tools (@('release', 'upload', $handoffTag, '--repo', $repo, '--clobber') + $assets) | Out-Null
    } else {
        $notes = "Signed binaries from run $SourceRunId ($($run.headSha)) for the Signed packages workflow. Not a release: keep it a draft. scripts/Invoke-ReleaseSigning.ps1 -Stage Cleanup removes it."
        Invoke-Gh $tools (@('release', 'create', $handoffTag, '--repo', $repo, '--draft', '--target', $run.headSha,
                '--title', "$($script:HandoffTitlePrefix): $($spec.Product) run $SourceRunId", '--notes', $notes) + $assets) | Out-Null
    }
    $check = Get-ReleaseInfo $tools $repo $handoffTag
    if (-not $check -or -not $check.isDraft) { throw "The handoff release $handoffTag is not a draft after upload." }
    Write-SigningLog $tools "Uploaded $($assets.Count) file(s) to the draft release $handoffTag."
    Write-SigningLog $tools "Next (stage 3): $dispatch"
    [pscustomobject]@{ Manifest = $manifest; HandoffTag = $handoffTag; Uploaded = $true; NextCommand = $dispatch }
}

# --- Stage 4: packages ---------------------------------------------------------------------

function Get-ReleaseNotes {
    param([hashtable]$Spec, $Config, $Manifest, [string]$PackagingRunId, [string]$Tag)
    if ($Spec.Product -eq 'rust') {
        return @"
Preview of the Marknexia Rust edition (raw Win32 + WebView2, no .NET runtime). It installs and runs side by side with the .NET edition, with its own identity.

- Built from commit $($Manifest.sourceCommit) by Rust Feasibility run $($Manifest.sourceRunId); packaged by Signed packages run $PackagingRunId. Checksums in SHA256SUMS.txt.
- The shell, the setup executable and the MSIX are code-signed by "$($Config.publisherDisplayName)" (certificate SHA-1 $($Config.thumbprint)). The WebView2 loader keeps its Microsoft signature.
- The MSIX publisher is now the certificate subject. A copy of an earlier unsigned preview MSIX must be removed before this one is installed.
- The portable ZIP needs the WebView2 Runtime, which Windows 11 includes.
- This is a pre-release: the .NET edition's updater does not offer it.
"@
    }
    @"
Marknexia $Tag for Windows x64 and ARM64: setup executable, portable ZIP and MSIX.

- Built from commit $($Manifest.sourceCommit) by release run $($Manifest.sourceRunId); packaged by Signed packages run $PackagingRunId.
- The setup executable, the application files and the MSIX are code-signed by "$($Config.publisherDisplayName)" (certificate SHA-1 $($Config.thumbprint)).
- The GitHub MSIX now uses the certificate subject as its publisher, so its package family name changed. If you installed an earlier GitHub MSIX, uninstall it and install this one. Microsoft Store installs are not affected.
"@
}

function Invoke-PackagesStage {
    [CmdletBinding()]
    param([Parameter(Mandatory)][hashtable]$Context, [Parameter(Mandatory)][string]$PackagingRunId, [string]$Tag)
    $tools = $Context.Tools; $config = $Context.Config; $spec = $Context.Spec; $repo = $Context.Repository; $root = $Context.OutputRoot
    $run = Get-WorkflowRun $tools $repo $PackagingRunId
    Assert-WorkflowRun -Run $run -WorkflowName $spec.PackagingWorkflow -Event 'workflow_dispatch'
    [IO.Directory]::CreateDirectory($root) | Out-Null

    $provenance = @{}
    foreach ($arch in $spec.Architectures) {
        $folder = Join-Path $root "packages\$arch"
        Save-RunArtifact $tools $repo $PackagingRunId $spec.PackagingArtifact[$arch] $folder
        $item = Get-Content -LiteralPath (Join-Path $folder 'packaging-provenance.json') -Raw | ConvertFrom-Json
        if ($item.schemaVersion -cne 'marknexia-signed-packages-provenance-v1' -or $item.product -cne $spec.Product -or $item.architecture -cne $arch -or "$($item.packagingRunId)" -cne $PackagingRunId) {
            throw "The packaging provenance for $arch does not match product $($spec.Product), run $PackagingRunId."
        }
        $provenance[$arch] = $item
    }
    $first = $provenance[$spec.Architectures[0]]
    foreach ($arch in $spec.Architectures) {
        foreach ($field in 'sourceRunId', 'sourceCommit', 'handoffTag', 'manifestSha256') {
            if ("$($provenance[$arch].$field)" -cne "$($first.$field)") { throw "The packages disagree on $field." }
        }
    }

    $handoffInfo = Get-ReleaseInfo $tools $repo $first.handoffTag
    if (-not $handoffInfo -or -not $handoffInfo.isDraft) { throw "The handoff draft $($first.handoffTag) is missing or not a draft." }
    $manifestFolder = Join-Path $root 'handoff-manifest'
    [IO.Directory]::CreateDirectory($manifestFolder) | Out-Null
    Invoke-Gh $tools @('release', 'download', $first.handoffTag, '--repo', $repo, '--pattern', 'signing-manifest.json', '--dir', $manifestFolder, '--clobber') | Out-Null
    $manifestPath = Join-Path $manifestFolder 'signing-manifest.json'
    if ((Get-FileSha256 $manifestPath) -cne $first.manifestSha256) { throw 'The handoff manifest differs from the one the packaging run verified.' }
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    if ($manifest.schemaVersion -cne $script:ManifestSchema -or $manifest.product -cne $spec.Product -or "$($manifest.sourceRunId)" -cne "$($first.sourceRunId)" -or $manifest.sourceCommit -cne $first.sourceCommit) {
        throw 'The handoff manifest does not match the packaging provenance.'
    }
    if ($manifest.signer.thumbprint -cne $config.thumbprint) { throw 'The handoff was signed with another certificate.' }

    if (-not $Tag -and $spec.Product -eq 'dotnet') { $Tag = $manifest.sourceRef }
    if (-not $Tag) { throw 'Give -Tag (rust-preview-<version>) for the Rust edition.' }
    if ($Tag -cnotmatch $spec.TagPattern) { throw "Tag '$Tag' does not match $($spec.TagPattern)." }
    if ($spec.Product -eq 'dotnet' -and $Tag -cne $manifest.sourceRef) { throw "The .NET release tag must be the tag that built the binaries ($($manifest.sourceRef))." }

    $release = Join-Path $root 'release'
    [IO.Directory]::CreateDirectory($release) | Out-Null
    $peTargets = [Collections.Generic.List[string]]::new(); $msixTargets = [Collections.Generic.List[string]]::new(); $signTargets = [Collections.Generic.List[string]]::new()
    $names = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($arch in $spec.Architectures) {
        $folder = Join-Path $root "packages\$arch"
        $main = @($manifest.files | Where-Object { $_.architecture -ceq $arch -and $_.path -ceq $spec.MainExecutable })
        if ($main.Count -ne 1) { throw "The handoff manifest has no $arch $($spec.MainExecutable)." }
        foreach ($rule in $spec.ReleaseFiles) {
            $found = @(Get-ChildItem -Path (Join-Path $folder $rule.Pattern) -File)
            if ($found.Count -ne 1) { throw "Expected one $($rule.Pattern) in the $arch packages; found $($found.Count)." }
            $name = $found[0].Name
            if (-not $names.Add($name)) { throw "Two packages are both named $name." }
            $destination = Join-Path $release $name
            $resume = $rule.Sign -and (Test-Path -LiteralPath $destination) -and (Test-ReleaseSigned -Tools $tools -Config $config -Path $destination)
            if (-not $resume) { Copy-Item -LiteralPath $found[0].FullName -Destination $destination -Force }
            if ($rule.ContainsMain) {
                $inner = & $tools.ArchiveEntrySha256 $destination $spec.MainExecutable
                if ($inner -cne $main[0].signedSha256) { throw "$name does not contain the signed $($spec.MainExecutable) from the handoff." }
            }
            if ($rule.Sign -eq 'Msix') {
                $publisher = & $tools.MsixPublisher $destination
                if ($publisher -cne $config.subject) { throw "$name has publisher '$publisher'; signing needs exactly '$($config.subject)'." }
            }
            if ($rule.Sign) {
                $signTargets.Add($destination)
                if (-not $resume) {
                    $state = Get-SignatureState -Tools $tools -Config $config -Path $destination
                    if ($state.State -notin 'NotSigned', 'Owner') { throw "$name already carries another signature ($($state.Status), $($state.Subject))." }
                    if ($rule.Sign -eq 'Pe') { $peTargets.Add($destination) } else { $msixTargets.Add($destination) }
                }
            }
        }
    }
    $expected = [Collections.Generic.HashSet[string]]::new($names, [StringComparer]::OrdinalIgnoreCase)
    foreach ($file in Get-ChildItem -LiteralPath $release -File) {
        $generated = ($spec.Checksums -eq 'Sums' -and $file.Name -ceq 'SHA256SUMS.txt') -or ($spec.Checksums -eq 'Sidecar' -and $expected.Contains(($file.Name -replace '\.sha256$', '')) -and $file.Name -like '*.sha256')
        if (-not $expected.Contains($file.Name) -and -not $generated) { throw "Unexpected file in $release`: $($file.Name). Delete $release and run again." }
    }
    if ($peTargets.Count -gt 0) { Invoke-SignFiles -Tools $tools -Config $config -Paths $peTargets.ToArray() -Kind Pe }
    if ($msixTargets.Count -gt 0) { Invoke-SignFiles -Tools $tools -Config $config -Paths $msixTargets.ToArray() -Kind Msix }
    foreach ($target in $signTargets) { Assert-ReleaseSignature -Tools $tools -Config $config -Path $target }

    if ($spec.Checksums -eq 'Sums') {
        Write-Sha256Sums -Folder $release -FileName 'SHA256SUMS.txt'
    } else {
        foreach ($name in $names) { Write-Sha256Sidecar (Join-Path $release $name) }
    }
    $assets = [string[]]@(Get-ChildItem -LiteralPath $release -File | ForEach-Object FullName)
    [Array]::Sort($assets, [StringComparer]::Ordinal)
    Write-JsonFile ([ordered]@{
            schemaVersion = 'marknexia-release-signing-evidence-v1'; product = $spec.Product; tag = $Tag
            sourceRunId = "$($manifest.sourceRunId)"; sourceCommit = $manifest.sourceCommit; packagingRunId = $PackagingRunId; handoffTag = $first.handoffTag
            signerThumbprint = $config.thumbprint; signedNow = @($signTargets | ForEach-Object { Split-Path $_ -Leaf })
            assets = @($assets | ForEach-Object { [ordered]@{ name = (Split-Path $_ -Leaf); sha256 = (Get-FileSha256 $_) } })
        }) (Join-Path $root 'release-signing-evidence.json')

    $notesPath = Join-Path $root 'release-notes.md'
    [IO.File]::WriteAllText($notesPath, (Get-ReleaseNotes -Spec $spec -Config $config -Manifest $manifest -PackagingRunId $PackagingRunId -Tag $Tag), [Text.UTF8Encoding]::new($false))
    $createArguments = if ($spec.Prerelease) {
        @('release', 'create', $Tag, '--repo', $repo, '--target', $manifest.sourceCommit, '--prerelease', '--latest=false', '--title', "$($spec.DisplayName) $Tag", '--notes-file', $notesPath)
    } else {
        @('release', 'create', $Tag, '--repo', $repo, '--verify-tag', '--latest', '--title', $Tag, '--notes-file', $notesPath)
    }
    if (-not $Context.Publish) {
        Write-SigningLog $tools "DRY RUN: $($assets.Count) signed and checked release file(s) are in $release. Nothing was published."
        Write-SigningLog $tools "Run again with -Publish to create the $(if ($spec.Prerelease) { 'pre-release' } else { 'release' }) $Tag."
        return [pscustomobject]@{ Tag = $Tag; ReleaseFolder = $release; Assets = $assets; Published = $false }
    }

    if (Get-ReleaseInfo $tools $repo $Tag) { throw "A release $Tag already exists. Nothing was published." }
    $tagCommit = Get-RemoteTagCommit $tools $repo $Tag
    if ($spec.Prerelease) {
        if ($tagCommit) { throw "Tag $Tag already exists. Choose a new rust-preview-* tag." }
    } elseif ($tagCommit -cne $manifest.sourceCommit) {
        throw "Tag $Tag must exist and point to $($manifest.sourceCommit) (found '$tagCommit')."
    }
    Invoke-Gh $tools ($createArguments + $assets) | Out-Null
    $published = Get-ReleaseInfo $tools $repo $Tag
    if (-not $published -or $published.isDraft -or [bool]$published.isPrerelease -ne [bool]$spec.Prerelease) { throw "The release $Tag was not published as expected; check it on GitHub." }
    $publishedNames = @($published.assets | ForEach-Object name | Sort-Object)
    $localNames = @($assets | ForEach-Object { Split-Path $_ -Leaf } | Sort-Object)
    if (($publishedNames -join '|') -cne ($localNames -join '|')) { throw "The release $Tag has other assets than $release; check it on GitHub." }
    Write-SigningLog $tools "Published $Tag with $($assets.Count) asset(s). Remove the handoff draft: -Stage Cleanup -RunId $($manifest.sourceRunId) -Publish"
    [pscustomobject]@{ Tag = $Tag; ReleaseFolder = $release; Assets = $assets; Published = $true }
}

# --- Cleanup -------------------------------------------------------------------------------

function Invoke-CleanupStage {
    [CmdletBinding()]
    param([Parameter(Mandatory)][hashtable]$Context, [Parameter(Mandatory)][string]$SourceRunId)
    $tools = $Context.Tools; $repo = $Context.Repository
    $tag = Get-HandoffTag $Context.Spec.Product $SourceRunId
    $info = Get-ReleaseInfo $tools $repo $tag
    if (-not $info) { Write-SigningLog $tools "No handoff draft $tag exists."; return $false }
    if (-not $info.isDraft -or -not "$($info.name)".StartsWith($script:HandoffTitlePrefix, [StringComparison]::Ordinal)) {
        throw "$tag is not a signing handoff draft. Nothing was removed."
    }
    if (-not $Context.Publish) { Write-SigningLog $tools "DRY RUN: would remove the draft release $tag. Run again with -Publish."; return $false }
    Invoke-Gh $tools @('release', 'delete', $tag, '--repo', $repo, '--yes') | Out-Null
    Write-SigningLog $tools "Removed the draft release $tag."
    $true
}

Export-ModuleMember -Function Get-ReleaseSigningConfig, Get-ProductSpec, Get-HandoffTag, Find-SignTool, New-SigningTools,
    Get-FileSha256, Get-PeContentDigest, Get-ZipEntrySha256, Get-MsixPublisher, Assert-SigningCertificate, Get-SignatureState,
    Test-ReleaseSigned, Get-SignToolSignArguments, Invoke-SignFiles, Assert-ReleaseSignature, Test-SigningSession, Get-PeFiles,
    Get-SigningPlan, Get-WorkflowRun, Assert-WorkflowRun, Save-RunArtifact, Get-ReleaseInfo, Get-RemoteTagCommit,
    ConvertTo-HandoffAssetName, Write-Sha256Sums, Write-Sha256Sidecar, Invoke-BinariesStage, Invoke-PackagesStage, Invoke-CleanupStage
