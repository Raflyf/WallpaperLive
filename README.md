<p align="center">
  <img src="assets/wallpaperlive.png" width="112" height="112" alt="WallpaperLive logo">
</p>
<h1 align="center">WallpaperLive</h1>
<p align="center">Videos, GIFs and images as wallpaper, with a native Windows interface.</p>

<p align="center">
  <a href="https://github.com/Raflyf/WallpaperLive/actions/workflows/ci.yml"><img src="https://github.com/Raflyf/WallpaperLive/actions/workflows/ci.yml/badge.svg" alt="Windows checks"></a>
  <a href="https://github.com/Raflyf/WallpaperLive/releases/tag/v0.1.3"><img src="https://img.shields.io/badge/release-v0.1.3-586575" alt="Release v0.1.3"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-586575" alt="GPL-3.0-or-later license"></a>
</p>

<p align="center">
  <a href="https://github.com/Raflyf/WallpaperLive/releases/download/v0.1.3/wallpaperlivesetup.exe">Download Windows installer</a>
  &nbsp;·&nbsp;
  <a href="https://github.com/Raflyf/WallpaperLive/releases">All releases</a>
  &nbsp;·&nbsp;
  <a href="CHANGELOG.md">Changelog</a>
  &nbsp;·&nbsp;
  <a href="https://github.com/Raflyf/WallpaperLive/issues">Report a bug</a>
</p>

WallpaperLive uses Rust and Win32 for its English interface, desktop integration and tray controls. The thumbnail library lets you preview a file before applying it. libmpv handles media decoding; the app does not require a browser engine or an account.

> **Release v0.1.3:** unsigned. Native controls and source regressions have been checked; installation and live desktop playback still need your testing. A release tag does not establish compatibility with every Windows build, GPU or media file. Total RAM is not capped.

## Features

- Import a local folder, including subfolders, or add individual files. Browse up to 4096 wallpapers; four thumbnails are loaded per page. Originals stay in their original locations.
- Filter the library by folder; imported folder roots remain available after restart. **Select all**, beside the folder filter rather than Previous/Next, selects every file in the current filter across all pages; Ctrl-click toggles individual files. **Remove (N)** confirms the count and removes library entries, never original media.
- Start local media immediately. Oversized SDR videos get a display-sized cache in the background; original playback can temporarily use more RAM until the cache is ready. Choose **24, 30, 45 or 60 FPS** below Placement in the selected-wallpaper panel (30 default); FPS changes do not re-encode or reload the wallpaper. Smaller sources are never upscaled.
- Prefer a detected dedicated GPU for D3D11 rendering and compatible hardware decoding; fall back to the default adapter when needed.
- Apply one active wallpaper to one display you choose.
- Choose **Fill** to crop to the screen or **Fit** to show the full frame.
- Loop playback with sound off by default; toggle **Sound: On/Off** without reloading the wallpaper.
- Pause, resume and stop from the panel or tray. The tray menu follows the Windows system dark/light theme on each open, without restarting; high-contrast mode uses the native system menu. Hovering the tray icon does not open the panel.
- Remove a wallpaper from the library without deleting its original media file.
- Resize or maximize the native window; library navigation stays below the thumbnails.
- Pause for fullscreen or maximized apps on the wallpaper display, battery power, inactive sessions, display sleep and system suspend. The last frame and decoder stay attached; returning to the desktop resumes without cache preparation or a file reload. Pause stops animation/audio, drops thumbnail buffers, cancels/defer conversion and asks Windows once to trim resident RAM pages. Decoder/driver allocations remain for fast resume. Fullscreen/maximize and battery pause are configurable; manual pause remains until you clear it.
- The tray always offers **Check for updates**. Clicking checks GitHub in the background and shows an English result dialog: latest version, newer version, or check failed. A newer version asks before opening its GitHub release page. The app also checks once at startup; no automatic retry, download or installation.

## Requirements

- Windows 10 or Windows 11, x64.
- Trusted local video, GIF or image files. The download does not include wallpapers.
- A native libmpv decoder. Setup can reuse a matching installed decoder or the pinned local archive; otherwise first-time setup asks for Internet access and download consent.

Native media parsers run with your user privileges. WallpaperLive is not a sandbox: malformed files can crash the app. Use media you trust.

## Install

