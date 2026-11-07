#Requires -Version 5.1
[CmdletBinding()]
param([string]$OutputDirectory)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $PSBoundParameters.ContainsKey('OutputDirectory')) { $OutputDirectory = Join-Path $root 'dist' }
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) { throw 'OutputDirectory cannot be empty.' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$output = Join-Path $OutputDirectory 'wallpaperlivesetup.exe'
if (Test-Path -LiteralPath $output) { throw "Existing installer left untouched: $output. Choose another -OutputDirectory." }
$exe = Join-Path $root 'target\release\wallpaperlive.exe'
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw 'Release executable missing. Run scripts/Build.ps1 first.' }
if (-not (Test-Path -LiteralPath (Join-Path $root 'assets\wallpaperlive.ico') -PathType Leaf)) {
    throw 'Product icon missing: assets/wallpaperlive.ico.'
}

$tools = Join-Path $root 'scratch\tools'
$archive = Join-Path $tools 'tools.innosetup.7.1.0.nupkg'
$packageHash = 'aad15c662593c6f5656457a5c001f4591babd72820cacf63f9706d4af5d93b75'
$compilerHash = 'd06ebd38f38e3cee60a3c50cc45bd449d77e0bc6a5cabc607ea9886808e4de1a'
if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
    New-Item -ItemType Directory -Path $tools -Force | Out-Null
    Invoke-WebRequest -UseBasicParsing -Uri 'https://api.nuget.org/v3-flatcontainer/tools.innosetup/7.1.0/tools.innosetup.7.1.0.nupkg' -OutFile $archive -TimeoutSec 120
}
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $packageHash) { throw "Compiler package checksum mismatch; left untouched: $archive" }
$toolRoot = Join-Path $tools ('inno-7.1.0-build-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $toolRoot | Out-Null
tar.exe -xf $archive -C $toolRoot
if ($LASTEXITCODE -ne 0) { throw "Compiler extraction failed (exit $LASTEXITCODE). Retained: $toolRoot" }
$compiler = Join-Path $toolRoot 'tools\ISCC.exe'
if ((Get-FileHash -LiteralPath $compiler -Algorithm SHA256).Hash -ne $compilerHash) { throw 'Portable compiler checksum mismatch.' }
foreach ($name in 'ISCC.exe', 'ISCmplr.dll', 'ISPP.dll', 'is7zxr-x64.dll') {
    $signature = Get-AuthenticodeSignature -LiteralPath (Join-Path $toolRoot ('tools\' + $name))
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notlike '*O=Pyrsys B.V.*') {
        throw "Untrusted portable compiler component: $name ($($signature.Status))."
    }
}
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
& $compiler --no-ide-signtools "--output-dir=$OutputDirectory" (Join-Path $root 'installer\WallpaperLive.iss')
if ($LASTEXITCODE -ne 0) { throw "Inno Setup compilation failed (exit $LASTEXITCODE)." }
if (-not (Test-Path -LiteralPath $output -PathType Leaf)) { throw 'Compiler produced no wallpaperlivesetup.exe.' }
$artifact = Get-Item -LiteralPath $output
Write-Output "Installer: $output"
Write-Output "Bytes: $($artifact.Length)"
Write-Output "SHA256: $((Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash)"
Write-Output 'Unsigned online installer; no installation or desktop test was performed.'
