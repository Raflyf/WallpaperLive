#Requires -Version 5.1
[CmdletBinding()]
param([string]$OutputDirectory, [string]$AppExe)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $PSBoundParameters.ContainsKey('OutputDirectory')) { $OutputDirectory = Join-Path $root 'dist' }
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) { throw 'OutputDirectory cannot be empty.' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)

$cargo = Get-Content -LiteralPath (Join-Path $root 'Cargo.toml') -Raw
if ($cargo -notmatch '(?m)^version\s*=\s*"(\d+\.\d+\.\d+)"') {
    throw 'Installer version in Cargo.toml must be a numeric X.Y.Z version.'
}
$version = $Matches[1]
$output = Join-Path $OutputDirectory "wallpaperlivesetup-v$version.msi"
if (Test-Path -LiteralPath $output) { throw "Existing installer left untouched: $output. Choose another -OutputDirectory." }

$exe = if ($PSBoundParameters.ContainsKey('AppExe')) { (Resolve-Path -LiteralPath $AppExe).ProviderPath } else { Join-Path $root 'target\release\wallpaperlive.exe' }
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw 'Release executable missing. Run scripts/Build.ps1 first.' }
if (-not (Test-Path -LiteralPath (Join-Path $root 'assets\wallpaperlive.ico') -PathType Leaf)) {
    throw 'Product icon missing: assets/wallpaperlive.ico.'
}

function Find-WixBinDir {
    $candidates = @()
    if (-not [string]::IsNullOrWhiteSpace($env:WIX)) {
        $candidates += (Join-Path $env:WIX 'bin')
    }
    if (-not [string]::IsNullOrWhiteSpace(${env:ProgramFiles(x86)})) {
        $candidates += (Join-Path ${env:ProgramFiles(x86)} 'WiX Toolset v3.14\bin')
    }
    foreach ($dir in $candidates) {
        if ((Test-Path -LiteralPath (Join-Path $dir 'candle.exe') -PathType Leaf) -and (Test-Path -LiteralPath (Join-Path $dir 'light.exe') -PathType Leaf)) {
            return $dir
        }
    }
    $cmd = Get-Command candle.exe -CommandType Application -ErrorAction SilentlyContinue
    if ($cmd) {
        $bin = Split-Path -Parent $cmd.Source
        if (Test-Path -LiteralPath (Join-Path $bin 'light.exe') -PathType Leaf) {
            return $bin
        }
    }
    return $null
}

$tools = Join-Path $root 'scratch\tools'
$wixBin = Find-WixBinDir
if (-not $wixBin) {
    $archive = Join-Path $tools 'wix314-binaries.zip'
    $archiveHash = '6AC824E1642D6F7277D0ED7EA09411A508F6116BA6FAE0AA5F2C7DAA2FF43D31'
    $candleHash = 'A464676E2A2191D58E6CA643E89505BFB9628D3E721873A14994D3F4667987E5'
    $lightHash = '8EE3D7FA5592511152A4D1FC605780EAE1B27AC7B41D75D518C958515DE39529'
    if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
        New-Item -ItemType Directory -Path $tools -Force | Out-Null
        Invoke-WebRequest -UseBasicParsing -Uri 'https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip' -OutFile $archive -TimeoutSec 120
    }
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $archiveHash) {
        throw "WiX archive checksum mismatch; left untouched: $archive"
    }
    $wixBin = Join-Path $tools ('wix-3.14-build-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $wixBin | Out-Null
    tar.exe -xf $archive -C $wixBin
    if ($LASTEXITCODE -ne 0) { throw "WiX extraction failed (exit $LASTEXITCODE). Retained: $wixBin" }
    if ((Get-FileHash -LiteralPath (Join-Path $wixBin 'candle.exe') -Algorithm SHA256).Hash -ne $candleHash) {
        throw 'Portable candle.exe checksum mismatch.'
    }
    if ((Get-FileHash -LiteralPath (Join-Path $wixBin 'light.exe') -Algorithm SHA256).Hash -ne $lightHash) {
        throw 'Portable light.exe checksum mismatch.'
    }
}

$candle = Join-Path $wixBin 'candle.exe'
$light = Join-Path $wixBin 'light.exe'

New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$staging = Join-Path $tools ('installer-app-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $staging | Out-Null
$packagedExe = Join-Path $staging 'wallpaperlive.exe'
[IO.File]::Copy($exe, $packagedExe, $false)
& (Join-Path $PSScriptRoot 'Embed-Icon.ps1') -Executable $packagedExe

$noticeText = (Get-Content -LiteralPath (Join-Path $root 'installer\ONLINE-NOTICE.txt') -Raw).Trim()
$licenseText = (Get-Content -LiteralPath (Join-Path $root 'LICENSE') -Raw).Trim()
$combinedText = "$noticeText`r`n`r`n---`r`n`r`n$licenseText"
$escapedRtf = $combinedText.Replace('\', '\\').Replace('{', '\{').Replace('}', '\}').Replace("`r`n", "`n").Replace("`n", "\par`r`n")
$licenseRtf = Join-Path $staging 'license.rtf'
[IO.File]::WriteAllText($licenseRtf, "{\rtf1\ansi\deff0{\fonttbl{\f0 Segoe UI;}}\fs18 $escapedRtf}", [Text.Encoding]::ASCII)

$wixObj = Join-Path $staging 'WallpaperLive.wixobj'
$wxs = Join-Path $root 'installer\WallpaperLive.wxs'

& $candle -nologo -arch x64 -ext WixUIExtension -ext WixUtilExtension "-dAppVersion=$version" "-dAppExe=$packagedExe" "-dRepoRoot=$root" "-dLicenseRtf=$licenseRtf" -out $wixObj $wxs
if ($LASTEXITCODE -ne 0) { throw "WiX candle compilation failed (exit $LASTEXITCODE)." }

& $light -nologo -ext WixUIExtension -ext WixUtilExtension -out $output $wixObj
if ($LASTEXITCODE -ne 0) { throw "WiX light linking failed (exit $LASTEXITCODE)." }

if (-not (Test-Path -LiteralPath $output -PathType Leaf)) { throw "Compiler produced no installer: $output" }

$msiInstaller = New-Object -ComObject WindowsInstaller.Installer
$db = $msiInstaller.OpenDatabase($output, 0)
function Get-MsiProperty([string]$prop) {
    $view = $db.OpenView("SELECT Value FROM Property WHERE Property='$prop'")
    $view.Execute()
    $rec = $view.Fetch()
    $val = if ($rec) { $rec.StringData(1) } else { '' }
    $view.Close()
    return $val
}
$msiVersion = (Get-MsiProperty 'ProductVersion').Trim()
$msiName = (Get-MsiProperty 'ProductName').Trim()
$msiManufacturer = (Get-MsiProperty 'Manufacturer').Trim()
[void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($db)
[void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($msiInstaller)

if ($msiVersion -ne $version -or $msiName -ne 'WallpaperLive' -or $msiManufacturer -ne 'Raflyf') {
    throw "Installer MSI metadata mismatch: Name=$msiName, Manufacturer=$msiManufacturer, Version=$msiVersion, expected=$version."
}

$artifact = Get-Item -LiteralPath $output
Write-Output "MSI Product: $msiName ($msiManufacturer); ProductVersion: $msiVersion"
Write-Output "Installer: $output"
Write-Output "Bytes: $($artifact.Length)"
Write-Output "SHA256: $((Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash)"
Write-Output 'Unsigned MSI installer; no installation or desktop test was performed.'
