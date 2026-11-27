#Requires -Version 5.1
[CmdletBinding()]
param([string]$OutputDirectory, [string]$AppExe)

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
        [long]$MaxBytes = 104857600
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
        $request.UserAgent = 'WallpaperLive-InstallerBuild'
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

$root = Split-Path -Parent $PSScriptRoot
if (-not $PSBoundParameters.ContainsKey('OutputDirectory')) { $OutputDirectory = Join-Path $root 'dist' }
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) { throw 'OutputDirectory cannot be empty.' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
Assert-NoReparsePoint -Path $OutputDirectory

$cargo = Get-Content -LiteralPath (Join-Path $root 'Cargo.toml') -Raw
if ($cargo -notmatch '(?m)^version\s*=\s*"(\d+\.\d+\.\d+)"') {
    throw 'Installer version in Cargo.toml must be a numeric X.Y.Z version.'
}
$version = $Matches[1]
$output = Join-Path $OutputDirectory "wallpaperlivesetup-v$version.msi"
if (Test-Path -LiteralPath $output) { throw "Existing installer left untouched: $output. Choose another -OutputDirectory." }

$exe = if ($PSBoundParameters.ContainsKey('AppExe')) { (Resolve-Path -LiteralPath $AppExe).ProviderPath } else { Join-Path $root 'target\release\wallpaperlive.exe' }
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw 'Release executable missing. Run scripts/Build.ps1 first.' }
Assert-NoReparsePoint -Path $exe
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
    $tarExe = Join-Path $env:SystemRoot 'System32\tar.exe'
    if (-not (Test-Path -LiteralPath $tarExe -PathType Leaf)) {
        throw "System tar.exe not found at $tarExe"
    }
    Assert-NoReparsePoint -Path $tarExe
    Assert-NoReparsePoint -Path $tools
    $archive = Join-Path $tools 'wix314-binaries.zip'
    $archiveHash = '6AC824E1642D6F7277D0ED7EA09411A508F6116BA6FAE0AA5F2C7DAA2FF43D31'
    $candleHash = 'A464676E2A2191D58E6CA643E89505BFB9628D3E721873A14994D3F4667987E5'
    $lightHash = '8EE3D7FA5592511152A4D1FC605780EAE1B27AC7B41D75D518C958515DE39529'
    if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
        [IO.Directory]::CreateDirectory($tools) | Out-Null
        $archivePart = Join-Path $tools ('.wix314-' + (New-RandomHex) + '.part')
        try {
            Invoke-PinnedHttpsDownload `
                -Uri 'https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip' `
                -OutFile $archivePart `
                -AllowedHosts @('github.com', 'objects.githubusercontent.com', 'release-assets.githubusercontent.com') `
                -MaxBytes 104857600
            if ((Get-FileHash -LiteralPath $archivePart -Algorithm SHA256).Hash -ne $archiveHash) {
                throw 'WiX archive checksum mismatch; aborted.'
            }
            [IO.File]::Move($archivePart, $archive)
        } finally {
            if (Test-Path -LiteralPath $archivePart -PathType Leaf) {
                try { [IO.File]::Delete($archivePart) } catch {}
            }
        }
    }
    Assert-NoReparsePoint -Path $archive
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $archiveHash) {
        throw "WiX archive checksum mismatch; left untouched: $archive"
    }
    $wixBin = Join-Path $tools ('wix-3.14-build-' + (New-RandomHex))
    [IO.Directory]::CreateDirectory($wixBin) | Out-Null
    & $tarExe -xf $archive -C $wixBin
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

[IO.Directory]::CreateDirectory($OutputDirectory) | Out-Null
[IO.Directory]::CreateDirectory($tools) | Out-Null
Assert-NoReparsePoint -Path $tools
$staging = Join-Path $tools ('installer-app-' + (New-RandomHex))
[IO.Directory]::CreateDirectory($staging) | Out-Null
$packagedExe = Join-Path $staging 'wallpaperlive.exe'
$licenseRtf = Join-Path $staging 'license.rtf'
$wixObj = Join-Path $staging 'WallpaperLive.wixobj'

