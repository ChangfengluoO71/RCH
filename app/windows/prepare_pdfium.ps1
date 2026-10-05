[CmdletBinding()]
param(
    [string] $Destination = (Join-Path $PSScriptRoot 'pdfium/win-x64'),
    [switch] $Force
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Pin an immutable upstream release so Debug and Release builds use the same
# native ABI. Update both values from the upstream release asset metadata.
$release = 'chromium/8076'
$archiveUrl = 'https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F8076/pdfium-win-x64.tgz'
$expectedSha256 = '808d36da9bc5a3104315fb307c80998121f565ee53953633bf33e80d7429e5ac'

$destinationPath = [System.IO.Path]::GetFullPath($Destination)
$destinationDll = Join-Path $destinationPath 'pdfium.dll'
$destinationLicense = Join-Path $destinationPath 'pdfium-LICENSE.txt'
if (-not $Force -and (Test-Path -LiteralPath $destinationDll) -and
    (Test-Path -LiteralPath $destinationLicense)) {
    Write-Host "PDFium $release is already prepared at $destinationPath"
    exit 0
}

$tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$workDirectory = Join-Path $tempRoot ("rch-pdfium-" + [Guid]::NewGuid().ToString('N'))
$archivePath = Join-Path $workDirectory 'pdfium-win-x64.tgz'
$extractPath = Join-Path $workDirectory 'extract'

try {
    New-Item -ItemType Directory -Path $extractPath -Force | Out-Null
    Invoke-WebRequest -Uri $archiveUrl -OutFile $archivePath -UseBasicParsing

    $actualSha256 = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualSha256 -ne $expectedSha256) {
        throw "PDFium archive hash mismatch for $release. Expected $expectedSha256, received $actualSha256."
    }

    $tar = Get-Command tar.exe -ErrorAction Stop
    & $tar.Source -xzf $archivePath -C $extractPath
    if ($LASTEXITCODE -ne 0) {
        throw "Could not extract PDFium archive (tar exit code $LASTEXITCODE)."
    }

    $sourceDll = Join-Path $extractPath 'bin/pdfium.dll'
    $sourceLicense = Join-Path $extractPath 'LICENSE'
    if (-not (Test-Path -LiteralPath $sourceDll) -or
        -not (Test-Path -LiteralPath $sourceLicense)) {
        throw 'The upstream archive did not contain bin/pdfium.dll and LICENSE.'
    }

    $stream = [System.IO.File]::OpenRead($sourceDll)
    try {
        $reader = New-Object System.IO.BinaryReader($stream)
        $dosSignature = [System.Text.Encoding]::ASCII.GetString($reader.ReadBytes(2))
        if ($dosSignature -ne 'MZ') {
            throw 'The downloaded pdfium.dll is not a Windows PE binary.'
        }
        $stream.Position = 0x3C
        $peOffset = $reader.ReadInt32()
        if ($peOffset -lt 0 -or $peOffset -gt ($stream.Length - 6)) {
            throw 'The downloaded pdfium.dll has an invalid PE header.'
        }
        $stream.Position = $peOffset
        $peSignature = [System.Text.Encoding]::ASCII.GetString($reader.ReadBytes(4))
        $machine = $reader.ReadUInt16()
        if ($peSignature -ne "PE`0`0" -or $machine -ne 0x8664) {
            throw 'The downloaded pdfium.dll is not a Windows x64 PE binary.'
        }
    }
    finally {
        $stream.Dispose()
    }

    New-Item -ItemType Directory -Path $destinationPath -Force | Out-Null
    Copy-Item -LiteralPath $sourceDll -Destination $destinationDll -Force
    Copy-Item -LiteralPath $sourceLicense -Destination $destinationLicense -Force
    Write-Host "Prepared PDFium $release at $destinationPath"
}
finally {
    $workPath = [System.IO.Path]::GetFullPath($workDirectory)
    $safePrefix = $tempRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
    if ($workPath.StartsWith($safePrefix, [System.StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $workPath).StartsWith('rch-pdfium-', [System.StringComparison]::OrdinalIgnoreCase) -and
        (Test-Path -LiteralPath $workPath)) {
        Remove-Item -LiteralPath $workPath -Recurse -Force
    }
}
