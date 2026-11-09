# Changelog

Release history for WallpaperLive. Each version describes its own changes; older installers keep their original behavior.

## Downloads and installation

- **Included:** Windows x64 installer, app, product icons, shortcuts and license notices.
- **Bring your own live wallpapers:** WallpaperLive applies and plays your local files. It does not host wallpaper downloads or provide an online gallery, catalog or marketplace.
- **Not bundled:** wallpapers, a browser engine or the native libmpv decoder. Setup reuses a verified decoder/local archive or asks before downloading the pinned decoder.
- **Unsigned builds:** use trusted local media and keep Windows security protections enabled. No administrator bypass is required.

---

## Unreleased

- Add [Saweria](<https://saweria.co/BuyMatcha>) to the README and alongside Buy me a coffee in Settings. The app change is queued for the next feature or fix release; v0.1.9 and its installer remain unchanged.

---

## v0.1.9

### Changed

- Align the Library toolbars, nine-card grid, preview and playback controls with consistent spacing and equal-width paired buttons. Use the thumbnail image area without a second letterbox and increase filename readability.
- Organize Settings into matching Playback & startup and Resources & support columns. Hide the unrelated wallpaper preview, filename and playback controls on this page.
- Replace the Where to find wallpapers confirmation dialog and popup menu with an integrated page in the app. Four fixed website links open in the default browser; media and license guidance stays inline. Back to Library or Escape preserves the previous filter, selection and page.
- Keep keyboard focus visible and let Space activate buttons and Settings checkboxes instead of triggering the Library pause shortcut.
- Add [Buy me a coffee](<https://yapp.ink/raflyf>) in Settings and the README. Support is optional; the link opens in the browser and never submits a payment from the app.
- Update application and installer metadata to `0.1.9`; the tray tooltip continues to use the executable's version.

### Unchanged

- Choosing a display only selects the target. Use Set wallpaper to apply; existing multi-display playback and saved media/preferences are unchanged.
- No wallpaper gallery, embedded browser, wallpaper downloader or new runtime dependency. Existing destructive-action confirmations remain.
- Historical screenshots and resource measurements keep their original version labels; they are not v0.1.9 measurements.

---

## v0.1.8

### Fixed

- Show `WallpaperLive v0.1.8` when hovering over the tray icon. Enable the standard Windows tooltip with version-4 tray notifications; the version comes from the executable build.

### Added

- Where to find wallpapers is visible in the main Library. Its menu opens [MyLiveWallpapers](<https://mylivewallpapers.com>), [MoeWalls](<https://moewalls.com>), [MotionBgs](<https://motionbgs.com>) or [DesktopHut](<https://www.desktophut.com>) in your browser.
- Settings includes a link to [LiveBar](<https://livebar.vercel.app>).
- Search filenames without case sensitivity within the selected folder filter. Sort by newest imported, A-Z or Z-A.
- Right-click a library thumbnail and choose Retry thumbnail to request a new preview.
- Play wallpapers on multiple displays. All displays applies the same selected file everywhere; applying to one display leaves other displays playing their assigned files.

### Changed

- Fullscreen/maximized auto-pause applies independently to each wallpaper display. Manual Pause/Resume and Stop affect all displays; sound uses at most one wallpaper stream.
- Settings version 4 stores per-monitor wallpaper assignments. Existing settings versions 1-3 retain the saved wallpaper on the same saved monitor, or the primary display when the monitor name is blank; migration does not expand playback to All displays. Existing data directories stay unchanged.
- Remove automatic cache eviction. When the 2 GiB budget cannot accommodate another copy, cache preparation refuses conversion and keeps original playback. Clear unused cache requires confirmation and protects imported library media, all active originals and active or waiting cache outputs.
- Update current application and installer metadata to `0.1.8`.

### Known limitations

- Each active display uses its own decoder. More displays can increase memory and GPU use; frame-perfect synchronization is not promised. Cache preparation remains limited to one concurrent worker.
- External links are browsing shortcuts, not an in-app gallery, hosted media or downloader. They do not establish third-party safety or endorsement. Obtain compatible media separately and respect the creator's license before importing it.
- Resource screenshots and measurements retain their original version labels. They are not v0.1.8 performance results or resource-use guarantees.

---

## v0.1.7

### Fixed

- **Taskbar Identity:** Set the stable `Raflyf.WallpaperLive` AppUserModelID on both the native process and Start-menu shortcut. Register large and small class icons before creating the panel window, instead of leaving taskbar identity to Windows heuristics. No icon-cache deletion, Explorer restart, or media/settings changes.

- **Executable Shell Icon:** The original v0.1.7 executable set window/tray icons at runtime but contained no Windows icon resources. Build and installer packaging now embed and byte-verify the nine-size ICO as `RT_GROUP_ICON`/`RT_ICON` while preserving the application manifest. This rebuild replaces the earlier v0.1.7 release; v0.1.6 and older releases remain unchanged.

### Changed

- **Sunset Window App Icon:** Replaced the angular blue mark with a tilted graphite window, copper edges, a cream sunset, and a winding light path through an orange canyon. The same artwork supplies the app window/tray, installer, shortcut, and README logo.
- **Orange Resource Annotations:** Recolored the large metric annotations to burnt orange (`#D9541E`). The v0.1.5 Task Manager photo strips, displayed measurements, and source attribution remain unchanged.
- Updated app and installer metadata to `0.1.7`; v0.1.6 and older release assets remain unchanged.

### Known limitations

- Performance screenshots still document v0.1.5/v0.1.6; they are not new v0.1.7 measurements. Playback, cache conversion, and pause behavior are unchanged.

---

## v0.1.6

### Added

- **3x3 Wallpaper Library Grid:**
  - Expanded the library browsing grid from 4 (2x2) to 9 (3x3) cards per page, allowing users to view more wallpapers simultaneously and reducing pagination.
  - Cards feature responsive 16:9 thumbnail viewports with rounded corners, clean title labels, and an "Active" badge on the currently playing wallpaper.
  - Integrated selection checkboxes for batch management, fully compatible with Ctrl-click and "Select all".
  - Updated keyboard navigation (`←`/`→` arrow keys) to step across 9-card pages.

### Changed

- **Visual Theme & Palette Overhaul:**
  - Refined the interface with a modern Cream, Ink Black, and Burnt Orange color palette:
    - **Ink Black (`#111111`):** Primary canvas, background panels, and footer.
    - **Cream (`#F3EEE3`):** High-contrast typography, headings, and primary button labels.
    - **Burnt Orange (`#D9541E`):** Accent highlights for active tabs, selected card borders, badges, and primary action buttons.
  - Active card selection now uses a sleek accent border rather than an opaque full-card color fill.
  - Removed standard Win32 dotted focus rectangle outlines (`DrawFocusRect`) on buttons and cards for a cleaner, unified presentation.
- **Header Navigation:**
  - Integrated top bar segmented control tabs for switching between `Library` and `Settings`.
  - Clean view switching with proper native window visibility management (`ShowWindow`), preventing background control bleed-through.
- **Updated Benchmark Showcase in README:** Added author-supplied v0.1.6 captures showing 437.5 MB during initial 4K conversion, 41.4–47.4 MB with the panel open, and 37.8 MB / 1.5% CPU during desktop playback. Retained the v0.1.5 tray and auto-pause photos (20.3 MB / 0.5% CPU and 4.5 MB / 0% CPU), with their original version labeled.
- Updated build package and installer metadata to `0.1.6`.

### Known limitations

- Library thumbnail extraction operates on local video files; thumbnails for very large or corrupted files fall back to extension labels until decoded.

---

## v0.1.5

### Fixed

- **4K display-cache encoding:** Corrected the libavcodec option from `ref=1` to `refs=1`. The invalid name stopped conversion with `AVOption 'ref' not found`, leaving original high-resolution playback active. The preset, CRF, output resolution, FPS policy and audio handling are unchanged.

### Added

- **Launch on Setup Finish:** Setup provides an option (checked by default) to immediately launch WallpaperLive upon installation completion.
- **Installer Auto-Exit:** Setup and uninstaller automatically terminate any running WallpaperLive instance so users no longer have to manually exit from the system tray before updating or uninstalling.
- **Live Diagnostics in Settings:** Settings view displays real-time memory usage (process working set and peak working set) and video cache storage usage.
- **Cache Manager & Manual Cleanup:** Added a "Clear unused cache" button in Settings to purge obsolete and inactive video cache files with confirmation, protecting currently playing wallpaper files.
- **Cache Auto-Eviction:** Video cache automatically prunes oldest unused cache files when near the 2 GiB limit before preparing new wallpapers, avoiding manual cache folder management.
- **Thumbnail Context Menu:** Right-clicking any library thumbnail opens a native menu with "Set as wallpaper", "Open file location", "Copy file path", and "Remove from library".
- **Double-Click to Apply:** Double-clicking any wallpaper thumbnail immediately starts and applies it as the active wallpaper.
- **Keyboard Shortcuts:** `Space` toggles Pause/Resume, and `Left`/`Right` arrow keys navigate library pages when the control panel is focused.

### Changed

- **Low-Spec Hardware Optimizations:**
  - Uses x264 `veryfast`, CRF 23, two encoder threads, lookahead 10 and one reference frame (`refs=1`). Lookahead/reference values already match `veryfast`; they do not establish an additional 75% RAM reduction.
  - Uses bilinear renderer scalers (`scale=bilinear`, `cscale=bilinear`, `dscale=bilinear`). Scaling/chroma reconstruction still performs GPU work; zero GPU usage or identical output quality is not guaranteed.
  - Forward packet budget is 4 MiB with 0.2-second readahead and two software-decode threads. These are limits/settings, not guaranteed total process memory or smooth 4K software decoding on every CPU.
  - Requests a Windows working-set trim after load and when hiding the panel. This affects resident pages, not total private allocations; pages may return and page faults can increase.
- **Updated Benchmark Showcase in README:** Updated resource usage tables and figures with real v0.1.5 author-supplied captures (reflecting ~348.9 MB for 1080p and ~409.3 MB temporary peak specifically when converting 4K wallpapers, ~43.1–45.4 MB panel-open playback, 20.3 MB / 0.5% CPU tray playback, and 4.5 MB / 0% CPU auto-pause).
- Updated build package and installer metadata to `0.1.5`.

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
