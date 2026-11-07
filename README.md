<p align="center">
  <img src="assets/wallpaperlive.png" width="112" height="112" alt="WallpaperLive logo">
</p>

<h1 align="center">WallpaperLive</h1>

<p align="center">Videos, GIFs and images as wallpaper, with a native Windows interface.</p>

<p align="center">
  <a href="https://github.com/Raflyf/WallpaperLive/actions/workflows/ci.yml"><img src="https://github.com/Raflyf/WallpaperLive/actions/workflows/ci.yml/badge.svg" alt="Windows checks"></a>
  <a href="https://github.com/Raflyf/WallpaperLive/releases/tag/v0.1.0-alpha.2"><img src="https://img.shields.io/badge/release-v0.1.0--alpha.1-586575" alt="Release v0.1.0-alpha.2"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-586575" alt="GPL-3.0-or-later license"></a>
</p>

<p align="center">
  <a href="https://github.com/Raflyf/WallpaperLive/releases/download/v0.1.0-alpha.2/wallpaperlivesetup.exe">Download Windows installer</a>
  &nbsp;·&nbsp;
  <a href="https://github.com/Raflyf/WallpaperLive/releases">All releases</a>
  &nbsp;·&nbsp;
  <a href="https://github.com/Raflyf/WallpaperLive/issues">Report a bug</a>
</p>

WallpaperLive uses Rust and Win32 for its English interface, desktop integration and tray controls. The thumbnail library lets you preview a file before applying it. libmpv handles media decoding; the app does not require a browser engine or an account.

> **Testing prerelease:** v0.1.0-alpha.2 is unsigned. Manual installation and desktop playback testing are still outstanding. Build checks do not establish compatibility with your Windows build, GPU or media. This release makes no measured CPU or RAM claims.

## Features

- Browse thumbnails for up to 12 recent local files. Your media stays in its original location.
- Apply one active wallpaper to one display you choose.
- Choose **Fill** to crop to the screen or **Fit** to show the full frame.
- Loop playback without sound, with pause, resume and stop controls in the app and tray.
- Pause for fullscreen apps, battery power, inactive sessions, display sleep and system suspend. Manual pause remains in effect until you clear it.
- Remember media and display preferences, with Windows startup as an opt-in setting.

## Requirements

- Windows 10 or Windows 11, x64.
- Trusted local video, GIF or image files. The download does not include wallpapers.
- A native libmpv decoder. First-time setup needs Internet access for its checksum-verified download; setup reuses an existing decoder only when it matches the pinned build.

Native media parsers run with your user privileges. WallpaperLive is not a sandbox: malformed files can crash the app. Use media you trust.

## Install

1. [Download the installer](https://github.com/Raflyf/WallpaperLive/releases/download/v0.1.0-alpha.2/wallpaperlivesetup.exe) and run it as your normal Windows user.
2. Review the decoder download prompt and grant consent if you want to proceed. Cancel setup if you do not want the download.
3. Open WallpaperLive from the Start menu after installation.

Setup installs per user without administrator rights. It asks before downloading libmpv and verifies SHA256 before installing it. Setup does not launch the app, change your wallpaper or enable startup.

Windows may show a publisher warning because this prerelease is unsigned. Keep the installer in a dedicated folder without unrelated DLL files. Do not disable security protections or run the app as administrator to work around desktop access errors.

## Set a wallpaper

1. Choose **Add wallpaper**, or drop a local media file into the window.
2. Select its thumbnail, choose a display and select **Fill** or **Fit**.
3. Choose **Set wallpaper**.

Selecting a thumbnail changes the preview, not the active wallpaper. Use **Pause** or **Resume** to control playback. **Stop** removes the live wallpaper without changing your Windows static wallpaper settings.

Closing the window keeps playback in the tray when the tray is available. Choose **Exit** in the app or tray menu to close WallpaperLive. You can enable **Start with Windows** in Settings; it starts disabled.

## Media formats

The file picker accepts:

`MP4` `GIF` `MKV` `WebM` `MOV` `AVI` `MPG` `MPEG` `M4V` `APNG` `PNG` `JPG` `JPEG` `BMP` `WebP`

Playback depends on the file's codec and the installed decoder. An accepted extension does not guarantee support for every codec or animated-image variant. WallpaperLive does not accept URLs, network paths, playlists, executable files or web wallpapers.

Hardware decoding depends on your codec and GPU. For lower playback cost, use video close to your display resolution and avoid high frame rates you do not need. Resource use depends on the media, decoder and hardware.

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
