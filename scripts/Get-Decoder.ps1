#Requires -Version 5.1
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$Destination
)

$ErrorActionPreference = 'Stop'
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor 12288
} catch {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
}

function Assert-NoReparsePoint {
    param([Parameter(Mandatory)][string]$Path)
    $current = [IO.Path]::GetFullPath($Path)
    while (-not [string]::IsNullOrEmpty($current)) {
        if (Test-Path -LiteralPath $current) {
            $attrs = [IO.File]::GetAttributes($current)
            if ($attrs -band [IO.FileAttributes]::ReparsePoint) {
                throw "Refusing reparse point in path: $current"
            }
        }
        $parent = [IO.Path]::GetDirectoryName($current)
        if ($parent -eq $current) { break }
        $current = $parent
    }
}

function New-RandomHex {
    param([int]$ByteCount = 16)
    $bytes = New-Object byte[] $ByteCount
    $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $rng.GetBytes($bytes)
    } finally {
        $rng.Dispose()
    }
    return ([BitConverter]::ToString($bytes).Replace('-', '').ToLowerInvariant())
}

function Invoke-PinnedHttpsDownload {
    param(
        [Parameter(Mandatory)][string]$Uri,
        [Parameter(Mandatory)][string]$OutFile,
        [Parameter(Mandatory)][string[]]$AllowedHosts,
        [long]$MaxBytes = 209715200
    )
    $current = [Uri]$Uri
    for ($hop = 0; $hop -lt 6; $hop++) {
        if ($current.Scheme -ne 'https') {
            throw "Refusing non-HTTPS URL: $current"
        }
        $hostAllowed = $false
        foreach ($allowed in $AllowedHosts) {
            if ($current.Host.Equals($allowed, [StringComparison]::OrdinalIgnoreCase)) {
                $hostAllowed = $true
                break
            }
        }
        if (-not $hostAllowed) {
            throw "Refusing unapproved download host: $($current.Host)"
        }
        $request = [Net.HttpWebRequest]::Create($current)
        $request.Method = 'GET'
        $request.AllowAutoRedirect = $false
        $request.Timeout = 120000
        $request.ReadWriteTimeout = 120000
        $request.UserAgent = 'WallpaperLive-Setup'
        $response = $null
        try {
            try {
                $response = [Net.HttpWebResponse]$request.GetResponse()
            } catch [Net.WebException] {
                if ($_.Exception.Response) {
                    $response = [Net.HttpWebResponse]$_.Exception.Response
                } else {
                    throw
                }
            }
            $code = [int]$response.StatusCode
            if ($code -in 301, 302, 303, 307, 308) {
                $location = $response.Headers['Location']
                if ([string]::IsNullOrWhiteSpace($location)) {
                    throw "Redirect from $current lacked a Location header."
                }
                $current = New-Object Uri($current, $location)
                continue
            }
            if ($code -ne 200) {
                throw "Download failed with HTTP $code from $current"
            }
            if ($response.ContentLength -gt $MaxBytes) {
                throw "Download exceeds maximum allowed size ($MaxBytes bytes)."
            }
            $stream = $response.GetResponseStream()
            $file = [IO.File]::Open($OutFile, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
            try {
                $buffer = New-Object byte[] 65536
                [long]$total = 0
                while (($read = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) {
                    $total += $read
                    if ($total -gt $MaxBytes) {
                        throw "Download exceeded maximum allowed size ($MaxBytes bytes)."
                    }
                    $file.Write($buffer, 0, $read)
                }
            } finally {
                $file.Dispose()
                $stream.Dispose()
            }
            return
        } finally {
            if ($response) { $response.Dispose() }
        }
    }
    throw 'Too many HTTP redirects during download.'
}

if (-not $PSBoundParameters.ContainsKey('Destination')) {
    $Destination = Join-Path (Split-Path -Parent $PSScriptRoot) 'runtime'
}
if ([string]::IsNullOrWhiteSpace($Destination)) { throw 'Destination cannot be empty.' }
$url = 'https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261107/mpv-dev-x86_64-20261107-git-9028187538.7z'
$allowedHosts = @('github.com', 'objects.githubusercontent.com', 'release-assets.githubusercontent.com')
$archiveHash = 'df7c433a2d481fdaf6163b201f26abbb0eff0c215c51368397088c6d8f7a001b'
$dllHash = 'c91949577ae9cc2d500208da51f2c4a24cfa7a02d0ce6e130bf67ec738b450bb'
$Destination = [IO.Path]::GetFullPath($Destination)
Assert-NoReparsePoint -Path $Destination
$dll = Join-Path $Destination 'libmpv-2.dll'

if (Test-Path -LiteralPath $dll) {
    Assert-NoReparsePoint -Path $dll
    if (-not (Test-Path -LiteralPath $dll -PathType Leaf)) { throw "Not a regular DLL file: $dll" }
    if ((Get-FileHash -LiteralPath $dll -Algorithm SHA256).Hash -ne $dllHash) {
        throw "Existing decoder differs from pinned build; left untouched: $dll"
    }
    Write-Output "Decoder already verified; no download: $dll"
    return
}
if ((Test-Path -LiteralPath $Destination) -and -not (Test-Path -LiteralPath $Destination -PathType Container)) {
    throw "Destination is not a directory: $Destination"
}
if (-not $PSCmdlet.ShouldProcess($Destination, 'Download and verify pinned native decoder')) { return }
$tarExe = Join-Path $env:SystemRoot 'System32\tar.exe'
if (-not (Test-Path -LiteralPath $tarExe -PathType Leaf)) {
    throw "System tar.exe not found at $tarExe"
}
Assert-NoReparsePoint -Path $tarExe

if (-not (Test-Path -LiteralPath $Destination)) {
    [IO.Directory]::CreateDirectory($Destination) | Out-Null
}
Assert-NoReparsePoint -Path $Destination

$stage = Join-Path $Destination ('.decoder-setup-' + (New-RandomHex))
[IO.Directory]::CreateDirectory($stage) | Out-Null
$archive = Join-Path $stage 'mpv-dev.7z'
$extracted = Join-Path $stage 'libmpv-2.dll'

try {
    Write-Output "Downloading pinned decoder from $url"
    Invoke-PinnedHttpsDownload -Uri $url -OutFile $archive -AllowedHosts $allowedHosts -MaxBytes 209715200
    Assert-NoReparsePoint -Path $archive
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $archiveHash) {
        throw 'Archive SHA256 mismatch; nothing installed.'
    }
    & $tarExe -xf $archive -C $stage 'libmpv-2.dll'
    if ($LASTEXITCODE -ne 0) { throw "tar extraction failed (exit $LASTEXITCODE)." }
    if (-not (Test-Path -LiteralPath $extracted -PathType Leaf)) {
        throw 'Archive did not contain libmpv-2.dll.'
    }
    Assert-NoReparsePoint -Path $extracted
    if ((Get-FileHash -LiteralPath $extracted -Algorithm SHA256).Hash -ne $dllHash) {
        throw 'Decoder SHA256 mismatch; nothing installed.'
    }
    Assert-NoReparsePoint -Path $Destination
    [IO.File]::Copy($extracted, $dll, $false)
    Write-Output "Decoder verified: $dll"
} finally {
    foreach ($tempFile in @($extracted, $archive)) {
        if (Test-Path -LiteralPath $tempFile -PathType Leaf) {
            try { [IO.File]::Delete($tempFile) } catch {}
        }
    }
    if (Test-Path -LiteralPath $stage -PathType Container) {
        try { [IO.Directory]::Delete($stage, $false) } catch {}
    }
}
