#Requires -Version 5.1
[CmdletBinding()]
param([string]$Destination)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $PSBoundParameters.ContainsKey('Destination')) { $Destination = Join-Path $root 'dist' }
if ([string]::IsNullOrWhiteSpace($Destination)) { throw 'Destination cannot be empty.' }
$exe = Join-Path $root 'target\release\wallpaperlive.exe'
$files = @($exe, (Join-Path $root 'README.md'), (Join-Path $root 'LICENSE'), (Join-Path $root 'THIRD_PARTY.md'))
$setup = Join-Path $PSScriptRoot 'Get-Decoder.ps1'
foreach ($file in ($files + @($setup))) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
        throw "Required package file missing: $file. Run scripts/Build.ps1 first."
    }
}
$stream = [IO.File]::OpenRead($exe)
try {
    $reader = New-Object IO.BinaryReader($stream)
    if ($stream.Length -lt 64 -or $reader.ReadUInt16() -ne 0x5A4D) { throw 'Release executable is not a PE image.' }
    $stream.Position = 0x3C
    $pe = $reader.ReadUInt32()
    if ($pe -gt ($stream.Length - 24)) { throw 'Release executable has an invalid PE header.' }
    $stream.Position = $pe
    if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) {
        throw 'Portable package requires a Windows x64 executable.'
    }
}
finally { $stream.Dispose() }
$cargo = Get-Content -LiteralPath (Join-Path $root 'Cargo.toml') -Raw
if ($cargo -notmatch '(?m)^version\s*=\s*"([^"]+)"') {
    throw 'Could not determine version from Cargo.toml.'
}
$version = $Matches[1]
$Destination = [IO.Path]::GetFullPath($Destination)
$package = Join-Path $Destination "WallpaperLive-$version-windows-x64"
$zip = $package + '.zip'
if ((Test-Path -LiteralPath $package) -or (Test-Path -LiteralPath $zip)) {
    throw "Package output already exists; choose a new -Destination. Nothing overwritten: $package"
}
New-Item -ItemType Directory -Path (Join-Path $package 'scripts') | Out-Null
foreach ($file in $files) {
    [IO.File]::Copy($file, (Join-Path $package (Split-Path -Leaf $file)), $false)
}
[IO.File]::Copy($setup, (Join-Path $package 'scripts\Get-Decoder.ps1'), $false)
Compress-Archive -LiteralPath $package -DestinationPath $zip -CompressionLevel Optimal
Write-Output "Package: $zip"
Write-Output 'Decoder excluded. Extract ZIP, then explicitly run scripts/Get-Decoder.ps1 once.'
