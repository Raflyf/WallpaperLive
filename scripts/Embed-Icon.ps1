#Requires -Version 5.1
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Executable, [switch]$VerifyOnly)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
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
    public static void Apply(string file, byte[] ico, bool verifyOnly) {
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
        if (!verifyOnly) {
            IntPtr update = BeginUpdateResourceW(file, false);
            if (update == IntPtr.Zero) throw new Win32Exception();
            try {
                for (int i = 0; i < count; i++)
                    if (!UpdateResourceW(update, new IntPtr(3), new IntPtr(i + 1), 1033, frames[i], (uint)frames[i].Length)) throw new Win32Exception();
                if (!UpdateResourceW(update, new IntPtr(14), new IntPtr(1), 1033, group, (uint)group.Length)) throw new Win32Exception();
                if (!EndUpdateResourceW(update, false)) throw new Win32Exception();
                update = IntPtr.Zero;
            } finally { if (update != IntPtr.Zero) EndUpdateResourceW(update, true); }
        }
        if (!Read(file, 24, 1).SequenceEqual(manifest)) throw new InvalidOperationException("Application manifest changed");
        if (!Read(file, 14, 1).SequenceEqual(group)) throw new InvalidOperationException("Embedded icon group differs");
        for (int i = 0; i < count; i++)
            if (!Read(file, 3, i + 1).SequenceEqual(frames[i])) throw new InvalidOperationException("Embedded icon frame differs");
    }
}}
'@
}
[WallpaperLiveBuild.IconResources]::Apply($Executable, $icon, $VerifyOnly.IsPresent)
Write-Output "Verified executable icon group and all ICO frames; application manifest preserved: $Executable"
