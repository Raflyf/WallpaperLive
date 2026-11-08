# Changelog

Release history for WallpaperLive. Each version describes its own changes; older installers keep their original behavior.

## Downloads and installation

- **Included:** Windows x64 installer, app, product icons, shortcuts and license notices.
- **Not bundled:** wallpapers, a browser engine or the native libmpv decoder. Setup reuses a verified decoder/local archive or asks before downloading the pinned decoder.
- **Unsigned builds:** use trusted local media and keep Windows security protections enabled. No administrator bypass is required.

---

## v0.1.4

### Fixed

- Setup now sets File version from the app version (`0.1.4.0`) instead of retaining the compiler's `0.0.0.0` default. Product version remains `0.1.4`.
- Installer builds verify both PE version strings before publishing, ignoring Inno Setup's trailing space padding while rejecting mismatched versions.

### Added

- Settings shows the installed app version, read directly from the build's package version.

### Changed

- Installer filenames include the release version, starting with `wallpaperlivesetup-v0.1.4.exe`. Future builds derive the filename from AppVersion automatically.
- GitHub Actions uploads, download links and SHA256 manifests use the same versioned filename. Older release files remain unchanged.
- README includes the author's real app/desktop screenshots and cache-preparation, cached-playback and auto-pause resource captures.
- Resource tables distinguish the 420.1 MB app/worker preparation group from the 73.4 MB cached app; additional snapshots show 40.1 MB playback and 4.1 MB auto-pause.
- Preview and comparison figures preserve original source screenshots and label compositions, cropped rows and snapshot limitations.
- Long cache/build notes are collapsible so downloads, features and real usage are easier to find.

### Known limitations

- Downloading the same version repeatedly can still add the browser's `(1)` suffix. No files are deleted or overwritten to prevent it.
- Screenshot values are momentary Task Manager observations, not a benchmark or fixed RAM ceiling. They do not establish total private memory, GPU utilization or VRAM savings.
- Playback, cache and pause behavior are unchanged from v0.1.3. No new native performance tests or installed-app interaction were performed for this patch.

---

## v0.1.3

### Changed

- **Tray updates:** fixed **Check for updates** action. A manual check runs in the background and shows a native English result dialog for the latest version, a newer release or a failed check.
  - Repeated clicks during a check share its result; no parallel requests or automatic retries.
  - Opening a newer release page requires confirmation. No automatic download or installation.
- **Library selection:** **Select all** moved beside the folder filter, separate from **Previous** and **Next**.
- **Wallpaper controls:** FPS selector moved below **Placement** in the selected-wallpaper panel, not Settings.
- **Settings:** compact, aligned pause/startup controls; removed the repeated Playback heading and full-width checkbox bars.
- **Language:** remaining app-authored Indonesian text changed to English.
- **Release notes:** grouped changes and limits by version instead of repeating an unstructured feature list.

### Fixed

- Unknown source FPS no longer becomes a false 1 FPS value that silently disables the requested cap.
- FPS changes retry native source metadata when needed; unknown rates keep the selected cap active.
- Update checks without a published stable release report an error, not a false latest-version result.
- Update completion monitoring is separate from wallpaper monitoring, so stopping playback does not strand a manual check.

### Verification and limits

- One owned native 4K/60 FPS probe measured scheduled frame rates of **59.6 / 24.2 / 60.2 FPS** for the **60 / 24 / 60** choices. D3D11VA decoding, D3D11 output, playback speed 1 and one file load were retained.
- This measures native frame scheduling, not physical display scanout or the installed app on every media file. Sources below the selected cap are not interpolated.
- RAM is not globally capped; the runtime filter does not reduce the source decode rate. GPU utilization and dedicated VRAM savings are not established.
- Installation and live desktop attachment still require manual testing.

---

## v0.1.2

### Fixed

- FPS changes update the running player without a file reload or another cache conversion.
- Original media starts immediately while a display-sized SDR cache is prepared in the background.
- Wallpaper switches no longer wait for running preparation workers to join on the UI thread.
- Old decoder replies and stale cache results cannot stop or replace a newer selection.
- A failed original load no longer strands a completed valid cache.

