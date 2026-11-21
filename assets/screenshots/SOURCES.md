# Screenshot sources

The project author supplied these WallpaperLive screenshots on 21 November 2026 (`v0.2.4` UI/tray captures at 14:13-14:15 and `v0.2.5` GPU-cache/pause captures at 14:44-14:46, plus a 1080p live playback video at 14:50-14:52), alongside historical captures from 8-9 November 2026. They show one Windows 11 setup, not a benchmark or a guarantee for other media, hardware, or releases.

## v0.2.5 and v0.2.4 captures (21 November 2026)

Generated README figures keep the unmodified desktop and Task Manager screenshots at full framing. Pillow composes side-by-side figures using `#111111` Ink Black backgrounds, `#F3EEE3` Cream headings, `#D9541E` Burnt Orange metric readouts matching visible Task Manager rows, and Segoe UI typography.

| File | Content | SHA256 |
| :--- | :--- | :--- |
| [preview-v0.2.5.png](preview-v0.2.5.png) | Smoked Ink Black Acrylic Library (3x3 grid, FPS/Quality/GPU controls) and Day Clock (v0.2.4 UI, identical layout to v0.2.5), resized 1600 x 900 | `b23f52c86237c0711d1d1db8f1273a8cd5fa0df913e4f3ea17e10199e2f76ee9` |
| [day-clock-v0.2.5.png](day-clock-v0.2.5.png) | Desktop Day Clock tab (Anurati typography, HSV color wheel + hex picker `#A31A37`, 24-hour/size/opacity/behind-icons controls, v0.2.4 UI), resized 1600 x 900 | `8870d6cfc08f121d6f0324f325f513bc3f27dc3e6a5cca58682e78aedd977900` |
| [cache-comparison-v0.2.5.png](cache-comparison-v0.2.5.png) | Side-by-side v0.2.5 4K GPU cache preparation (`449.8 MB`, `12.8% CPU`, `14:44:35`) vs finished cache ready (`35.4 MB`, `1.8% CPU`, `14:44:51`, ~16 s later) | `9871ca5cd2e486482ed256d7f235ab110504c065263d8cf8ad73259b6140bd10` |
| [tray-and-pause-v0.2.5.png](tray-and-pause-v0.2.5.png) | Side-by-side tray live playback + Day Clock (`24.3 MB`, `1.5% CPU`, v0.2.4 at `14:13`) vs paused / auto-paused (`2.1 MB`, `0% CPU`, v0.2.5 at `14:45`) | `03144f3010b2ed7d9f846aac160263288e7e3895c463512445f459321ce0bfcb` |

