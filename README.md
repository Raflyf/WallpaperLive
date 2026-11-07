<p align="center">
  <img src="assets/wallpaperlive.png" width="112" height="112" alt="WallpaperLive logo">
</p>

<h1 align="center">WallpaperLive</h1>

<p align="center">Videos, GIFs and images as wallpaper, with a native Windows interface.</p>

<p align="center">
  <a href="https://github.com/Raflyf/WallpaperLive/actions/workflows/ci.yml"><img src="https://github.com/Raflyf/WallpaperLive/actions/workflows/ci.yml/badge.svg" alt="Windows checks"></a>
  <a href="https://github.com/Raflyf/WallpaperLive/releases/tag/v0.1.1"><img src="https://img.shields.io/badge/release-v0.1.1-586575" alt="Release v0.1.1"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-586575" alt="GPL-3.0-or-later license"></a>
</p>

<p align="center">
  <a href="https://github.com/Raflyf/WallpaperLive/releases/download/v0.1.1/wallpaperlivesetup.exe">Download Windows installer</a>
  &nbsp;·&nbsp;
  <a href="https://github.com/Raflyf/WallpaperLive/releases">All releases</a>
  &nbsp;·&nbsp;
  <a href="https://github.com/Raflyf/WallpaperLive/issues">Report a bug</a>
</p>

WallpaperLive uses Rust and Win32 for its English interface, desktop integration and tray controls. The thumbnail library lets you preview a file before applying it. libmpv handles media decoding; the app does not require a browser engine or an account.

> **Release v0.1.1:** unsigned. Native controls and source regressions have been checked; installation and live desktop playback still need your testing. A release tag does not establish compatibility with every Windows build, GPU or media file. Total RAM is not capped.

## Features

- Import a local folder, including subfolders, or add individual files. Browse up to 4096 wallpapers; four thumbnails are loaded per page. Originals stay in their original locations.
- Filter the library by folder; imported folder roots remain available after restart. **Select all** selects every file in the current filter across all pages; Ctrl-click toggles individual files. **Remove (N)** confirms the count and removes library entries, never original media.
- Automatically cache oversized SDR videos at the selected display's pixel resolution. Choose a **24, 30, 45 or 60 FPS** cap in Settings (30 default); smaller sources are never upscaled.
- Prefer a detected dedicated GPU for D3D11 rendering and compatible hardware decoding; fall back to the default adapter when needed.
- Apply one active wallpaper to one display you choose.
- Choose **Fill** to crop to the screen or **Fit** to show the full frame.
- Loop playback with sound off by default; toggle **Sound: On/Off** without reloading the wallpaper.
- Pause, resume and stop from the panel or tray. The tray menu follows the Windows system dark/light theme on each open, without restarting; high-contrast mode uses the native system menu. Hovering the tray icon does not open the panel.
- Remove a wallpaper from the library without deleting its original media file.
- Resize or maximize the native window; library navigation stays below the thumbnails.
- Pause for fullscreen or maximized apps on the wallpaper display, battery power, inactive sessions, display sleep and system suspend. Pause releases the decoder and GPU surface; resume reloads the cached wallpaper near its previous position. Video preparation is deferred/cancelled while paused. Fullscreen/maximize and battery pause are configurable; manual pause remains in effect until you clear it.
- Check GitHub once in the background at each app start. A newer stable release shows a tray balloon and **Pembaruan tersedia (vX.Y.Z)**; clicking opens its release page. No automatic download or installation.

## Requirements

- Windows 10 or Windows 11, x64.
- Trusted local video, GIF or image files. The download does not include wallpapers.
- A native libmpv decoder. Setup can reuse a matching installed decoder or the pinned local archive; otherwise first-time setup asks for Internet access and download consent.

Native media parsers run with your user privileges. WallpaperLive is not a sandbox: malformed files can crash the app. Use media you trust.

## Install

