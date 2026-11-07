#Requires -Version 5.1
[CmdletBinding()]
param([switch]$DebugBuild)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $root
try {
    $arguments = @('build', '--locked', '--bin', 'wallpaperlive')
    if (-not $DebugBuild) { $arguments += '--release' }
    & cargo @arguments
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)." }
    $profile = if ($DebugBuild) { 'debug' } else { 'release' }
    $exe = Join-Path $root "target\$profile\wallpaperlive.exe"
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
        throw "Build output not found at $exe. This helper expects the default Cargo target directory."
    }
    Write-Output "Built: $exe"
}
finally { Pop-Location }