Source captures (supplied in the author's local archive on 21 November 2026; WebP/MP4 showcase derivatives live in the LivelyDesk website repository):
- `library showcase.png`: `7b058e4b5842124a47aac3d5b0cbb1bcce68b4ffadd0ed9b4cbc6bf737947614` (v0.2.4 UI at 14:15)
- `clock showcase.png`: `09ffed3b6cd9820f8c3cd12e131dab6801db2b9052fa4778f1cedfce116d888f` (v0.2.4 UI at 14:15)
- `setting showcase.png`: `de7683bd55194d4626224e8de9492d59784e4705dfbfc8ba04202493d5495816` (v0.2.4 Settings at 14:15)
- `other wallpaper live usage in open panel.png`: `ac76104e2db1cbd7c1dbe3e2c3cfe071c5bef3fec799fe876311d65b64383ddc` (`46.0 MB`, `1.8% CPU`, v0.2.4 at 14:14)
- `wallpaper live usage in closed panel.png`: `b3b6a4a68f4ddf577c5f05656b7a5d162658d877816b08549500a0e8b57c82ae` (`24.3 MB`, `1.5% CPU`, `0.2 MB/s` Disk, v0.2.4 at 14:13)
- `during cache.png`: `0229ee23cce2e80a7d00d680b482b9e8ef4d550f9b4cee7ff4076112fb4f746f` (`449.8 MB`, `12.8% CPU`, `0.8 MB/s` Disk, v0.2.5 at 14:44:35)
- `after cache.png`: `01bcb08a687fa0bb6d94da8052ecbb44880bdfef52bfefa6990b429b2d7e5fad` (`35.4 MB`, `1.8% CPU`, `0 MB/s` Disk, v0.2.5 at 14:44:51)
- `paused.png`: `04ee034c8d4b04b64f7dd57b0fbc179c5fb8cce985f9b9e02e1e71a49dadbdd7` (`2.1 MB`, `0% CPU`, `0 MB/s` Disk, v0.2.5 at 14:45)
- `video wallpaperlive.mp4`: `f6a58f6cd047362c4f0869d4b85f232296c589e83d71207cbf4f7aab2d69d132` (v0.2.5 continuous live playback + Task Manager `~26-39 MB`, `1.3-2.3% CPU`, 14:50-14:52)

## Historical v0.1.6 captures (9 November 2026)

Copies retain the supplied attachment bytes. SHA256 values below identify the normalized attachments used here, not an unobserved original capture file.

| File | Content | SHA256 |
| :--- | :--- | :--- |
| [cache-preparing-v0.1.6.png](cache-preparing-v0.1.6.png) | Background cache preparation: 437.5 MB, 42.4% CPU (two processes: app + cache worker) | `52c46ce0addfcfe312fb655b1239bbbab0987dd98500ca0adeda28898d012042` |
| [cache-ready-v0.1.6.png](cache-ready-v0.1.6.png) | Cache ready, panel open: 41.4 MB, 2.5% CPU | `24154a4796db8ad4f2ef567b29ea636d70869c1d5aa1ef8a1261c2969790c2a5` |
| [desktop-playing-tray-v0.1.6.png](desktop-playing-tray-v0.1.6.png) | Live playback on desktop: 37.8 MB, 1.5% CPU | `34ac3d66b12aaa20b738d8e3b07864bb637656ad1795bcae32068d4baa9165b3` |
| [task-manager-playing-v0.1.6.png](task-manager-playing-v0.1.6.png) | WallpaperLive 3x3 library panel open: 47.4 MB, 1.3% CPU | `fc4b989156d002b146807d04ad58663326c6ac58b18054ebb4cb71f044093d51` |

## v0.1.6 README compositions

Pillow composes these figures using the original captures. Segoe UI labels match the app's Windows typography; no screenshot values or app states are synthesized.

- [preview-v0.1.6.png](preview-v0.1.6.png): desktop capture resized to 1600 x 900 without synthetic overlays.
- [cache-comparison-v0.1.6.png](cache-comparison-v0.1.6.png): complete preparation and completion captures resized side by side; annotations repeat the visible process values. Preparation groups the app and worker (437.5 MB); completion shows the app alone (41.4 MB).

## Retained tray and auto-pause figure (v0.1.5)

- [resource-use-v0.1.5.png](resource-use-v0.1.5.png): existing figure kept unchanged. Both Task Manager strips show `wallpaperlive.exe`: tray playback (20.3 MB / 0.5% CPU) and auto-pause (4.5 MB / 0% CPU), supplied on 8 November 2026.
- Original figure SHA256: `cd60990214fb034dbf28039684a9870b10fe9a9c15879739d0e156e017b795bc`.
- [resource-use-orange.png](resource-use-orange.png): derivative figure kept for reference. Only the large blue metric annotations were recolored `#D9541E`.
- Orange figure SHA256: `03f5bc5e6280fc799e106af939a3d6edc9c6f9338e5758bba8b3f9e774e5f96d`.

## Earlier captures (v0.1.5)

| File | Content | SHA256 |
| :--- | :--- | :--- |
| [cache-preparing-v0.1.5.png](cache-preparing-v0.1.5.png) | Background cache preparation (1080p): 348.9 MB, 42.5% CPU | `1a25c7911ece87bb7f6f7dee21190cee07ee2b669f39297f3ecf1bbecd5a7c91` |
| [cache-preparing-4k-v0.1.5.png](cache-preparing-4k-v0.1.5.png) | 4K cache preparation peak (specifically when converting 4K wallpaper): 409.3 MB, 50.3% CPU | `bb9689818b2598c8cb69caebdf700b9621f60b3ca4cb1ea9276dd6d373534741` |
| [cache-ready-v0.1.5.png](cache-ready-v0.1.5.png) | Cache ready, panel open: 43.1 MB, 1.3% CPU | `21f94c3077f2da806c650ac0445137376a0e6017cdb858698e978cda2fd6ec86` |
| [desktop-playing-tray-v0.1.5.png](desktop-playing-tray-v0.1.5.png) | Live playback in tray (panel closed): 20.3 MB, 0.5% CPU | `30bb0fe548f778499f8cd5aa4236f48d6b059d65fd30bbe41040fee372d328ca` |
| [task-manager-playing-v0.1.5.png](task-manager-playing-v0.1.5.png) | Playback snapshot: 45.4 MB, 2.6% CPU | `56291ae18f47b69e263e39445c2b2b8b1d56330a302f7d09b7e68eaef418e867` |
| [task-manager-paused-v0.1.5.png](task-manager-paused-v0.1.5.png) | Fullscreen/maximized auto-pause: 4.5 MB, 0% CPU | `d8bc63444beddb4aae2f6f6d383207383e823b962fd6aa99721bffac9fcf65db` |

## Earlier captures (v0.1.3)

| File | Content | SHA256 |
| :--- | :--- | :--- |
| [library-v0.1.3.png](library-v0.1.3.png) | Native app, library and selected-wallpaper controls | `da962d9e5f98d277c94388e444f15f33b5c3bcdfbb00002499ef42e86b2585e0` |
| [desktop-v0.1.3.png](desktop-v0.1.3.png) | Video wallpaper on the author's desktop | `550f41c66c775609c78192596979d6f49b4658aa3d60a0` |
| [cache-preparing-v0.1.3.png](cache-preparing-v0.1.3.png) | Background cache preparation, app/worker group: 420.1 MB, 33.9% CPU | `7850ffe555d2e735dd49ef6cb9e5c0004b237687464ade57d62002bbc4763142` |
| [cache-ready-v0.1.3.png](cache-ready-v0.1.3.png) | Cache ready, app: 73.4 MB, 1.8% CPU | `74872db170cbb7cfa73771a77946e5dbb5bafe671ebf2ff3a88366deee6d478d` |
| [task-manager-playing-v0.1.3.png](task-manager-playing-v0.1.3.png) | Separate cached-playback snapshot: 40.1 MB, 2.0% CPU | `f021756b044edeca5848eb80b11667a9e8c4cd467bf232dfac375708330ed541` |
| [task-manager-paused-v0.1.3.png](task-manager-paused-v0.1.3.png) | Fullscreen/maximized auto-pause: 4.1 MB, 0% CPU | `cb4a790548a284d743a73d2a8eec05d6f49c416305b2d27016ab1fd1fb22b662` |

Descriptions and source attribution are embedded in the PNG metadata. Original captures remain available above for review.

## Interpretation and rights

Task Manager displays rounded, momentary CPU and memory values. Its Memory column does not establish total private allocation or dedicated VRAM. These captures do not measure GPU utilization, time averages, or a fixed memory ceiling.

Wallpaper artwork belongs to its respective owners. Screenshots illustrate the author's local setup; wallpapers are not included with WallpaperLive, and the app's GPL license does not relicense that artwork.