try {
    [IO.File]::Copy($exe, $packagedExe, $false)
    & (Join-Path $PSScriptRoot 'Embed-Icon.ps1') -Executable $packagedExe

    $noticeText = (Get-Content -LiteralPath (Join-Path $root 'installer\ONLINE-NOTICE.txt') -Raw).Trim()
    $licenseText = (Get-Content -LiteralPath (Join-Path $root 'LICENSE') -Raw).Trim()
    $combinedText = "$noticeText`r`n`r`n---`r`n`r`n$licenseText"
    $escapedRtf = $combinedText.Replace('\', '\\').Replace('{', '\{').Replace('}', '\}').Replace("`r`n", "`n").Replace("`n", "\par`r`n")
    [IO.File]::WriteAllText($licenseRtf, "{\rtf1\ansi\deff0{\fonttbl{\f0 Segoe UI;}}\fs18 $escapedRtf}", [Text.Encoding]::ASCII)

    $wxs = Join-Path $root 'installer\WallpaperLive.wxs'

    & $candle -nologo -arch x64 -ext WixUIExtension -ext WixUtilExtension "-dAppVersion=$version" "-dAppExe=$packagedExe" "-dRepoRoot=$root" "-dLicenseRtf=$licenseRtf" -out $wixObj $wxs
    if ($LASTEXITCODE -ne 0) { throw "WiX candle compilation failed (exit $LASTEXITCODE)." }

    & $light -nologo -ext WixUIExtension -ext WixUtilExtension -out $output $wixObj
    if ($LASTEXITCODE -ne 0) { throw "WiX light linking failed (exit $LASTEXITCODE)." }
} finally {
    foreach ($tempFile in @($wixObj, $licenseRtf, $packagedExe)) {
        if (Test-Path -LiteralPath $tempFile -PathType Leaf) {
            try { [IO.File]::Delete($tempFile) } catch {}
        }
    }
    if (Test-Path -LiteralPath $staging -PathType Container) {
        try { [IO.Directory]::Delete($staging, $false) } catch {}
    }
}

if (-not (Test-Path -LiteralPath $output -PathType Leaf)) { throw "Compiler produced no installer: $output" }

if (-not ([Management.Automation.PSTypeName]'WallpaperLiveMsiReader').Type) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class WallpaperLiveMsiReader {
    [DllImport("msi.dll", CharSet = CharSet.Unicode)]
    static extern uint MsiOpenDatabase(string szDatabasePath, IntPtr szPersist, out IntPtr phDatabase);
    [DllImport("msi.dll", CharSet = CharSet.Unicode)]
    static extern uint MsiDatabaseOpenView(IntPtr hDatabase, string szQuery, out IntPtr phView);
    [DllImport("msi.dll")]
    static extern uint MsiViewExecute(IntPtr hView, IntPtr hRecord);
    [DllImport("msi.dll")]
    static extern uint MsiViewFetch(IntPtr hView, out IntPtr phRecord);
    [DllImport("msi.dll", CharSet = CharSet.Unicode)]
    static extern uint MsiRecordGetString(IntPtr hRecord, uint iField, StringBuilder szValueBuf, ref uint pcchValueBuf);
    [DllImport("msi.dll")]
    static extern uint MsiCloseHandle(IntPtr hAny);

    public static string GetProperty(string msiPath, string name) {
        IntPtr db = IntPtr.Zero, view = IntPtr.Zero, rec = IntPtr.Zero;
        try {
            if (MsiOpenDatabase(msiPath, IntPtr.Zero, out db) != 0) throw new Exception("MsiOpenDatabase failed.");
            if (MsiDatabaseOpenView(db, "SELECT `Value` FROM `Property` WHERE `Property` = '" + name + "'", out view) != 0) throw new Exception("MsiDatabaseOpenView failed.");
            if (MsiViewExecute(view, IntPtr.Zero) != 0) throw new Exception("MsiViewExecute failed.");
            if (MsiViewFetch(view, out rec) != 0 || rec == IntPtr.Zero) return "";
            uint len = 512;
            var sb = new StringBuilder((int)len);
            if (MsiRecordGetString(rec, 1, sb, ref len) != 0) return "";
            return sb.ToString();
        } finally {
            if (rec != IntPtr.Zero) MsiCloseHandle(rec);
            if (view != IntPtr.Zero) MsiCloseHandle(view);
            if (db != IntPtr.Zero) MsiCloseHandle(db);
        }
    }
}
'@
}
$msiVersion = ([WallpaperLiveMsiReader]::GetProperty($output, 'ProductVersion')).Trim()
$msiName = ([WallpaperLiveMsiReader]::GetProperty($output, 'ProductName')).Trim()
$msiManufacturer = ([WallpaperLiveMsiReader]::GetProperty($output, 'Manufacturer')).Trim()
if ($msiVersion -ne $version -or $msiName -ne 'WallpaperLive' -or $msiManufacturer -ne 'Raflyf') {
    throw "Installer MSI metadata mismatch: Name=$msiName, Manufacturer=$msiManufacturer, Version=$msiVersion, expected=$version."
}

$artifact = Get-Item -LiteralPath $output
Write-Output "MSI Product: $msiName ($msiManufacturer); ProductVersion: $msiVersion"
Write-Output "Installer: $output"
Write-Output "Bytes: $($artifact.Length)"
Write-Output "SHA256: $((Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash)"
Write-Output 'Unsigned MSI installer; no installation or desktop test was performed.'
