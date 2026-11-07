#Requires -Version 5.1
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$Destination
)

$ErrorActionPreference = 'Stop'
if (-not $PSBoundParameters.ContainsKey('Destination')) {
    $Destination = Join-Path (Split-Path -Parent $PSScriptRoot) 'runtime'
}
if ([string]::IsNullOrWhiteSpace($Destination)) { throw 'Destination cannot be empty.' }
$url = 'https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261107/mpv-dev-x86_64-20261107-git-9028187538.7z'
$archiveHash = 'df7c433a2d481fdaf6163b201f26abbb0eff0c215c51368397088c6d8f7a001b'
$dllHash = 'c91949577ae9cc2d500208da51f2c4a24cfa7a02d0ce6e130bf67ec738b450bb'
$Destination = [IO.Path]::GetFullPath($Destination)
$dll = Join-Path $Destination 'libmpv-2.dll'

if (Test-Path -LiteralPath $dll) {
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
$tar = Get-Command tar.exe -CommandType Application -ErrorAction Stop
$stage = Join-Path $Destination ('.decoder-setup-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage -ErrorAction Stop | Out-Null
$archive = Join-Path $stage 'mpv-dev.7z'

# ponytail: retain setup staging files; add explicit cleanup only with user consent.
Write-Output "Downloading pinned decoder from $url"
Write-Output "Setup files retained at: $stage"
Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $archive -TimeoutSec 120
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $archiveHash) {
    throw "Archive SHA256 mismatch; nothing installed. Retained: $archive"
}
& $tar.Source -xf $archive -C $stage 'libmpv-2.dll'
if ($LASTEXITCODE -ne 0) { throw "tar extraction failed (exit $LASTEXITCODE). Retained: $stage" }
$extracted = Join-Path $stage 'libmpv-2.dll'
if ((Get-FileHash -LiteralPath $extracted -Algorithm SHA256).Hash -ne $dllHash) {
    throw "Decoder SHA256 mismatch; nothing installed. Retained: $stage"
}
[IO.File]::Copy($extracted, $dll, $false)
Write-Output "Decoder verified: $dll"
