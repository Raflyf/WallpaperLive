<p align="center">
  <img src="assets/wallpaperlive.png" width="112" height="112" alt="WallpaperLive logo">
</p>

<h1 align="center">WallpaperLive</h1>

<p align="center">Videos, GIFs and images as wallpaper, with a native Windows interface.</p>

<p align="center">
  <a href="https://github.com/Raflyf/WallpaperLive/actions/workflows/ci.yml"><img src="https://github.com/Raflyf/WallpaperLive/actions/workflows/ci.yml/badge.svg" alt="Windows checks"></a>
  <a href="https://github.com/Raflyf/WallpaperLive/releases/tag/v0.1.0"><img src="https://img.shields.io/badge/release-v0.1.0-586575" alt="Release v0.1.0"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-586575" alt="GPL-3.0-or-later license"></a>
</p>

<p align="center">
  <a href="https://github.com/Raflyf/WallpaperLive/releases/download/v0.1.0/wallpaperlivesetup.exe">Download Windows installer</a>
  &nbsp;·&nbsp;
  <a href="https://github.com/Raflyf/WallpaperLive/releases">All releases</a>
  &nbsp;·&nbsp;
  <a href="https://github.com/Raflyf/WallpaperLive/issues">Report a bug</a>
</p>

WallpaperLive uses Rust and Win32 for its English interface, desktop integration and tray controls. The thumbnail library lets you preview a file before applying it. libmpv handles media decoding; the app does not require a browser engine or an account.

> **Release v0.1.0:** unsigned. Native controls and source regressions have been checked; installation and live desktop playback still need your testing. A release tag does not establish compatibility with every Windows build, GPU or media file. Total RAM is not capped.

## Features

- Browse thumbnails for up to 12 recent local files. Your media stays in its original location.
- Apply one active wallpaper to one display you choose.
- Choose **Fill** to crop to the screen or **Fit** to show the full frame.
- Loop playback with sound off by default; toggle **Sound: On/Off** without reloading the wallpaper.
- Pause, resume and stop from the panel or tray. Hovering the tray icon does not open the panel.
- Remove a wallpaper from the library without deleting its original media file.
- Resize or maximize the native window; library navigation stays below the thumbnails.
- Pause for fullscreen apps, battery power, inactive sessions, display sleep and system suspend. Fullscreen and battery pause are configurable; manual pause remains in effect until you clear it.
- Remember media and display preferences, with Windows startup as an opt-in setting.

## Requirements

- Windows 10 or Windows 11, x64.
- Trusted local video, GIF or image files. The download does not include wallpapers.
- A native libmpv decoder. Setup can reuse a matching installed decoder or the pinned local archive; otherwise first-time setup asks for Internet access and download consent.

Native media parsers run with your user privileges. WallpaperLive is not a sandbox: malformed files can crash the app. Use media you trust.

## Install

1. [Download the installer](https://github.com/Raflyf/WallpaperLive/releases/download/v0.1.0/wallpaperlivesetup.exe) and run it as your normal Windows user.
2. If a decoder download is needed, review the prompt and grant consent to proceed. Setup skips that prompt when it can verify an existing decoder or a local archive.
3. Open WallpaperLive from the Start menu after installation.

Setup installs per user without administrator rights. It asks before downloading libmpv and verifies SHA256 before installing it. Setup does not launch the app, change your wallpaper or enable startup.

For an offline upgrade, install into the same folder: Setup reuses a matching decoder. For a first install without an in-setup download, obtain the [pinned upstream archive](https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261107/mpv-dev-x86_64-20261107-git-9028187538.7z) beforehand, rename it `mpv-dev.7z` and place it beside the installer. Both archive and DLL checksums are verified. The single installer does not bundle the decoder; its complete dependency-source record has not been verified for redistribution.

Windows may show a publisher warning because this release is unsigned. Keep the installer in a dedicated folder without unrelated DLL files. Do not disable security protections or run the app as administrator to work around desktop access errors.

## Set a wallpaper

1. Choose **Add wallpaper**, or drop a local media file into the window.
2. Select its thumbnail, choose a display and select **Fill** or **Fit**.
3. Choose **Set wallpaper**.

Selecting a thumbnail changes the preview, not the active wallpaper. Use **Pause** or **Resume** to control playback. **Stop** removes the live wallpaper without changing your Windows static wallpaper settings. **Sound: On/Off** selects or disables the media's audio track; files without audio remain silent. **Remove** asks for confirmation and removes only the library entry; removing an active wallpaper also stops it.

Closing the window keeps playback in the tray when the tray is available. Choose **Exit** in the app or tray menu to close WallpaperLive. You can enable **Start with Windows** in Settings; it starts disabled.

## Media formats

The file picker accepts:

`MP4` `GIF` `MKV` `WebM` `MOV` `AVI` `MPG` `MPEG` `M4V` `APNG` `PNG` `JPG` `JPEG` `BMP` `WebP`

Playback depends on the file's codec and the installed decoder. An accepted extension does not guarantee support for every codec or animated-image variant. WallpaperLive does not accept URLs, network paths, playlists, executable files or web wallpapers.

Hardware decoding depends on your codec and GPU. Thumbnail extraction runs one at a time in a short-lived child process; the panel retains only visible/selected thumbnails, each at most 480 × 270 pixels, and drops them when hidden. The decoder uses an 8 MiB forward packet budget, no backwards packet cache, 0.5 seconds of readahead and four software decode threads. This does not cap total RAM: native libraries, video frames, shell codecs in the child and GPU allocations use additional memory. For lower playback cost, use video close to your display resolution and avoid unnecessary frame rates.

A single decoder-only comparison using the same 4K input and a 1280 × 720 native window observed median working sets of 478.955 MiB before and 467.783 MiB after the option changes. That is a small decrease, not a total-memory fix or a repeatable performance guarantee. The panel alone used about 20 MiB with test thumbnails; that figure excludes wallpaper playback.

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
