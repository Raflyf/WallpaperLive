#Requires -Version 5.1
[CmdletBinding()]
param([switch]$DebugBuild)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $root
$prevEncodedFlags = $env:CARGO_ENCODED_RUSTFLAGS
try {
    $arguments = @('build', '--locked', '--bin', 'wallpaperlive')
    if (-not $DebugBuild) {
        $arguments += '--release'
        $cargoHome = if ([string]::IsNullOrWhiteSpace($env:CARGO_HOME)) {
            Join-Path $env:USERPROFILE '.cargo'
        } else {
            $env:CARGO_HOME
        }
        $flags = @(
            '-C', 'control-flow-guard=checks',
            "--remap-path-prefix=$cargoHome=/cargo",
            "--remap-path-prefix=$env:USERPROFILE=/user",
            "--remap-path-prefix=$root=/src"
        )
        $env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]0x1f
    }
    & cargo @arguments
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)." }
    $profile = if ($DebugBuild) { 'debug' } else { 'release' }
    $targetDir = Join-Path $root 'target'
    try {
        $meta = (& cargo metadata --format-version 1 --no-deps | ConvertFrom-Json)
        if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($meta.target_directory)) {
            $targetDir = $meta.target_directory
        }
    } catch {}
    $builtExe = Join-Path $targetDir "$profile\wallpaperlive.exe"
    $exe = Join-Path $root "target\$profile\wallpaperlive.exe"
    if (-not (Test-Path -LiteralPath $builtExe -PathType Leaf)) {
        throw "Build output not found at $builtExe."
    }
    if (-not $builtExe.Equals($exe, [StringComparison]::OrdinalIgnoreCase)) {
        [IO.Directory]::CreateDirectory((Split-Path -Parent $exe)) | Out-Null
        [IO.File]::Copy($builtExe, $exe, $true)
    }
    & (Join-Path $PSScriptRoot 'Embed-Icon.ps1') -Executable $exe
    Write-Output "Built: $exe"
}
finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $prevEncodedFlags
    Pop-Location
}
