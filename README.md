# WallpaperLive

A native Windows app for setting your own videos, GIFs and images as desktop wallpaper.

WallpaperLive keeps the workflow local: add a file, choose a display and set it as your wallpaper. Its English interface uses a thumbnail library and a selected-file preview, with playback controls kept close to the image.

[Download the Windows installer](https://github.com/Raflyf/WallpaperLive/releases) · [Report a bug](https://github.com/Raflyf/WallpaperLive/issues)

> Version 0.1.0 is an early testing release. Desktop compatibility and resource usage still need testing on your hardware.

## Features

- A local library with up to 12 recent files. Media stays in its original location.
- One active wallpaper on your chosen display.
- Fill or fit placement, with muted looping playback.
- Pause, resume and stop from the app or Windows tray.
- Automatic pause for fullscreen apps, battery power, inactive sessions and display sleep.
- Remembered display preferences and optional startup with Windows.

The app uses Rust and Win32 for its interface and desktop integration, with libmpv for native media decoding. It does not run a browser engine or require an account.

## Install

Requires Windows 10 or 11, 64-bit.

1. Download `setup.exe` from Releases and run it as your normal Windows user.
2. Accept the one-time decoder download. Internet access is required during installation.
3. Open WallpaperLive from the Start menu.

The installer downloads about 31.8 MB and verifies its checksum before installing the decoder. Installation is per-user; no administrator rights are required. It does not start the app, change your wallpaper or enable startup automatically.

This release is unsigned, so Windows may display a publisher warning. Keep the installer in a dedicated folder without unrelated DLL files. Do not disable security protections to run it.

## Set a wallpaper

1. Choose **Add wallpaper** or drop a local media file into the window.
2. Select its thumbnail, choose a display and select **Fill** or **Fit**.
3. Choose **Set wallpaper**.

Selecting a thumbnail changes the preview, not the active wallpaper. **Stop** removes the live wallpaper. Closing the window keeps playback in the tray when the tray is available; use **Exit** to close the app completely.

## Media formats

Accepted extensions:

`MP4` `GIF` `MKV` `WebM` `MOV` `AVI` `MPG` `MPEG` `M4V` `APNG` `PNG` `JPG` `JPEG` `BMP` `WebP`

Playback depends on the file's codec and the installed decoder. An accepted extension does not guarantee every codec or animated-image variant. URLs, playlists, executable files and web wallpapers are not supported.

For lower playback costs, use a video close to your display resolution and avoid unnecessarily high frame rates. Hardware decoding is used where supported; CPU and GPU usage depend on your media and hardware.

## Build from source

Install a Rust toolchain with its normal Windows linker prerequisites, then run:

```powershell
cargo build --locked --release --bin wallpaperlive
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\Get-Decoder.ps1 -Destination .\target\release\runtime
```

Create the installer with:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\Installer.ps1
```

Run source checks with:

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Build outputs, downloaded tools, decoder binaries and local settings are excluded from Git. Installer downloads belong in Releases, not the source tree.

## Limitations

Use trusted local media. Native decoders are not a sandbox, and malformed files can crash the app. Windows desktop changes, HDR, virtual desktops and display reconnection can affect wallpaper attachment. If Windows isolates the process or blocks access to Explorer, keep playback stopped rather than running the app as administrator.

Install, playback and resource-use testing remains necessary before treating this release as production-ready. Bug reports should include the app version, Windows build, GPU, media format and steps to reproduce.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE) and [THIRD_PARTY.md](THIRD_PARTY.md) for dependency notices.