1. [Download the installer](https://github.com/Raflyf/WallpaperLive/releases/download/v0.1.3/wallpaperlivesetup.exe) and run it as your normal Windows user.
2. If a decoder download is needed, review the prompt and grant consent to proceed. Setup skips that prompt when it can verify an existing decoder or a local archive.
3. Open WallpaperLive from the Start menu after installation.

Setup installs per user without administrator rights. It asks before downloading libmpv and verifies SHA256 before installing it. Setup does not launch the app, change your wallpaper or enable startup.

For an offline upgrade, install into the same folder: Setup reuses a matching decoder. For a first install without an in-setup download, obtain the [pinned upstream archive](https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261107/mpv-dev-x86_64-20261107-git-9028187538.7z) beforehand, rename it `mpv-dev.7z` and place it beside the installer. Both archive and DLL checksums are verified. The single installer does not bundle the decoder; its complete dependency-source record has not been verified for redistribution.

Windows may show a publisher warning because this release is unsigned. Keep the installer in a dedicated folder without unrelated DLL files. Do not disable security protections or run the app as administrator to work around desktop access errors.

## Set a wallpaper

1. Choose **Add wallpaper**, **Add folder**, or drop a local media file into the window.
2. Select its thumbnail, choose a display and select **Fill** or **Fit**. The selected-wallpaper panel offers **24, 30, 45 or 60 FPS** below Placement (cap, not interpolation).
3. Choose **Set wallpaper**. The original starts immediately. A display-sized SDR cache is prepared in the background if needed, then playback switches to it near the current position. Changing FPS does not start another conversion; **Stop** cancels preparation. Originals are never modified.

Selecting a thumbnail changes the preview, not the active wallpaper. Use **Pause** or **Resume** to control playback. **Stop** removes the live wallpaper without changing your Windows static wallpaper settings. **Sound: On/Off** selects or disables the media's audio track; files without audio remain silent. **Remove** asks for confirmation and removes only the library entry; removing an active wallpaper also stops it.

Closing the window keeps playback in the tray when the tray is available. Choose **Exit** in the app or tray menu to close WallpaperLive. Media/display preferences are saved, and **Start with Windows** remains opt-in in Settings.

On each new application process, one background HTTPS request checks the latest stable release on GitHub. **Check for updates** starts a check on demand; repeated clicks while a check runs share its result instead of starting parallel requests. The check sends the application version in its User-Agent, not wallpaper paths or preferences. No periodic network polling or automatic retry runs. Offline, rate-limit and invalid-response errors appear in the manual result dialog, not as a false latest-version message. Opening a newer release page requires confirmation; no automatic download or installation runs. Windows notification settings or Focus Assist may hide the startup balloon; the fixed tray action remains available.

## Media formats

The file picker accepts:

`MP4` `GIF` `MKV` `WebM` `MOV` `AVI` `MPG` `MPEG` `M4V` `APNG` `PNG` `JPG` `JPEG` `BMP` `WebP`

Playback depends on the file's codec and the installed decoder. An accepted extension does not guarantee support for every codec or animated-image variant. WallpaperLive does not accept URLs, network paths, playlists, executable files or web wallpapers.

Hardware decoding depends on the codec and GPU. A detected hardware adapter with dedicated VRAM is preferred through D3D11; this also selects the device for compatible D3D11VA decoding. Driver/codec failure may fall back. Dedicated GPU use can increase power consumption and does not eliminate system RAM allocations.

Automatic cache conversion uses the installed libmpv encoder, not a separately downloaded FFmpeg program. It preserves the original file, audio, aspect ratio and playback speed. Cache copies are H.264/AAC MP4, keyed by source SHA256, display dimensions and a fixed source-rate profile capped at 60 FPS, independent of the runtime cap. Runtime FPS changes use a pure frame-rate filter without interpolation or playback-speed changes; CFR resampling can repeat existing frames around irregular timestamps. Decoding still processes the cached source rate. Old cache files remain untouched. HDR and image/animated-image inputs remain original in this release, with an explicit status note; no silent 8-bit HDR conversion. Conversion requires finite video up to one hour/16 GiB, runs one below-normal-priority worker, and stops at 15 minutes or 512 MiB per output. Total cache is limited to 2 GiB; a full cache reports an error rather than deleting original or cached files automatically. Cancelled partials are never used. Cache is in `%LOCALAPPDATA%\\WallpaperLive\\video-cache`; move unused cache files aside to free space.

Pause keeps the decoder, rendering surface and last frame so wallpaper does not disappear or wait for a reload when the desktop returns. Audio/video playback stops, thumbnail buffers are released and cache conversion is cancelled/deferred. Windows gets one resident-page trim request per confirmed pause; this lowers the working set when pages can be reclaimed, not private allocations or dedicated VRAM. Do not expect zero resources or a fixed RAM ceiling. The native panel/tray and 500 ms pause monitor remain. An unfinished conversion restarts in the background after resume; active playback does not wait for it. A completed cache waits at least two seconds after resume before a separate handoff at the current media position.

Thumbnail extraction runs one at a time in a short-lived child; the panel retains only four visible thumbnails plus the selection, each at most 480 × 270 pixels, and drops them when hidden, minimized or paused. Decoder packet budgets remain 8 MiB forward, no backward cache, 0.5 seconds readahead and four software decode threads. Total RAM is not capped. Native libraries and GPU drivers allocate additional memory; reported improvements are fixture-specific, not a guarantee for every device.

Historical v0.1.1 measurement, not a retained-pause v0.1.3 result: one owned-fixture comparison on an RTX 3050 Laptop GPU used the same 1920 × 1080 native test window and D3D11VA decoding for 15 seconds per condition. Original 4K/60 FPS video used median **275.5 MiB working set / 856.3 MiB private bytes**; its 1080p/30 FPS cache used **187.1 MiB / 355.1 MiB**. Process CPU fell from **3.79% to 1.84%** of total machine capacity. After decoder destruction, working sets were about **57 MiB**, including the Python measurement host but excluding the app panel. This was one sequential pair, not a repeatability guarantee; VRAM and GPU utilization were not measured.

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
