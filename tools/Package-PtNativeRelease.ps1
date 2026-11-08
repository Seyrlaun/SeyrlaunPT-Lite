# SPDX-License-Identifier: Apache-2.0
#requires -Version 7.0

[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$NativeMod,
    [Parameter(Mandatory)]
    [ValidatePattern('^[0-9A-Fa-f]{64}$')]
    [string]$ExpectedNativeModSha256,
    [Parameter(Mandatory)]
    [string]$OidnArchive,
    [Parameter(Mandatory)]
    [string]$OutputDirectory,
    [ValidatePattern('^[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?$')]
    [string]$Version = '0.1.0-alpha.2',
    [switch]$VerifyOnly
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type -AssemblyName System.IO.Compression

$repository = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$metadataRoot = Join-Path $PSScriptRoot 'pt_native_bundle'
$runtimeManifestName = 'oidn-2.5.1-windows-x64.manifest.json'
$metadataLayout = 'developer'
if (-not (Test-Path -LiteralPath (Join-Path $metadataRoot $runtimeManifestName) -PathType Leaf)) {
    $metadataRoot = $repository
    $metadataLayout = 'public-source'
}
$runtimeManifestPath = Join-Path $metadataRoot $runtimeManifestName
if (-not (Test-Path -LiteralPath $runtimeManifestPath -PathType Leaf)) {
    throw 'OIDN metadata was not found in tools/pt_native_bundle or the repository root.'
}
$runtimeManifest = Get-Content -LiteralPath $runtimeManifestPath -Raw | ConvertFrom-Json
$runtimePrefix = 'native-tools/offline_pt_lite/oidn'
$horizon = '9d979caa48a3bb58203319b9ea044db58c38e0d8'
$mixins = '5a64af938ec86b00d53d16a408aabc0038fb3e57'
$officialArchiveHash = 'F11F91BC072A5E3A564515724CB72AB8FCFBC445C84A84C197FF9B16CC01396F'
$officialArchiveUrl = 'https://github.com/RenderKit/oidn/releases/download/v2.5.1/oidn-2.5.1.x64.windows.zip'

function Get-FileRecord([string]$Path, [string]$Name) {
    $file = Get-Item -LiteralPath $Path
    if ($file.PSIsContainer -or ($file.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "Expected an ordinary file: $Path"
    }
    [pscustomobject][ordered]@{
        path = $Name
        bytes = $file.Length
        sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
    }
}

function Get-StreamHash([IO.Stream]$Stream) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try { [BitConverter]::ToString($sha.ComputeHash($Stream)).Replace('-', '') }
    finally { $sha.Dispose() }
}

function Get-EntryRecord([IO.Compression.ZipArchiveEntry]$Entry, [string]$Name) {
    $stream = $Entry.Open()
    try {
        [pscustomobject][ordered]@{
            path = $Name
            bytes = $Entry.Length
            sha256 = Get-StreamHash $stream
        }
    } finally { $stream.Dispose() }
}

function Assert-RelativePath([string]$Name) {
    if (-not $Name -or $Name.Contains('\') -or $Name.Contains(':') -or
        $Name.StartsWith('/') -or $Name.Contains([char]0) -or
        ($Name.Split('/') | Where-Object { $_ -in @('', '.', '..') })) {
        throw "Unsafe archive path: $Name"
    }
}

function Get-Entries([IO.Compression.ZipArchive]$Zip) {
    if ($Zip.Entries.Count -gt 512) { throw 'Archive has more than 512 entries.' }
    $entries = [Collections.Generic.Dictionary[string, IO.Compression.ZipArchiveEntry]]::new(
        [StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in $Zip.Entries) {
        $name = $entry.FullName.TrimEnd('/')
        Assert-RelativePath $name
        if ($entry.FullName.EndsWith('/')) {
            if ($entry.Length -ne 0) { throw "Nonempty directory entry: $name" }
            continue
        }
        if (-not $entries.TryAdd($name, $entry)) { throw "Duplicate archive path: $name" }
    }
    ,$entries
}

function Write-NewText([string]$Path, [string]$Text) {
    $stream = [IO.File]::Open($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
    try {
        $bytes = [Text.UTF8Encoding]::new($false).GetBytes($Text)
        $stream.Write($bytes, 0, $bytes.Length)
    } finally { $stream.Dispose() }
}

function Join-Contained([string]$Root, [string]$Name) {
    Assert-RelativePath $Name
    $path = [IO.Path]::GetFullPath((Join-Path $Root $Name))
    if (-not $path.StartsWith($Root + [IO.Path]::DirectorySeparatorChar,
            [StringComparison]::OrdinalIgnoreCase)) {
        throw "Path leaves output root: $Name"
    }
    $path
}

function Copy-NewFile([string]$Source, [string]$Destination) {
    [IO.Directory]::CreateDirectory((Split-Path -Parent $Destination)) | Out-Null
    [IO.File]::Copy($Source, $Destination, $false)
}

function Assert-Matches($Actual, $Expected) {
    if ($Actual.bytes -ne $Expected.bytes -or $Actual.sha256 -cne $Expected.sha256) {
        throw "Size or SHA-256 mismatch: $($Actual.path)"
    }
}

$nativePath = (Get-Item -LiteralPath $NativeMod).FullName
$oidnPath = (Get-Item -LiteralPath $OidnArchive).FullName
$nativeRecord = Get-FileRecord $nativePath 'horizon-native-mods/offline_pt_lite.hnmod'
if ($nativeRecord.sha256 -cne $ExpectedNativeModSha256.ToUpperInvariant()) {
    throw 'Native mod SHA-256 does not match the reviewed asset.'
}
if ($runtimeManifest.schemaVersion -ne 1 -or $runtimeManifest.version -ne '2.5.1' -or
    $runtimeManifest.platform -ne 'windows-x64' -or
    $runtimeManifest.source.archiveSha256 -cne $officialArchiveHash -or
    $runtimeManifest.source.archiveUrl -cne $officialArchiveUrl) {
    throw 'The checked-in OIDN manifest does not match the pinned official release.'
}
Assert-Matches (Get-FileRecord $oidnPath 'OIDN archive') ([pscustomobject]@{
    bytes = $runtimeManifest.source.archiveBytes
    sha256 = $officialArchiveHash
})

$nativeZip = [IO.Compression.ZipFile]::OpenRead($nativePath)
$oidnZip = $null
try {
    $nativeEntries = Get-Entries $nativeZip
    if (-not $nativeEntries.ContainsKey('horizon-native.json') -or
        -not $nativeEntries.ContainsKey('mixins/offline_pt_lite/horizon-mixin.toml')) {
        throw 'Native mod root manifest or mixin manifest is missing.'
    }
    $nativeBytes = 0L
    $dataFiles = @('path_tracer.wgsl', 'path_tracer_ctm.wgsl',
        'LICENSE-Apache-2.0.txt', 'LICENSE-Horizon.txt', 'NOTICE.txt', 'provenance.json')
    foreach ($entry in $nativeEntries.Values) {
        $name = $entry.FullName
        $nativeBytes += $entry.Length
        if ($nativeBytes -gt 64MB) { throw 'Native mod exceeds the 64 MiB unpacked limit.' }
        if ($name -eq 'horizon-native.json') { continue }
        $prefix = 'mixins/offline_pt_lite/'
        if (-not $name.StartsWith($prefix, [StringComparison]::Ordinal)) {
            throw "Unexpected native mod entry: $name"
        }
        $relative = $name.Substring($prefix.Length)
        if ($relative.StartsWith('data/', [StringComparison]::Ordinal)) {
            if ($relative.Substring(5) -cnotin $dataFiles) {
                throw "Unexpected native mod data file: $name"
            }
        } elseif ([IO.Path]::GetExtension($relative) -cnotin @('.rs', '.toml')) {
            throw "Native mod must contain only its reviewed source and shader data: $name"
        }
    }
    $manifestStream = $nativeEntries['horizon-native.json'].Open()
    $reader = [IO.StreamReader]::new($manifestStream, [Text.UTF8Encoding]::new($false, $true))
    try { $nativeManifest = $reader.ReadToEnd() | ConvertFrom-Json }
    finally { $reader.Dispose() }
    if ($nativeManifest.schema -ne 1 -or $nativeManifest.id -cne 'offline_pt_lite' -or
        $nativeManifest.version -cne $Version -or $nativeManifest.game -cne '26.3' -or
        $nativeManifest.horizon -cne $horizon -or $nativeManifest.mixins -cne $mixins) {
        throw 'Native mod identity/version/compatibility does not match this release.'
    }

    $oidnZip = [IO.Compression.ZipFile]::OpenRead($oidnPath)
    $oidnEntries = Get-Entries $oidnZip
    $packagePrefix = $runtimeManifest.packageRoot + '/'
    $selected = [Collections.Generic.Dictionary[string, object]]::new([StringComparer]::Ordinal)
    $totalBytes = 0L
    foreach ($file in $runtimeManifest.files) {
        Assert-RelativePath $file.path
        if (-not $selected.TryAdd($file.path, $file)) { throw 'Duplicate runtime manifest entry.' }
        $archiveName = $packagePrefix + $file.path
        if (-not $oidnEntries.ContainsKey($archiveName)) {
            throw "Pinned OIDN file is missing: $archiveName"
        }
        Assert-Matches (Get-EntryRecord $oidnEntries[$archiveName] $file.path) $file
        $totalBytes += $file.bytes
    }
    if ($selected.Count -ne $runtimeManifest.fileCount -or
        $totalBytes -ne $runtimeManifest.totalBytes) {
        throw 'OIDN runtime manifest totals do not match its file list.'
    }
    foreach ($name in $oidnEntries.Keys) {
        if (-not $name.StartsWith($packagePrefix, [StringComparison]::Ordinal)) {
            throw "OIDN entry outside its pinned package root: $name"
        }
        $relative = $name.Substring($packagePrefix.Length)
        if (($relative.StartsWith('doc/', [StringComparison]::Ordinal) -or
                ($relative.StartsWith('bin/', [StringComparison]::Ordinal) -and
                 [IO.Path]::GetExtension($relative) -ieq '.dll')) -and
            -not $selected.ContainsKey($relative)) {
            throw "An upstream DLL, document or license would be omitted: $relative"
        }
    }
    $documents = [ordered]@{
        'README.md' = Join-Path $metadataRoot 'OIDN-RELEASE.md'
        'LICENSE' = if ($metadataLayout -eq 'developer') {
            Join-Path $metadataRoot 'LICENSE-APACHE-2.0'
        } else {
            Join-Path $metadataRoot 'LICENSE'
        }
        'NOTICE' = Join-Path $metadataRoot 'NOTICE'
        'LICENSE-Horizon.txt' = if ($metadataLayout -eq 'developer') {
            Join-Path $repository 'LICENSE'
        } else {
            Join-Path $metadataRoot 'LICENSE-Horizon.txt'
        }
    }
    foreach ($source in $documents.Values) { Get-FileRecord $source 'release document' | Out-Null }
    $output = [IO.Path]::GetFullPath($OutputDirectory)
    if (Test-Path -LiteralPath $output) { throw "Refusing an existing output path: $output" }
    $parent = Split-Path -Parent $output
    $resolvedParent = (Get-Item -LiteralPath $parent)
    if (-not $resolvedParent.PSIsContainer -or
        ($resolvedParent.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw 'The output parent must be an existing ordinary directory.'
    }
    if ($VerifyOnly) {
        [pscustomobject]@{
            verified = $true
            version = $Version
            nativeModSha256 = $nativeRecord.sha256
            oidnArchiveSha256 = $officialArchiveHash
            runtimeFiles = $selected.Count
            runtimeBytes = $totalBytes
            metadataLayout = $metadataLayout
            outputCreated = $false
        } | ConvertTo-Json
        return
    }

    # A fresh output is never merged, overwritten or recursively removed, including on failure.
    [IO.Directory]::CreateDirectory($output) | Out-Null
    $payload = Join-Path $output 'payload'
    [IO.Directory]::CreateDirectory($payload) | Out-Null
    $records = [Collections.Generic.List[object]]::new()
    foreach ($name in $documents.Keys) {
        $to = Join-Contained $payload $name
        Copy-NewFile $documents[$name] $to
        $records.Add((Get-FileRecord $to $name))
    }
    $to = Join-Contained $payload $nativeRecord.path
    Copy-NewFile $nativePath $to
    $copiedMod = Get-FileRecord $to $nativeRecord.path
    Assert-Matches $copiedMod $nativeRecord
    $records.Add($copiedMod)
    foreach ($name in ($selected.Keys | Sort-Object -CaseSensitive)) {
        $entry = $oidnEntries[$packagePrefix + $name]
        $to = Join-Contained $payload ($runtimePrefix + '/' + $name)
        [IO.Directory]::CreateDirectory((Split-Path -Parent $to)) | Out-Null
        $source = $entry.Open()
        $destination = [IO.File]::Open($to, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
        try { $source.CopyTo($destination) }
        finally { $destination.Dispose(); $source.Dispose() }
        $record = Get-FileRecord $to ($runtimePrefix + '/' + $name)
        Assert-Matches $record $selected[$name]
        $records.Add($record)
    }
    $manifestName = $runtimePrefix + '/runtime-manifest.json'
    $to = Join-Contained $payload $manifestName
    Copy-NewFile $runtimeManifestPath $to
    $records.Add((Get-FileRecord $to $manifestName))
    $releaseManifest = [ordered]@{
        schemaVersion = 1
        product = 'Offline PT Lite'
        version = $Version
        platform = 'windows-x64'
        horizonRevision = $horizon
        horizonMixinsRevision = $mixins
        nativeModSha256 = $nativeRecord.sha256
        oidnSource = $runtimeManifest.source
        files = @($records | Sort-Object -Property path -CaseSensitive)
    }
    $to = Join-Contained $payload 'release-manifest.json'
    Write-NewText $to (($releaseManifest | ConvertTo-Json -Depth 8) + "`n")
    $records.Add((Get-FileRecord $to 'release-manifest.json'))
    $sums = ($records | Sort-Object -Property path -CaseSensitive |
        ForEach-Object { "$($_.sha256)  $($_.path)" }) -join "`n"
    $sumsPath = Join-Contained $payload 'SHA256SUMS'
    Write-NewText $sumsPath ($sums + "`n")
    $zipRecords = @($records) + (Get-FileRecord $sumsPath 'SHA256SUMS')

    $artifactName = "offline-pt-lite-$Version-windows-x64-with-oidn"
    $zipPath = Join-Path $output ($artifactName + '.zip')
    $releaseZip = [IO.Compression.ZipFile]::Open($zipPath, [IO.Compression.ZipArchiveMode]::Create)
    try {
        $names = @($records.path) + 'SHA256SUMS'
        foreach ($name in ($names | Sort-Object -CaseSensitive)) {
            $entry = $releaseZip.CreateEntry($name, [IO.Compression.CompressionLevel]::Optimal)
            $entry.LastWriteTime = [DateTimeOffset]::new(2000, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
            $source = [IO.File]::OpenRead((Join-Contained $payload $name))
            $destination = $entry.Open()
            try { $source.CopyTo($destination) }
            finally { $destination.Dispose(); $source.Dispose() }
        }
    } finally { $releaseZip.Dispose() }
    $roundTrip = [IO.Compression.ZipFile]::OpenRead($zipPath)
    try {
        $writtenEntries = Get-Entries $roundTrip
        if ($writtenEntries.Count -ne $zipRecords.Count) {
            throw 'Release ZIP entry count does not match its payload.'
        }
        foreach ($record in $zipRecords) {
            if (-not $writtenEntries.ContainsKey($record.path)) {
                throw "Release ZIP entry is missing: $($record.path)"
            }
            Assert-Matches (Get-EntryRecord $writtenEntries[$record.path] $record.path) $record
        }
    } finally { $roundTrip.Dispose() }
    $publicModName = "offline-pt-lite-$Version.hnmod"
    Copy-NewFile $nativePath (Join-Path $output $publicModName)
    $publicMod = Get-FileRecord (Join-Path $output $publicModName) $publicModName
    Assert-Matches $publicMod $nativeRecord
    $assets = @(
        $publicMod
        Get-FileRecord $zipPath ($artifactName + '.zip')
    )
    Write-NewText (Join-Path $output 'SHA256SUMS') (
        (($assets | ForEach-Object { "$($_.sha256)  $($_.path)" }) -join "`n") + "`n")
    [pscustomobject]@{
        outputDirectory = $output
        runtimeFiles = $selected.Count
        runtimeBytes = $totalBytes
        metadataLayout = $metadataLayout
        assets = $assets
    } | ConvertTo-Json -Depth 5
} finally {
    if ($oidnZip) { $oidnZip.Dispose() }
    $nativeZip.Dispose()
}
