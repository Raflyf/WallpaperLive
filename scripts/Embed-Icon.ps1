#Requires -Version 5.1
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Executable, [switch]$VerifyOnly)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$cargo = Get-Content -LiteralPath (Join-Path $root 'Cargo.toml') -Raw
if ($cargo -notmatch '(?m)^version\s*=\s*"(\d+)\.(\d+)\.(\d+)"') {
    throw 'Could not determine numeric X.Y.Z version from Cargo.toml.'
}
$verMajor = [int]$Matches[1]
$verMinor = [int]$Matches[2]
$verPatch = [int]$Matches[3]
$Executable = (Resolve-Path -LiteralPath $Executable).ProviderPath
$entry = Get-Item -LiteralPath $Executable
if ($entry.PSIsContainer) { throw 'Executable must be a regular file.' }
while ($entry) {
    if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw 'Icon embedding refuses reparse points in the executable path.'
    }
    $entry = if ($entry -is [IO.FileInfo]) { $entry.Directory } else { $entry.Parent }
}
$allowed = @('target', 'scratch') | ForEach-Object { (Join-Path $root $_) + [IO.Path]::DirectorySeparatorChar }
if (-not ($allowed | Where-Object { $Executable.StartsWith($_, [StringComparison]::OrdinalIgnoreCase) })) {
    throw 'Icon embedding accepts only owned target or scratch build outputs, never an installed application.'
}
if ((Get-AuthenticodeSignature -LiteralPath $Executable).Status -ne 'NotSigned') {
    throw 'Icon embedding refuses signed executables.'
}
$icon = [IO.File]::ReadAllBytes((Join-Path $root 'assets\wallpaperlive.ico'))
if (-not ('WallpaperLiveBuild.IconResources' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Linq;
using System.Runtime.InteropServices;
namespace WallpaperLiveBuild {
public static class IconResources {
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern IntPtr BeginUpdateResourceW(string file, bool deleteExisting);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool UpdateResourceW(IntPtr update, IntPtr type, IntPtr name, ushort language, byte[] data, uint size);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool EndUpdateResourceW(IntPtr update, bool discard);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern IntPtr LoadLibraryExW(string file, IntPtr reserved, uint flags);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern IntPtr FindResourceExW(IntPtr module, IntPtr type, IntPtr name, ushort language);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern IntPtr LoadResource(IntPtr module, IntPtr resource);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern IntPtr LockResource(IntPtr resource);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern uint SizeofResource(IntPtr module, IntPtr resource);
    [DllImport("kernel32.dll")]
    static extern bool FreeLibrary(IntPtr module);

    static byte[] Read(string file, int type, int id) {
        // LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE: no target code executes.
        IntPtr module = LoadLibraryExW(file, IntPtr.Zero, 0x22);
        if (module == IntPtr.Zero) throw new Win32Exception();
        try {
            IntPtr resource = FindResourceExW(module, new IntPtr(type), new IntPtr(id), 1033);
            if (resource == IntPtr.Zero) throw new Win32Exception();
            uint size = SizeofResource(module, resource);
            if (size == 0 || size > 4194304) throw new InvalidOperationException("Invalid resource size");
            IntPtr loaded = LoadResource(module, resource);
            if (loaded == IntPtr.Zero) throw new Win32Exception();
            IntPtr bytes = LockResource(loaded);
            if (bytes == IntPtr.Zero) throw new Win32Exception();
            byte[] data = new byte[size];
            Marshal.Copy(bytes, data, 0, data.Length);
            return data;
        } finally { FreeLibrary(module); }
    }
    static byte[] Align4(System.IO.MemoryStream ms) {
        while ((ms.Position & 3) != 0) ms.WriteByte(0);
        return null;
    }
    static void PatchLength(System.IO.MemoryStream ms, long start) {
        long end = ms.Position;
        ms.Position = start;
        byte[] len = BitConverter.GetBytes((ushort)(end - start));
        ms.Write(len, 0, 2);
        ms.Position = end;
    }
    static byte[] WriteStringEntry(string key, string value) {
        using (var ms = new System.IO.MemoryStream())
        using (var bw = new System.IO.BinaryWriter(ms, System.Text.Encoding.Unicode, true)) {
            bw.Write((ushort)0);
            bw.Write((ushort)(value.Length + 1));
            bw.Write((ushort)1);
            bw.Write(System.Text.Encoding.Unicode.GetBytes(key + "\0"));
            Align4(ms);
            bw.Write(System.Text.Encoding.Unicode.GetBytes(value + "\0"));
            PatchLength(ms, 0);
            return ms.ToArray();
        }
    }
    public static byte[] BuildVersionResource(int major, int minor, int patch) {
        string productVer = string.Format("{0}.{1}.{2}", major, minor, patch);
        string fileVer = string.Format("{0}.{1}.{2}.0", major, minor, patch);
        var entries = new[] {
            new[] { "CompanyName", "Raflyf" },
            new[] { "FileDescription", "WallpaperLive" },
            new[] { "FileVersion", fileVer },
            new[] { "InternalName", "wallpaperlive" },
            new[] { "LegalCopyright", "GPL-3.0-or-later" },
            new[] { "OriginalFilename", "wallpaperlive.exe" },
            new[] { "ProductName", "WallpaperLive" },
            new[] { "ProductVersion", productVer },
        };
        byte[] tableBytes;
        using (var ms = new System.IO.MemoryStream())
        using (var bw = new System.IO.BinaryWriter(ms, System.Text.Encoding.Unicode, true)) {
            bw.Write((ushort)0);
            bw.Write((ushort)0);
            bw.Write((ushort)1);
            bw.Write(System.Text.Encoding.Unicode.GetBytes("040904b0\0"));
            Align4(ms);
            for (int i = 0; i < entries.Length; i++) {
                if (i > 0) Align4(ms);
                bw.Write(WriteStringEntry(entries[i][0], entries[i][1]));
            }
            PatchLength(ms, 0);
            tableBytes = ms.ToArray();
        }
        byte[] stringFileInfo;
        using (var ms = new System.IO.MemoryStream())
        using (var bw = new System.IO.BinaryWriter(ms, System.Text.Encoding.Unicode, true)) {
            bw.Write((ushort)0);
            bw.Write((ushort)0);
            bw.Write((ushort)1);
            bw.Write(System.Text.Encoding.Unicode.GetBytes("StringFileInfo\0"));
            Align4(ms);
            bw.Write(tableBytes);
            PatchLength(ms, 0);
            stringFileInfo = ms.ToArray();
        }
        byte[] varEntry;
        using (var ms = new System.IO.MemoryStream())
        using (var bw = new System.IO.BinaryWriter(ms, System.Text.Encoding.Unicode, true)) {
            bw.Write((ushort)0);
            bw.Write((ushort)4);
            bw.Write((ushort)0);
            bw.Write(System.Text.Encoding.Unicode.GetBytes("Translation\0"));
            Align4(ms);
            bw.Write((ushort)0x0409);
            bw.Write((ushort)0x04B0);
            PatchLength(ms, 0);
            varEntry = ms.ToArray();
        }
        byte[] varFileInfo;
        using (var ms = new System.IO.MemoryStream())
        using (var bw = new System.IO.BinaryWriter(ms, System.Text.Encoding.Unicode, true)) {
            bw.Write((ushort)0);
            bw.Write((ushort)0);
            bw.Write((ushort)1);
            bw.Write(System.Text.Encoding.Unicode.GetBytes("VarFileInfo\0"));
            Align4(ms);
            bw.Write(varEntry);
            PatchLength(ms, 0);
            varFileInfo = ms.ToArray();
        }
        using (var ms = new System.IO.MemoryStream())
        using (var bw = new System.IO.BinaryWriter(ms, System.Text.Encoding.Unicode, true)) {
            bw.Write((ushort)0);
            bw.Write((ushort)52);
            bw.Write((ushort)0);
            bw.Write(System.Text.Encoding.Unicode.GetBytes("VS_VERSION_INFO\0"));
            Align4(ms);
            uint msVer = ((uint)major << 16) | (uint)minor;
            uint lsVer = (uint)patch << 16;
            bw.Write(0xFEEF04BDu);
            bw.Write(0x00010000u);
            bw.Write(msVer);
            bw.Write(lsVer);
            bw.Write(msVer);
            bw.Write(lsVer);
            bw.Write(0x0000003Fu);
            bw.Write(0u);
            bw.Write(0x00040004u);
            bw.Write(0x00000001u);
            bw.Write(0u);
            bw.Write(0u);
            bw.Write(0u);
            Align4(ms);
            bw.Write(stringFileInfo);
            Align4(ms);
            bw.Write(varFileInfo);
            PatchLength(ms, 0);
            return ms.ToArray();
        }
    }
    static byte[] TryRead(string file, int type, int id) {
        try { return Read(file, type, id); } catch { return null; }
    }
    static bool GroupMatches(byte[] actual, byte[] expected, int count) {
        if (actual == null || actual.Length != expected.Length) return false;
        for (int i = 0; i < 6; i++) if (actual[i] != expected[i]) return false;
        for (int i = 0; i < count; i++) {
            int o = 6 + i * 14;
            // Width, height, color count, reserved (0..3), bit count (6..7), bytesInRes (8..11), nID (12..13) must match exactly.
            // rc.exe normalizes wPlanes=0 in ICO headers to wPlanes=1 in RT_GROUP_ICON.
            for (int j = 0; j < 4; j++) if (actual[o + j] != expected[o + j]) return false;
            ushort actPlanes = BitConverter.ToUInt16(actual, o + 4);
            ushort expPlanes = BitConverter.ToUInt16(expected, o + 4);
            if (actPlanes != expPlanes && !(expPlanes == 0 && actPlanes == 1)) return false;
            for (int j = 6; j < 14; j++) if (actual[o + j] != expected[o + j]) return false;
        }
        return true;
    }
    static bool AlreadyMatches(string file, byte[] group, byte[][] frames) {
        byte[] existingGroup = TryRead(file, 14, 1);
        if (!GroupMatches(existingGroup, group, frames.Length)) return false;
        for (int i = 0; i < frames.Length; i++) {
            byte[] existingFrame = TryRead(file, 3, i + 1);
            if (existingFrame == null || !existingFrame.SequenceEqual(frames[i])) return false;
        }
        return TryRead(file, 16, 1) != null;
    }
    public static void Apply(string file, byte[] ico, byte[] versionInfo, bool verifyOnly) {
        if (ico.Length < 6 || ico.Length > 4194304 || BitConverter.ToUInt16(ico, 0) != 0 || BitConverter.ToUInt16(ico, 2) != 1)
            throw new InvalidOperationException("Invalid ICO header");
        int count = BitConverter.ToUInt16(ico, 4);
        if (count < 1 || count > 256 || ico.Length < 6 + count * 16)
            throw new InvalidOperationException("Invalid ICO directory");
        byte[] group = new byte[6 + count * 14];
        Buffer.BlockCopy(ico, 0, group, 0, 6);
        byte[][] frames = new byte[count][];
        for (int i = 0; i < count; i++) {
            int entry = 6 + i * 16;
            uint size = BitConverter.ToUInt32(ico, entry + 8), offset = BitConverter.ToUInt32(ico, entry + 12);
            if (ico[entry + 3] != 0 || size == 0 || offset < 6 + count * 16 || (ulong)offset + size > (ulong)ico.Length)
                throw new InvalidOperationException("Invalid ICO frame range");
            frames[i] = new byte[size];
            Buffer.BlockCopy(ico, (int)offset, frames[i], 0, (int)size);
            Buffer.BlockCopy(ico, entry, group, 6 + i * 14, 12);
            Buffer.BlockCopy(BitConverter.GetBytes((ushort)(i + 1)), 0, group, 18 + i * 14, 2);
        }
        byte[] manifest = Read(file, 24, 1);
        bool embeddedAtLinkTime = AlreadyMatches(file, group, frames);
        if (!verifyOnly && !embeddedAtLinkTime) {
            IntPtr update = BeginUpdateResourceW(file, false);
            if (update == IntPtr.Zero) throw new Win32Exception();
            try {
                for (int i = 0; i < count; i++)
                    if (!UpdateResourceW(update, new IntPtr(3), new IntPtr(i + 1), 1033, frames[i], (uint)frames[i].Length)) throw new Win32Exception();
                if (!UpdateResourceW(update, new IntPtr(14), new IntPtr(1), 1033, group, (uint)group.Length)) throw new Win32Exception();
                if (!UpdateResourceW(update, new IntPtr(16), new IntPtr(1), 1033, versionInfo, (uint)versionInfo.Length)) throw new Win32Exception();
                if (!EndUpdateResourceW(update, false)) throw new Win32Exception();
                update = IntPtr.Zero;
            } finally { if (update != IntPtr.Zero) EndUpdateResourceW(update, true); }
        }
        if (!Read(file, 24, 1).SequenceEqual(manifest)) throw new InvalidOperationException("Application manifest changed");
        if (!GroupMatches(Read(file, 14, 1), group, count)) throw new InvalidOperationException("Embedded icon group differs");
        if (!embeddedAtLinkTime && !Read(file, 16, 1).SequenceEqual(versionInfo)) throw new InvalidOperationException("Embedded version resource differs");
        if (Read(file, 16, 1).Length == 0) throw new InvalidOperationException("Embedded version resource missing");
        for (int i = 0; i < count; i++)
            if (!Read(file, 3, i + 1).SequenceEqual(frames[i])) throw new InvalidOperationException("Embedded icon frame differs");
    }
}}
'@
}
$versionBytes = [WallpaperLiveBuild.IconResources]::BuildVersionResource($verMajor, $verMinor, $verPatch)
[WallpaperLiveBuild.IconResources]::Apply($Executable, $icon, $versionBytes, $VerifyOnly.IsPresent)
$info = [Diagnostics.FileVersionInfo]::GetVersionInfo($Executable)
$expectedProduct = "$verMajor.$verMinor.$verPatch"
$expectedFile = "$expectedProduct.0"
if ($info.ProductName.Trim() -ne 'WallpaperLive' -or $info.ProductVersion.Trim() -ne $expectedProduct -or $info.FileVersion.Trim() -ne $expectedFile) {
    throw "Executable version metadata mismatch: ProductName=$($info.ProductName), ProductVersion=$($info.ProductVersion), FileVersion=$($info.FileVersion)."
}
Write-Output "Verified executable icon group, all ICO frames, and version metadata ($expectedProduct); application manifest preserved: $Executable"