1. [Download the installer](https://github.com/Raflyf/WallpaperLive/releases/download/v0.1.1/wallpaperlivesetup.exe) and run it as your normal Windows user.
2. If a decoder download is needed, review the prompt and grant consent to proceed. Setup skips that prompt when it can verify an existing decoder or a local archive.
3. Open WallpaperLive from the Start menu after installation.

Setup installs per user without administrator rights. It asks before downloading libmpv and verifies SHA256 before installing it. Setup does not launch the app, change your wallpaper or enable startup.

For an offline upgrade, install into the same folder: Setup reuses a matching decoder. For a first install without an in-setup download, obtain the [pinned upstream archive](https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261107/mpv-dev-x86_64-20261107-git-9028187538.7z) beforehand, rename it `mpv-dev.7z` and place it beside the installer. Both archive and DLL checksums are verified. The single installer does not bundle the decoder; its complete dependency-source record has not been verified for redistribution.

Windows may show a publisher warning because this release is unsigned. Keep the installer in a dedicated folder without unrelated DLL files. Do not disable security protections or run the app as administrator to work around desktop access errors.

## Set a wallpaper

1. Choose **Add wallpaper**, **Add folder**, or drop a local media file into the window.
2. Select its thumbnail, choose a display and select **Fill** or **Fit**. Settings offers **24, 30, 45 or 60 FPS** (cap, not interpolation).
3. Choose **Set wallpaper**. Oversized/high-FPS SDR videos are prepared once in the background; **Stop** cancels preparation. Originals are never modified.

Selecting a thumbnail changes the preview, not the active wallpaper. Use **Pause** or **Resume** to control playback. **Stop** removes the live wallpaper without changing your Windows static wallpaper settings. **Sound: On/Off** selects or disables the media's audio track; files without audio remain silent. **Remove** asks for confirmation and removes only the library entry; removing an active wallpaper also stops it.

Closing the window keeps playback in the tray when the tray is available. Choose **Exit** in the app or tray menu to close WallpaperLive. Media/display preferences are saved, and **Start with Windows** remains opt-in in Settings.

On each new application process, one background HTTPS request checks the latest stable release on GitHub. It sends the application version in its User-Agent, not wallpaper paths or preferences. No polling or automatic retry runs. If offline, rate-limited or unable to validate the response, the tray label says the check failed; clicking it still opens this repository's releases page. No download or installation starts without your action. Windows notification settings or Focus Assist may hide a balloon; the menu remains available.

## Media formats

The file picker accepts:

`MP4` `GIF` `MKV` `WebM` `MOV` `AVI` `MPG` `MPEG` `M4V` `APNG` `PNG` `JPG` `JPEG` `BMP` `WebP`

Playback depends on the file's codec and the installed decoder. An accepted extension does not guarantee support for every codec or animated-image variant. WallpaperLive does not accept URLs, network paths, playlists, executable files or web wallpapers.

Hardware decoding depends on the codec and GPU. A detected hardware adapter with dedicated VRAM is preferred through D3D11; this also selects the device for compatible D3D11VA decoding. Driver/codec failure may fall back. Dedicated GPU use can increase power consumption and does not eliminate system RAM allocations.

Automatic cache conversion uses the installed libmpv encoder, not a separately downloaded FFmpeg program. It preserves the original file, audio, aspect ratio and playback speed. Cache copies are H.264/AAC MP4, keyed by source SHA256, display dimensions and FPS cap. The cap drops extra frames; it does not invent frames in slower sources. HDR and image/animated-image inputs remain original in this release, with an explicit status note; no silent 8-bit HDR conversion. Conversion requires finite video up to one hour/16 GiB, runs one below-normal-priority worker, and stops at 15 minutes or 512 MiB per output. Total cache is limited to 2 GiB; a full cache reports an error rather than deleting original or cached files automatically. Cancelled partials are never used. Cache is in `%LOCALAPPDATA%\\WallpaperLive\\video-cache`; move unused cache files aside to free space.

Pause releases the decoder, rendering surface and thumbnail buffers; resume reloads the wallpaper near its saved position. Cache conversion is cancelled/deferred when pause conditions apply, so it does not keep competing with a fullscreen game. RAM/VRAM should fall after decoder teardown, but Windows/driver reclamation is not instantaneous. The native panel, tray and 500 ms resume-state monitor remain: usage is low, not zero. A cancelled conversion restarts when resumed if no finalized copy exists.

Thumbnail extraction runs one at a time in a short-lived child; the panel retains only four visible thumbnails plus the selection, each at most 480 × 270 pixels, and drops them when hidden, minimized or paused. Decoder packet budgets remain 8 MiB forward, no backward cache, 0.5 seconds readahead and four software decode threads. Total RAM is not capped. Native libraries and GPU drivers allocate additional memory; reported improvements are fixture-specific, not a guarantee for every device.

One owned-fixture comparison on an RTX 3050 Laptop GPU used the same 1920 × 1080 native test window and D3D11VA decoding for 15 seconds per condition. Original 4K/60 FPS video used median **275.5 MiB working set / 856.3 MiB private bytes**; its 1080p/30 FPS cache used **187.1 MiB / 355.1 MiB**. Process CPU fell from **3.79% to 1.84%** of total machine capacity. After decoder destruction, working sets were about **57 MiB**, including the Python measurement host but excluding the app panel. This was one sequential pair, not a repeatability guarantee; VRAM and GPU utilization were not measured.

Desktop changes, HDR, virtual desktops and display reconnection can affect wallpaper attachment. If Windows blocks access to Explorer, leave playback stopped. When reporting a problem, include the app version, Windows build, GPU, media format and steps to reproduce.

## Build from source

Install a Rust toolchain with its Windows linker prerequisites. Build the app and prepare its decoder:

```powershell
cargo build --locked --release --bin wallpaperlive
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\Get-Decoder.ps1 -Destination .\target\release\runtime
```

[Get-Decoder.ps1](scripts/Get-Decoder.ps1) performs an explicit, checksum-verified decoder download. Release builds look for the decoder beside the executable, not in your media folder or on PATH.

Run source checks:

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

To create the installer, run [Installer.ps1](scripts/Installer.ps1) after the release build:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\Installer.ps1
```

The script downloads and verifies a portable Inno Setup compiler when needed. It does not install tools globally.

[Windows checks](.github/workflows/ci.yml) builds the application and `wallpaperlivesetup.exe` on every push, pull request and manual run. Download the installer and its SHA256 manifest from the run's **wallpaperlive-windows-x64** artifact. A pushed version tag beginning with `v` publishes those same verified files to GitHub Releases after all checks pass; tags containing `-` are prereleases. Builds remain unsigned. CI does not install the app or run desktop playback tests.

## License

WallpaperLive is licensed under **GPL-3.0-or-later**. See [LICENSE](LICENSE) for the license and [THIRD_PARTY.md](THIRD_PARTY.md) for dependency notices. libmpv is a separate native decoder, not Rust code or bundled wallpaper content.