### Changed

- **Pause retains the last frame and decoder**, rather than removing the wallpaper surface. Resume does not re-prepare or reload the active file.
- Pause stops animation/audio, releases thumbnails, cancels/defer conversion and requests one resident-page trim from Windows.
- A completed cache waits at least two seconds after resume before a separate handoff near the current position.
- Cache profile uses source-rate video capped at 60 FPS, independent of the runtime FPS choice. Previous cache files are kept.

### Known limitations

- Uncached 4K playback can temporarily use more RAM while the cache is built.
- Resident-page trimming does not release private decoder allocations or dedicated VRAM.
- Unknown source FPS could bypass the runtime cap in this release; addressed in **v0.1.3**.
- Installed-app/desktop pixel retention and physical display cadence were not verified by the isolated source/native tests.

---

## v0.1.1

### Added

- Automatic display-sized SDR video cache without changing original media.
- Manual **24 / 30 / 45 / 60 FPS** choices.
- Detected dedicated GPU preference for compatible D3D11 rendering/decoding.
- Folder and subfolder import, folder filter, **Select all** and confirmed mass library removal.
- One background GitHub update check per app process, with a newer-release notification.

### Changed

- Pause releases the decoder, video surface and thumbnail buffers; resume reloads near the previous position.
- Fullscreen and maximized foreground apps on the wallpaper display trigger automatic pause.
- Conversion is cancelled/deferred while paused.
- Tray context menus follow the Windows dark/light theme; high-contrast mode retains native system rendering.

### Known limitations

- The wallpaper surface disappears while paused in this version; retained-frame pause arrives in **v0.1.2**.
- HDR and animated images keep their original media. RAM is not globally capped.
- Historical isolated measurements do not establish later-version resource use or a fixed 200 MB limit.

---

## v0.1.0

### Fixed

- Visible **Previous / Next** library navigation and working window maximize.
- **Sound On / Off** control.
- Library removal without deleting original media.
- Tray event handling ignores hover/focus instead of opening the panel.

### Changed

- Thumbnail extraction is serialized in a bounded child process and cancelled/reaped on normal app exit.
- Setup can reuse a matching decoder or a checksum-verified local archive; otherwise it asks before downloading the decoder.

### Known limitations

- One isolated 4K decoder comparison observed **478.955 MiB before / 467.783 MiB after** option changes. This is not a full-app or repeatability guarantee.
- Installation, desktop playback and resource use still require manual testing.

---

## v0.1.0-alpha.2

### Delivery

- Prerelease installer built by GitHub Actions after formatting, lint, tests and release compilation passed.
- Setup requires consent for a one-time checksum-verified native decoder download.
- No wallpaper or startup setting is applied by Setup.

### Known limitations

- Original release notes recorded delivery checks, not a separate feature-change list.
- Unsigned testing prerelease; installation, desktop playback and resource use require manual testing.

---

## v0.1.0-alpha.1

### Added

- Native Rust/Win32 app for Windows 10/11 x64, with an English thumbnail library and product icons.
- Trusted local video, GIF and image selection with thumbnail previews.
- One active wallpaper on a selected display, using **Fill** or **Fit**.
- Muted looping playback, pause, resume and stop.
- Automatic pause for fullscreen apps, battery power, inactive sessions, display sleep and system suspend.
- Saved preferences and optional Windows startup.
- Per-user installer, Start menu shortcut and dependency notices.

### Verification and limits

- Original release recorded passing Rust formatting, Clippy, unit tests and release compilation; native decoder tests remained opt-in.
- Installer PE header and SHA256 manifest were inspected without running Setup.
- Unsigned testing prerelease. Build checks do not guarantee desktop attachment, GPU compatibility or measured CPU/RAM use.
- Setup does not launch the app, apply a wallpaper or enable startup. No wallpapers are included.

---

[All GitHub releases](https://github.com/Raflyf/WallpaperLive/releases) · [Report a bug](https://github.com/Raflyf/WallpaperLive/issues)
