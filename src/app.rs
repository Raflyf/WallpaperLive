use crate::{
    core::{Fit, PauseReasons, Rect, Settings, validate_media},
    desktop::{self, Monitor, Surface},
    mpv::{self, Event, Mpv},
    visuals,
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        Storage::FileSystem::*,
        System::{
            Com::*, LibraryLoader::GetModuleHandleW, Power::*, Registry::*, RemoteDesktop::*,
            SystemServices::*, Threading::*,
        },
        UI::{
            Controls::{
                BST_CHECKED, DRAWITEMSTRUCT, Dialogs::*, ICC_STANDARD_CLASSES,
                INITCOMMONCONTROLSEX, InitCommonControlsEx, SetWindowTheme,
            },
            HiDpi::*,
            Input::KeyboardAndMouse::{EnableWindow, SetFocus},
            Shell::*,
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, w},
};

const ACTION: u32 = WM_APP + 1;
const LAYOUT: u32 = WM_APP + 2;
const POWER: u32 = WM_APP + 3;
const SESSION: u32 = WM_APP + 4;
const DISPLAY: u32 = WM_APP + 5;
const TRAY: u32 = WM_APP + 6;
const TRAY_EVENT: u32 = WM_APP + 7;
const DROP: u32 = WM_APP + 8;
const PICK: usize = 101;
const APPLY: usize = 102;
const PAUSE: usize = 103;
const STOP: usize = 104;
const SCREEN: usize = 105;
const FIT: usize = 106;
const BATTERY: usize = 107;
const STARTUP: usize = 108;
const OPEN: usize = 109;
const EXIT: usize = 110;
const HIDE: usize = 111;
const FILE: usize = 112;
const STATUS: usize = 113;
const SETTINGS: usize = 114;
const THUMBNAIL: u32 = WM_APP + 9;
const CLASS: PCWSTR = w!("WallpaperLive.Panel");
static TASKBAR: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn native_error(error: windows::core::Error) -> String {
    error.to_string()
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: owned native handle, never closed by another object.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
struct Playback {
    player: Mpv,
    surface: Surface,
    path: PathBuf,
}
struct App {
    hwnd: HWND,
    controls: Vec<(usize, HWND)>,
    font: HFONT,
    monitors: Vec<Monitor>,
    settings: Settings,
    selected: Option<PathBuf>,
    config: PathBuf,
    config_valid: bool,
    playback: Option<Playback>,
    reasons: PauseReasons,
    requested_pause: bool,
    actual_pause: bool,
    loaded: bool,
    loading_since: Option<Instant>,
    recovery: Option<(Instant, u8, PathBuf)>,
    power: Vec<HPOWERNOTIFY>,
    tray_present: bool,
    error: Option<String>,
    library: Vec<PathBuf>,
    page: usize,
    settings_open: bool,
    thumbnail_rx: std::sync::mpsc::Receiver<(PathBuf, isize)>,
    thumbnail_tx: std::sync::mpsc::Sender<(PathBuf, isize)>,
}
impl App {
    fn control(&self, id: usize) -> HWND {
        self.controls
            .iter()
            .find(|c| c.0 == id)
            .map(|c| c.1)
            .unwrap_or_default()
    }
    fn text(&self, id: usize, text: &str) {
        let text = wide(text);
        // SAFETY: synchronous native text copy into owned control.
        unsafe {
            let _ = SetWindowTextW(self.control(id), PCWSTR(text.as_ptr()));
        }
    }
    fn set_error(&mut self, error: String) {
        self.error = Some(error);
        self.update_status();
    }
    fn screen(&self) -> &Monitor {
        self.monitors
            .iter()
            .find(|m| m.name == self.settings.monitor)
            .unwrap_or(&self.monitors[0])
    }
    fn save(&self) -> Result<(), String> {
        if !self.config_valid {
            return Err("Existing settings could not be read and were not overwritten. Repair or move settings.ini before saving again.".into());
        }
        let parent = self.config.parent().ok_or("Invalid settings directory")?;
        fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create the settings directory: {e}"))?;
        let content = self.settings.encode()?;
        let temporary = self.config.with_extension("tmp");
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .map_err(|e| format!("Could not save settings: {e}"))?;
        file.write_all(content.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("Could not write settings: {e}"))?;
        drop(file);
        let from = wide(temporary.to_str().ok_or("Invalid settings path")?);
        let to = wide(self.config.to_str().ok_or("Invalid settings path")?);
        // SAFETY: atomic write-through replacement only targets owned application settings.
        unsafe {
            MoveFileExW(
                PCWSTR(from.as_ptr()),
                PCWSTR(to.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(|e| format!("Previous settings were kept: {e}"))
    }
    fn create_controls(&mut self) -> Result<(), String> {
        // SAFETY: native widgets preserve accessibility/keyboard; GDI changes drawing only.
        unsafe {
            let instance = GetModuleHandleW(None).map_err(native_error)?;
            for (id, class, text, extra) in [
                (
                    FILE,
                    w!("STATIC"),
                    "No wallpaper selected",
                    SS_ENDELLIPSIS.0,
                ),
                (
                    PICK,
                    w!("BUTTON"),
                    "Add wallpaper",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    APPLY,
                    w!("BUTTON"),
                    "Set wallpaper",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    SCREEN,
                    w!("COMBOBOX"),
                    "",
                    CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0 | WS_TABSTOP.0,
                ),
                (
                    FIT,
                    w!("COMBOBOX"),
                    "",
                    CBS_DROPDOWNLIST as u32 | WS_TABSTOP.0,
                ),
                (
                    PAUSE,
                    w!("BUTTON"),
                    "Pause",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    STOP,
                    w!("BUTTON"),
                    "Stop",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    BATTERY,
                    w!("BUTTON"),
                    "Pause on battery",
                    BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0,
                ),
                (
                    STARTUP,
                    w!("BUTTON"),
                    "Start with Windows",
                    BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0,
                ),
                (
                    STATUS,
                    w!("EDIT"),
                    "",
                    ES_READONLY as u32 | ES_MULTILINE as u32 | WS_TABSTOP.0,
                ),
                (
                    SETTINGS,
                    w!("BUTTON"),
                    "Settings",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    visuals::PREVIEW,
                    w!("BUTTON"),
                    "Selected wallpaper preview",
                    BS_OWNERDRAW as u32 | BS_NOTIFY as u32,
                ),
                (
                    visuals::PREVIOUS,
                    w!("BUTTON"),
                    "Previous",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    visuals::NEXT,
                    w!("BUTTON"),
                    "Next",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    EXIT,
                    w!("BUTTON"),
                    "Exit",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
            ] {
                let text = wide(text);
                let child = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    class,
                    PCWSTR(text.as_ptr()),
                    WS_CHILD | WS_VISIBLE | WINDOW_STYLE(extra),
                    0,
                    0,
                    0,
                    0,
                    self.hwnd,
                    HMENU(id as *mut _),
                    instance,
                    None,
                )
                .map_err(native_error)?;
                if matches!(id, SCREEN | FIT) {
                    let _ = SetWindowTheme(child, w!("DarkMode_Explorer"), None);
                }
                self.controls.push((id, child));
            }
            for i in 0..6 {
                let child = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("BUTTON"),
                    w!("Local wallpaper"),
                    WS_CHILD | WINDOW_STYLE(BS_OWNERDRAW as u32) | WS_TABSTOP,
                    0,
                    0,
                    0,
                    0,
                    self.hwnd,
                    HMENU((visuals::TILE_BASE + i) as *mut _),
                    instance,
                    None,
                )
                .map_err(native_error)?;
                self.controls.push((visuals::TILE_BASE + i, child));
            }
            for label in ["Fill (crop to screen)", "Fit (show full frame)"] {
                let label = wide(label);
                SendMessageW(
                    self.control(FIT),
                    CB_ADDSTRING,
                    WPARAM(0),
                    LPARAM(label.as_ptr() as isize),
                );
            }
            SendMessageW(
                self.control(FIT),
                CB_SETCURSEL,
                WPARAM(usize::from(self.settings.fit == Fit::Contain)),
                LPARAM(0),
            );
            SendMessageW(
                self.control(BATTERY),
                BM_SETCHECK,
                WPARAM(self.settings.pause_battery as usize),
                LPARAM(0),
            );
            if let Ok(enabled) = startup_enabled() {
                SendMessageW(
                    self.control(STARTUP),
                    BM_SETCHECK,
                    WPARAM(enabled as usize),
                    LPARAM(0),
                );
            }
            self.refresh_monitors()?;
            self.refresh_library()?;
            self.layout()?;
            DragAcceptFiles(self.hwnd, true);
        }
        Ok(())
    }
    fn refresh_monitors(&mut self) -> Result<(), String> {
        let monitors = desktop::monitors()?;
        if !monitors.iter().any(|m| m.name == self.settings.monitor) {
            self.settings.monitor = monitors[0].name.clone();
        }
        self.monitors = monitors;
        // SAFETY: native combo owns copies of strings and uses stable indexes in current monitor list.
        unsafe {
            SendMessageW(self.control(SCREEN), CB_RESETCONTENT, WPARAM(0), LPARAM(0));
            for (i, monitor) in self.monitors.iter().enumerate() {
                let label = wide(&format!(
                    "{}  {}x{}{}",
                    monitor.name,
                    monitor.rect.right - monitor.rect.left,
                    monitor.rect.bottom - monitor.rect.top,
                    if monitor.primary { " (primary)" } else { "" }
                ));
                SendMessageW(
                    self.control(SCREEN),
                    CB_ADDSTRING,
                    WPARAM(0),
                    LPARAM(label.as_ptr() as isize),
                );
                if monitor.name == self.settings.monitor {
                    SendMessageW(self.control(SCREEN), CB_SETCURSEL, WPARAM(i), LPARAM(0));
                }
            }
        }
        Ok(())
    }
    fn layout(&mut self) -> Result<(), String> {
        // SAFETY: layout uses logical pixels scaled once; native control HWNDs belong to App.
        unsafe {
            let dpi = GetDpiForWindow(self.hwnd).max(96) as i32;
            let scale = |n: i32| n * dpi / 96;
            let font = CreateFontW(
                -scale(14),
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                0,
                0,
                CLEARTYPE_QUALITY.0 as u32,
                0,
                w!("Segoe UI"),
            );
            if font.0.is_null() {
                return Err("Could not create the Windows font".into());
            }
            for (_, control) in &self.controls {
                SendMessageW(*control, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
            }
            if !self.font.0.is_null() {
                let _ = DeleteObject(self.font);
            }
            self.font = font;
            let mut r = RECT::default();
            GetClientRect(self.hwnd, &mut r).map_err(native_error)?;
            let width = r.right * 96 / dpi;
            let height = r.bottom * 96 / dpi;
            let split = width - 344;
            let tile_width = (split - 72) / 2;
            let tile_height = ((height - 326) / 2).clamp(170, 210);
            let layout = [
                (PICK, width - 330, 20, 188, 36),
                (SETTINGS, width - 128, 20, 100, 36),
                (visuals::PREVIEW, split + 24, 143, 296, 177),
                (FILE, split + 24, 334, 296, 24),
                (SCREEN, split + 24, 403, 296, 220),
                (FIT, split + 24, 481, 296, 180),
                (APPLY, split + 24, 538, 296, 42),
                (PAUSE, split + 24, 594, 142, 36),
                (STOP, split + 178, 594, 142, 36),
                (BATTERY, 28, 316, split - 56, 36),
                (STARTUP, 28, 366, split - 56, 36),
                (visuals::PREVIOUS, 28, height - 136, 112, 34),
                (visuals::NEXT, 152, height - 136, 112, 34),
                (EXIT, width - 108, height - 60, 80, 36),
                (STATUS, 28, height - 68, width - 164, 56),
            ];
            for (id, x, y, w, h) in layout {
                SetWindowPos(
                    self.control(id),
                    None,
                    scale(x),
                    scale(y),
                    scale(w),
                    scale(h),
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
                .map_err(native_error)?;
            }
            for i in 0..6 {
                let control = self.control(visuals::TILE_BASE + i);
                let x = 28 + (i % 2) as i32 * (tile_width + 16);
                let y = 183 + (i / 2) as i32 * (tile_height + 18);
                SetWindowPos(
                    control,
                    None,
                    scale(x),
                    scale(y),
                    scale(tile_width),
                    scale(tile_height),
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
                .map_err(native_error)?;
                let visible =
                    !self.settings_open && i < 4 && self.library.get(self.page * 4 + i).is_some();
                let _ = ShowWindow(control, if visible { SW_SHOW } else { SW_HIDE });
            }
            for id in [BATTERY, STARTUP] {
                let _ = ShowWindow(
                    self.control(id),
                    if self.settings_open { SW_SHOW } else { SW_HIDE },
                );
            }
            for id in [visuals::PREVIOUS, visuals::NEXT] {
                let _ = ShowWindow(
                    self.control(id),
                    if !self.settings_open && self.library.len() > 4 {
                        SW_SHOW
                    } else {
                        SW_HIDE
                    },
                );
            }
            let _ = EnableWindow(self.control(visuals::PREVIOUS), self.page > 0);
            let _ = EnableWindow(
                self.control(visuals::NEXT),
                (self.page + 1) * 4 < self.library.len(),
            );
            let _ = InvalidateRect(self.hwnd, None, false);
        }
        Ok(())
    }
    fn refresh_library(&mut self) -> Result<(), String> {
        let start = self.page * 4;
        let paths: Vec<_> = self.library.iter().skip(start).take(4).cloned().collect();
        visuals::tiles(&paths);
        visuals::selected(self.selected.clone());
        for (i, path) in paths.iter().enumerate() {
            self.text(
                visuals::TILE_BASE + i,
                &path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Wallpaper".into()),
            );
        }
        self.text(
            FILE,
            &self
                .selected
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "No wallpaper selected".into()),
        );
        unsafe {
            let _ = EnableWindow(self.control(APPLY), self.selected.is_some());
            let _ = InvalidateRect(self.hwnd, None, false);
            for (_, h) in &self.controls {
                let _ = InvalidateRect(*h, None, false);
            }
        }
        self.layout()
    }
    fn add_media(&mut self, path: PathBuf) -> Result<(), String> {
        let path = validate_media(&path)?;
        self.library.retain(|p| p != &path);
        self.library.insert(0, path.clone());
        self.library.truncate(12);
        self.page = 0;
        self.selected = Some(path.clone());
        self.error = None;
        self.refresh_library()?;
        let mut recent = String::from("version=1\n");
        for item in &self.library {
            recent.push_str(item.to_str().ok_or("File name is not valid Unicode")?);
            recent.push('\n');
        }
        if let Some(parent) = self.config.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            fs::write(parent.join("recent.txt"), recent)
                .map_err(|e| format!("Could not save the library: {e}"))?;
        }
        self.request_thumbnail(path);
        self.update_status();
        Ok(())
    }
    fn request_thumbnail(&self, path: PathBuf) {
        let tx = self.thumbnail_tx.clone();
        let hwnd = self.hwnd.0 as usize;
        // Worker performs optional shell thumbnail work; it never receives a Rust GUI reference.
        std::thread::spawn(move || {
            let result = unsafe {
                if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
                    return;
                }
                let name = wide(path.to_str().unwrap_or(""));
                let image = (|| {
                    let item: IShellItemImageFactory =
                        SHCreateItemFromParsingName(PCWSTR(name.as_ptr()), None)?;
                    item.GetImage(
                        SIZE { cx: 640, cy: 360 },
                        SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK,
                    )
                })();
                CoUninitialize();
                image
            };
            if let Ok(image) = result {
                if tx.send((path, image.0 as isize)).is_err() {
                    unsafe {
                        let _ = DeleteObject(image);
                    }
                    return;
                }
                unsafe {
                    let _ = PostMessageW(HWND(hwnd as *mut _), THUMBNAIL, WPARAM(0), LPARAM(0));
                }
            }
        });
    }
    fn tray(&mut self, add: bool) -> Result<(), String> {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: TRAY_EVENT,
            ..Default::default()
        };
        // SAFETY: shared system icon must not be destroyed; notification structure copied synchronously.
        unsafe {
            data.hIcon = LoadIconW(None, IDI_APPLICATION).map_err(native_error)?;
        }
        let tip = wide("WallpaperLive controls");
        data.szTip[..tip.len()].copy_from_slice(&tip);
        if !unsafe { Shell_NotifyIconW(if add { NIM_ADD } else { NIM_DELETE }, &data) }.as_bool() {
            return Err(
                "Windows tray is unavailable. Keep this window open to control playback.".into(),
            );
        }
        self.tray_present = add;
        Ok(())
    }
    fn show(&self) {
        // SAFETY: own top-level panel only; focus request may be refused by Windows normally.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(self.control(PICK));
        }
    }
    fn tray_menu(&mut self) -> Result<bool, String> {
        // SAFETY: native menu is synchronously tracked and destroyed before returning.
        unsafe {
            let menu = CreatePopupMenu().map_err(native_error)?;
            let result = (|| {
                for (id, label) in [
                    (OPEN, "Open controls"),
                    (
                        PAUSE,
                        if self.reasons.manual {
                            "Resume"
                        } else {
                            "Pause"
                        },
                    ),
                    (STOP, "Stop wallpaper"),
                    (EXIT, "Exit"),
                ] {
                    let text = wide(label);
                    AppendMenuW(menu, MF_STRING, id, PCWSTR(text.as_ptr()))
                        .map_err(native_error)?;
                }
                let mut point = POINT::default();
                GetCursorPos(&mut point).map_err(native_error)?;
                let _ = SetForegroundWindow(self.hwnd);
                let id = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_RIGHTBUTTON,
                    point.x,
                    point.y,
                    0,
                    self.hwnd,
                    None,
                )
                .0 as usize;
                self.action(id, 0)
            })();
            let _ = DestroyMenu(menu);
            result
        }
    }
    fn release_playback(&mut self) {
        self.playback = None;
        self.loaded = false;
        self.loading_since = None;
        self.requested_pause = false;
        self.actual_pause = false;
        // SAFETY: teardown cancels only the timer owned by this controller.
        unsafe {
            let _ = KillTimer(self.hwnd, 1);
        }
    }
    fn start(&mut self) -> Result<(), String> {
        let path = self
            .selected
            .clone()
            .ok_or("Select a wallpaper file first")?;
        self.start_path(&path)
    }
    fn start_path(&mut self, source: &Path) -> Result<(), String> {
        let path = validate_media(source)?;
        let root = wide(&format!(
            "{}\\",
            &path.to_str().ok_or("Invalid file name")?[..2]
        ));
        // SAFETY: canonical local drive root queried without changing filesystem.
        if !matches!(unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) }, 2 | 3 | 5) {
            return Err("Media must be on a local drive, not a network drive".into());
        }
        self.release_playback();
        let surface = Surface::create(self.screen().rect)?;
        let player = Mpv::open_at(&decoder_path()?, surface.hwnd, self.hwnd, self.settings.fit)?;
        self.update_reasons();
        let pause = self.reasons.reason().is_some();
        player.pause(pause)?;
        player.load(&path)?;
        self.playback = Some(Playback {
            player,
            surface,
            path,
        });
        self.requested_pause = pause;
        self.loading_since = Some(Instant::now());
        self.error = None;
        // SAFETY: timer exists only while playback/recovery needs low-rate monitoring, never drives rendering.
        unsafe {
            SetTimer(self.hwnd, 1, 500, None);
        }
        self.update_status();
        Ok(())
    }
    fn stop(&mut self) -> Result<(), String> {
        self.recovery = None;
        self.release_playback();
        self.reasons.manual = false;
        self.settings.path = None;
        unsafe {
            let _ = KillTimer(self.hwnd, 1);
        }
        self.save()?;
        self.error = None;
        self.update_status();
        Ok(())
    }
    fn update_reasons(&mut self) {
        let mut status = SYSTEM_POWER_STATUS::default();
        // SAFETY: valid output buffer, read-only system state.
        if unsafe { GetSystemPowerStatus(&mut status) }.is_ok() {
            self.reasons.battery = self.settings.pause_battery && status.ACLineStatus == 0;
        }
        // SAFETY: window handles queried only; never injects into foreground process.
        unsafe {
            self.reasons.remote = GetSystemMetrics(SM_REMOTESESSION) != 0;
            let mut protocol = windows::core::PWSTR::null();
            let mut bytes = 0;
            if WTSQuerySessionInformationW(
                WTS_CURRENT_SERVER_HANDLE,
                WTS_CURRENT_SESSION,
                WTSClientProtocolType,
                &mut protocol,
                &mut bytes,
            )
            .is_ok()
            {
                if bytes >= 2 && !protocol.0.is_null() {
                    self.reasons.remote |= std::ptr::read_unaligned(protocol.0) != 0;
                }
                WTSFreeMemory(protocol.0.cast());
            }
            let mut state = windows::core::PWSTR::null();
            bytes = 0;
            if WTSQuerySessionInformationW(
                WTS_CURRENT_SERVER_HANDLE,
                WTS_CURRENT_SESSION,
                WTSConnectState,
                &mut state,
                &mut bytes,
            )
            .is_ok()
            {
                if bytes >= 4 && !state.0.is_null() {
                    self.reasons.disconnected =
                        std::ptr::read_unaligned(state.0.cast::<u32>()) != 0;
                }
                WTSFreeMemory(state.0.cast());
            }
            let foreground = GetForegroundWindow();
            let mut pid = 0;
            GetWindowThreadProcessId(foreground, Some(&mut pid));
            let mut rect = RECT::default();
            let mut class = [0u16; 64];
            let n = GetClassNameW(foreground, &mut class).max(0) as usize;
            let class = String::from_utf16_lossy(&class[..n]);
            self.reasons.fullscreen = pid != std::process::id()
                && !foreground.0.is_null()
                && !IsIconic(foreground).as_bool()
                && !matches!(
                    class.as_str(),
                    "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
                )
                && GetWindowRect(foreground, &mut rect).is_ok()
                && Rect {
                    left: rect.left,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                }
                .covers(self.screen().rect);
        }
    }
    fn arm_monitor(&self) -> Result<(), String> {
        // SAFETY: timer belongs to controller, never drives video or thumbnail redraw.
        if unsafe { SetTimer(self.hwnd, 1, 500, None) } == 0 {
            return Err("Could not start the playback monitor".into());
        }
        Ok(())
    }
    fn tick(&mut self) -> Result<(), String> {
        if self
            .recovery
            .as_ref()
            .is_some_and(|(due, _, _)| Instant::now() >= *due)
            && let Some((_, tries, path)) = self.recovery.take()
        {
            match self.start_path(&path) {
                Ok(()) => {}
                Err(error) if tries < 2 => {
                    self.recovery = Some((
                        Instant::now() + Duration::from_millis(1200),
                        tries + 1,
                        path,
                    ));
                    self.error = Some(error);
                    self.arm_monitor()?;
                }
                Err(error) => {
                    unsafe {
                        let _ = KillTimer(self.hwnd, 1);
                    }
                    return Err(format!(
                        "Desktop recovery failed after 3 attempts. Set the wallpaper again when Explorer is ready. {error}"
                    ));
                }
            }
        }
        if let Some(playback) = self.playback.as_ref()
            && !playback.surface.valid()
        {
            let path = playback.path.clone();
            self.release_playback();
            self.recovery = Some((Instant::now() + Duration::from_millis(500), 0, path));
            self.arm_monitor()?;
            self.update_status();
            return Ok(());
        }
        self.update_reasons();
        let paused = self.reasons.reason().is_some();
        if paused != self.requested_pause
            && let Some(playback) = &self.playback
        {
            playback.player.pause(paused)?;
            self.requested_pause = paused;
        }
        if self
            .loading_since
            .is_some_and(|s| s.elapsed() > Duration::from_secs(15))
        {
            self.release_playback();
            return Err("Media did not finish loading. Try another file or decoder.".into());
        }
        self.events()?;
        self.update_status();
        Ok(())
    }
    fn events(&mut self) -> Result<(), String> {
        let events = self
            .playback
            .as_ref()
            .map(|p| p.player.poll())
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Loaded => {}
                Event::Ready => {
                    if self.loaded {
                        continue;
                    }
                    let Some(playback) = self.playback.as_ref() else {
                        continue;
                    };
                    if playback.player.property(c"video-format").is_none() {
                        continue;
                    }
                    if let Err(error) = playback.surface.prepare_renderer() {
                        self.release_playback();
                        return Err(error);
                    }
                    playback.surface.show();
                    self.settings.path = Some(playback.path.clone());
                    self.loaded = true;
                    self.loading_since = None;
                    if let Err(error) = self.save() {
                        self.error = Some(format!("Playing, but settings were not saved: {error}"));
                    }
                }
                Event::Failed(error) => {
                    self.release_playback();
                    unsafe {
                        let _ = KillTimer(self.hwnd, 1);
                    }
                    return Err(error);
                }
                Event::Pause(paused) => self.actual_pause = paused,
            }
        }
        self.update_status();
        Ok(())
    }
    fn update_status(&self) {
        let status = if let Some(error) = &self.error {
            format!("Error: {error}")
        } else if self.recovery.is_some() {
            "Desktop changed. Restoring wallpaper...".into()
        } else if self.playback.is_none() {
            "Choose a file from your library, then Set wallpaper.".into()
        } else if !self.loaded {
            "Loading media...".into()
        } else if let Some(reason) = self.reasons.reason() {
            reason.into()
        } else if self.actual_pause {
            "Resuming playback...".into()
        } else {
            "Playing muted. Auto-pause is enabled for fullscreen apps.".into()
        };
        if window_text(self.control(STATUS)) != status {
            self.text(STATUS, &status);
        }
        let label = if self.reasons.manual {
            "Resume"
        } else {
            "Pause"
        };
        if window_text(self.control(PAUSE)) != label {
            self.text(PAUSE, label);
        }
        unsafe {
            let _ = EnableWindow(self.control(PAUSE), self.playback.is_some());
            let _ = EnableWindow(
                self.control(STOP),
                self.playback.is_some() || self.recovery.is_some(),
            );
        }
    }
    fn pick(&mut self) -> Result<(), String> {
        let mut file = vec![0u16; 32768];
        let filter = wide(
            "Wallpaper media\0*.mp4;*.gif;*.mkv;*.webm;*.mov;*.avi;*.mpg;*.mpeg;*.m4v;*.apng;*.png;*.jpg;*.jpeg;*.bmp;*.webp\0\0",
        );
        let mut dialog = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: self.hwnd,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: windows::core::PWSTR(file.as_mut_ptr()),
            nMaxFile: file.len() as u32,
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER,
            ..Default::default()
        };
        if unsafe { GetOpenFileNameW(&mut dialog) }.as_bool() {
            let end = file.iter().position(|n| *n == 0).unwrap_or(file.len());
            let path = validate_media(Path::new(&String::from_utf16_lossy(&file[..end])))?;
            self.add_media(path)?;
        } else {
            let error = unsafe { CommDlgExtendedError() };
            if error.0 != 0 {
                return Err(format!("File picker failed ({})", error.0));
            }
        }
        Ok(())
    }
    fn action(&mut self, id: usize, notification: u32) -> Result<bool, String> {
        match id {
            SETTINGS => {
                self.settings_open = !self.settings_open;
                visuals::settings(self.settings_open);
                self.layout()?;
            }
            visuals::PREVIOUS => {
                self.page = self.page.saturating_sub(1);
                self.refresh_library()?;
            }
            visuals::NEXT => {
                if (self.page + 1) * 4 < self.library.len() {
                    self.page += 1;
                    self.refresh_library()?;
                }
            }
            id if (visuals::TILE_BASE..visuals::TILE_BASE + 4).contains(&id) => {
                if let Some(path) = self
                    .library
                    .get(self.page * 4 + id - visuals::TILE_BASE)
                    .cloned()
                {
                    self.selected = Some(path.clone());
                    self.error = None;
                    self.refresh_library()?;
                    self.request_thumbnail(path);
                }
            }
            PICK => self.pick()?,
            APPLY => {
                self.recovery = None;
                self.start()?;
            }
            PAUSE => {
                self.reasons.manual = !self.reasons.manual;
                self.tick()?;
            }
            STOP => self.stop()?,
            OPEN => self.show(),
            EXIT => return Ok(true),
            HIDE => {
                if self.tray_present {
                    unsafe {
                        let _ = ShowWindow(self.hwnd, SW_HIDE);
                    }
                } else {
                    return Ok(true);
                }
            }
            SCREEN if notification == CBN_SELCHANGE => {
                let selected = unsafe {
                    SendMessageW(self.control(SCREEN), CB_GETCURSEL, WPARAM(0), LPARAM(0)).0
                };
                if let Some(monitor) = self.monitors.get(selected as usize) {
                    self.settings.monitor = monitor.name.clone();
                }
                if let Some(playback) = &self.playback {
                    playback.surface.place(self.screen().rect)?;
                }
                self.save()?;
                self.tick()?;
            }
            FIT if notification == CBN_SELCHANGE => {
                self.settings.fit = if unsafe {
                    SendMessageW(self.control(FIT), CB_GETCURSEL, WPARAM(0), LPARAM(0)).0
                } == 0
                {
                    Fit::Fill
                } else {
                    Fit::Contain
                };
                if let Some(playback) = &self.playback {
                    playback.player.set_fit(self.settings.fit)?;
                }
                self.save()?;
            }
            BATTERY => {
                self.settings.pause_battery = unsafe {
                    SendMessageW(self.control(BATTERY), BM_GETCHECK, WPARAM(0), LPARAM(0)).0
                } == BST_CHECKED.0 as isize;
                self.save()?;
                self.tick()?;
            }
            STARTUP => {
                let enabled = unsafe {
                    SendMessageW(self.control(STARTUP), BM_GETCHECK, WPARAM(0), LPARAM(0)).0
                } == 1;
                if let Err(error) = set_startup(enabled) {
                    unsafe {
                        SendMessageW(
                            self.control(STARTUP),
                            BM_SETCHECK,
                            WPARAM(!enabled as usize),
                            LPARAM(0),
                        );
                    }
                    return Err(error);
                }
            }
            _ => {}
        }
        Ok(false)
    }
}
impl Drop for App {
    fn drop(&mut self) {
        self.release_playback();
        // SAFETY: unregister only notifications/timer/tray and HWND owned by this App.
        unsafe {
            let _ = KillTimer(self.hwnd, 1);
            let _ = WTSUnRegisterSessionNotification(self.hwnd);
            for notification in self.power.drain(..) {
                let _ = UnregisterPowerSettingNotification(notification);
            }
            if self.tray_present {
                let _ = self.tray(false);
            }
            if IsWindow(self.hwnd).as_bool() {
                let _ = DestroyWindow(self.hwnd);
            }
            if !self.font.0.is_null() {
                let _ = DeleteObject(self.font);
            }
            visuals::clear();
            for (_, bitmap) in self.thumbnail_rx.try_iter() {
                let _ = DeleteObject(HBITMAP(bitmap as *mut _));
            }
        }
    }
}
fn window_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd).clamp(0, 32768) as usize;
        let mut text = vec![0u16; len + 1];
        let n = GetWindowTextW(hwnd, &mut text).max(0) as usize;
        String::from_utf16_lossy(&text[..n])
    }
}
fn decoder_path() -> Result<PathBuf, String> {
    let directory = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("Application directory not found")?
        .to_owned();
    let mut candidates = vec![
        directory.join("runtime/libmpv-2.dll"),
        directory.join("libmpv-2.dll"),
    ];
    if cfg!(debug_assertions) {
        candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime/libmpv-2.dll"));
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or("Decoder is missing. Run scripts/Get-Decoder.ps1 once.".into())
}
struct RegKey(HKEY);
impl Drop for RegKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}
fn startup_command() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(format!("\"{}\" --background", exe.display()))
}
fn startup_key() -> Result<RegKey, String> {
    let mut key = HKEY::default();
    unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            0,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            &mut key,
        )
    }
    .ok()
    .map_err(native_error)?;
    Ok(RegKey(key))
}
fn startup_value(key: &RegKey) -> Result<Option<String>, String> {
    let mut size = 0;
    let mut kind = REG_VALUE_TYPE::default();
    let result = unsafe {
        RegQueryValueExW(
            key.0,
            w!("WallpaperLive"),
            None,
            Some(&mut kind),
            None,
            Some(&mut size),
        )
    };
    if result == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    result.ok().map_err(native_error)?;
    if kind != REG_SZ || size > 65536 {
        return Err(
            "The existing WallpaperLive startup entry has an unknown type and was not overwritten"
                .into(),
        );
    }
    let mut bytes = vec![0u8; size as usize];
    unsafe {
        RegQueryValueExW(
            key.0,
            w!("WallpaperLive"),
            None,
            None,
            Some(bytes.as_mut_ptr()),
            Some(&mut size),
        )
    }
    .ok()
    .map_err(native_error)?;
    let units: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u16::from_le_bytes(*p))
        .take_while(|u| *u != 0)
        .collect();
    Ok(Some(String::from_utf16_lossy(&units)))
}
fn startup_enabled() -> Result<bool, String> {
    let key = startup_key()?;
    Ok(startup_value(&key)?.as_deref() == Some(&startup_command()?))
}
fn set_startup(enabled: bool) -> Result<(), String> {
    let key = startup_key()?;
    let expected = startup_command()?;
    if startup_value(&key)?.is_some_and(|current| current != expected) {
        return Err(
            "A WallpaperLive startup entry belongs to another installation and was not overwritten"
                .into(),
        );
    }
    if enabled {
        let value = wide(&expected);
        let bytes: Vec<_> = value.iter().flat_map(|n| n.to_le_bytes()).collect();
        unsafe { RegSetValueExW(key.0, w!("WallpaperLive"), 0, REG_SZ, Some(&bytes)) }
            .ok()
            .map_err(native_error)?;
    } else {
        let status = unsafe { RegDeleteValueW(key.0, w!("WallpaperLive")) };
        if status != ERROR_FILE_NOT_FOUND {
            status.ok().map_err(native_error)?;
        }
    }
    Ok(())
}

pub fn run() -> Result<(), String> {
    // SAFETY: all GUI work below belongs to this UI thread; no Rust pointers shared through WndProc.
    unsafe {
        let mutex =
            CreateMutexW(None, false, w!("Local\\Raflyf.WallpaperLive")).map_err(native_error)?;
        let duplicate = GetLastError() == ERROR_ALREADY_EXISTS;
        let _instance = Handle(mutex);
        if duplicate {
            if let Ok(window) = FindWindowW(CLASS, None) {
                let _ = PostMessageW(window, ACTION, WPARAM(OPEN), LPARAM(0));
            }
            return Ok(());
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES,
        });
        let instance = GetModuleHandleW(None).map_err(native_error)?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).map_err(native_error)?,
            hIcon: LoadIconW(None, IDI_APPLICATION).map_err(native_error)?,
            hbrBackground: visuals::brush(visuals::CANVAS),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err("Could not register the control window".into());
        }
        let dpi = GetDpiForSystem() as i32;
        let hwnd = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            CLASS,
            w!("WallpaperLive"),
            (WS_OVERLAPPEDWINDOW & !WS_MAXIMIZEBOX) | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1100 * dpi / 96,
            780 * dpi / 96,
            None,
            None,
            instance,
            None,
        )
        .map_err(native_error)?;
        let data = std::env::var_os("WALLPAPERLIVE_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("WallpaperLive"))
            })
            .ok_or("Windows data directory is unavailable")?;
        let config = data.join("settings.ini");
        let (settings, error, config_valid) = match fs::read_to_string(&config) {
            Ok(text) => match Settings::decode(&text) {
                Ok(settings) => (settings, None, true),
                Err(e) => (Settings::default(), Some(e), false),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None, true),
            Err(e) => (
                Settings::default(),
                Some(format!("Could not read settings: {e}")),
                false,
            ),
        };
        let selected = settings.path.clone();
        let library = fs::read_to_string(config.with_file_name("recent.txt"))
            .ok()
            .map(|t| {
                t.lines()
                    .skip(1)
                    .filter_map(|p| validate_media(Path::new(p)).ok())
                    .take(12)
                    .collect()
            })
            .unwrap_or_else(Vec::new);
        let (thumbnail_tx, thumbnail_rx) = std::sync::mpsc::channel();
        let mut app = App {
            hwnd,
            controls: vec![],
            font: HFONT::default(),
            monitors: desktop::monitors()?,
            settings,
            selected,
            config,
            config_valid,
            playback: None,
            reasons: PauseReasons::default(),
            requested_pause: false,
            actual_pause: false,
            loaded: false,
            loading_since: None,
            recovery: None,
            power: vec![],
            tray_present: false,
            error,
            library,
            page: 0,
            settings_open: false,
            thumbnail_tx,
            thumbnail_rx,
        };
        let dark = 1i32;
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark as *const i32).cast(),
            4,
        );
        app.create_controls()?;
        for path in app.library.clone() {
            app.request_thumbnail(path);
        }
        if let Err(e) = app.tray(true) {
            app.set_error(e);
        }
        if let Err(e) = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) {
            app.set_error(format!("Session notifications are unavailable: {e}"));
        }
        for guid in [&GUID_SESSION_DISPLAY_STATUS, &GUID_ACDC_POWER_SOURCE] {
            match RegisterPowerSettingNotification(hwnd, guid, DEVICE_NOTIFY_WINDOW_HANDLE) {
                Ok(handle) => app.power.push(handle),
                Err(e) => app.set_error(format!("Power notifications are unavailable: {e}")),
            }
        }
        TASKBAR.store(
            RegisterWindowMessageW(w!("TaskbarCreated")),
            std::sync::atomic::Ordering::Relaxed,
        );
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        let background = args.iter().any(|a| a == "--background");
        if let Some(path) = args.iter().find(|a| *a != "--background") {
            app.selected = Some(PathBuf::from(path));
        }
        if app.selected.is_some()
            && let Err(e) = app.start()
        {
            app.set_error(e);
        }
        if !background || app.error.is_some() || !app.tray_present {
            app.show();
        }
        app.update_status();
        let mut message = MSG::default();
        loop {
            let result = GetMessageW(&mut message, None, 0, 0).0;
            if result == 0 {
                break;
            }
            if result < 0 {
                return Err("Windows message loop failed".into());
            }
            let _ = TranslateMessage(&message);
            if (WM_KEYFIRST..=WM_KEYLAST).contains(&message.message)
                && IsDialogMessageW(hwnd, &message).as_bool()
            {
                continue;
            }
            let handled = message.hwnd == hwnd;
            let action: Result<bool, String> = if handled {
                match message.message {
                    ACTION => app.action(
                        message.wParam.0 & 0xffff,
                        ((message.wParam.0 >> 16) & 0xffff) as u32,
                    ),
                    THUMBNAIL => {
                        for (path, bitmap) in app.thumbnail_rx.try_iter() {
                            visuals::thumbnail(path, HBITMAP(bitmap as *mut _));
                        }
                        app.refresh_library().map(|_| false)
                    }
                    LAYOUT => app.layout().map(|_| false),
                    mpv::WM_MPV => app.events().map(|_| false),
                    WM_TIMER => app.tick().map(|_| false),
                    POWER => {
                        match message.wParam.0 {
                            1 => app.update_reasons(),
                            2 => app.reasons.display_off = message.lParam.0 == 0,
                            3 => app.reasons.suspended = message.lParam.0 != 0,
                            _ => {}
                        }
                        app.tick().map(|_| false)
                    }
                    SESSION => {
                        app.reasons.session_event(message.wParam.0 as u32);
                        app.tick().map(|_| false)
                    }
                    DISPLAY => {
                        if let Err(e) = app.refresh_monitors() {
                            app.set_error(e);
                        }
                        if app.tray_present {
                            let _ = app.tray(true);
                        }
                        if let Some(p) = &app.playback
                            && let Err(e) = p.surface.place(app.screen().rect)
                        {
                            let path = p.path.clone();
                            app.release_playback();
                            app.recovery =
                                Some((Instant::now() + Duration::from_millis(500), 0, path));
                            if let Err(error) = app.arm_monitor() {
                                app.set_error(error);
                            }
                            app.set_error(e);
                        }
                        Ok(false)
                    }
                    TRAY => {
                        if message.lParam.0 as u32 == WM_RBUTTONUP {
                            app.tray_menu()
                        } else {
                            app.show();
                            Ok(false)
                        }
                    }
                    DROP => {
                        let drop = HDROP(message.wParam.0 as *mut _);
                        let mut name = vec![0u16; 32768];
                        let n = DragQueryFileW(drop, 0, Some(&mut name));
                        DragFinish(drop);
                        if n > 0 {
                            let path = PathBuf::from(String::from_utf16_lossy(&name[..n as usize]));
                            app.add_media(path).map(|_| false)
                        } else {
                            Err("Could not read the dropped file".into())
                        }
                    }
                    _ => {
                        if !IsDialogMessageW(hwnd, &message).as_bool() {
                            let _ = TranslateMessage(&message);
                            DispatchMessageW(&message);
                        }
                        Ok(false)
                    }
                }
            } else {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
                Ok(false)
            };
            match action {
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => app.set_error(error),
            }
        }
    }
    Ok(())
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: no Rust state references; transient power/DPI payloads read only during native dispatch.
    unsafe {
        let post = |message: u32, wparam: WPARAM, lparam: LPARAM| {
            let _ = PostMessageW(window, message, wparam, lparam);
        };
        match message {
            WM_PAINT => {
                visuals::paint(window);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_DRAWITEM if lparam.0 != 0 => {
                visuals::draw_item(&*(lparam.0 as *const DRAWITEMSTRUCT));
                LRESULT(1)
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
                visuals::control_colors(message, HDC(wparam.0 as *mut _), HWND(lparam.0 as *mut _))
                    .unwrap_or_else(|| DefWindowProcW(window, message, wparam, lparam))
            }
            WM_COMMAND => {
                post(ACTION, wparam, lparam);
                LRESULT(0)
            }
            WM_CLOSE => {
                post(ACTION, WPARAM(HIDE), LPARAM(0));
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_SIZE => {
                post(LAYOUT, WPARAM(0), LPARAM(0));
                LRESULT(0)
            }
            WM_DPICHANGED => {
                if lparam.0 != 0 {
                    let rect = *(lparam.0 as *const RECT);
                    let _ = SetWindowPos(
                        window,
                        None,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                post(LAYOUT, WPARAM(0), LPARAM(0));
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                if lparam.0 != 0 {
                    let info = &mut *(lparam.0 as *mut MINMAXINFO);
                    let dpi = GetDpiForWindow(window).max(96) as i32;
                    info.ptMinTrackSize = POINT {
                        x: 930 * dpi / 96,
                        y: 750 * dpi / 96,
                    };
                }
                LRESULT(0)
            }
            WM_DISPLAYCHANGE => {
                post(DISPLAY, WPARAM(0), LPARAM(0));
                LRESULT(0)
            }
            WM_DROPFILES => {
                post(DROP, wparam, lparam);
                LRESULT(0)
            }
            TRAY_EVENT => {
                post(TRAY, wparam, lparam);
                LRESULT(0)
            }
            WM_WTSSESSION_CHANGE => {
                post(SESSION, wparam, lparam);
                LRESULT(0)
            }
            WM_POWERBROADCAST => {
                match wparam.0 as u32 {
                    PBT_POWERSETTINGCHANGE if lparam.0 != 0 => {
                        let setting = &*(lparam.0 as *const POWERBROADCAST_SETTING);
                        if setting.DataLength == 4 {
                            let value =
                                std::ptr::read_unaligned(setting.Data.as_ptr().cast::<u32>());
                            if setting.PowerSetting == GUID_SESSION_DISPLAY_STATUS {
                                post(POWER, WPARAM(2), LPARAM(value as isize));
                            } else if setting.PowerSetting == GUID_ACDC_POWER_SOURCE {
                                post(POWER, WPARAM(1), LPARAM(0));
                            }
                        }
                    }
                    PBT_APMSUSPEND => post(POWER, WPARAM(3), LPARAM(1)),
                    PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => {
                        post(POWER, WPARAM(3), LPARAM(0))
                    }
                    PBT_APMPOWERSTATUSCHANGE => post(POWER, WPARAM(1), LPARAM(0)),
                    _ => {}
                }
                LRESULT(1)
            }
            _ if message == TASKBAR.load(std::sync::atomic::Ordering::Relaxed) && message != 0 => {
                post(DISPLAY, WPARAM(0), LPARAM(0));
                LRESULT(0)
            }
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_media_preserves_preview_and_last_good_recovery() {
        let active = PathBuf::from(r"C:\wallpaperlive-missing-regression-87b81342\active.mp4");
        let preview = PathBuf::from(r"C:\wallpaperlive-missing-regression-87b81342\preview.mp4");
        let settings = Settings {
            path: Some(active.clone()),
            ..Settings::default()
        };
        let (thumbnail_tx, thumbnail_rx) = std::sync::mpsc::channel();
        // No windows or decoder are created: every exercised path fails validation first.
        let mut app = App {
            hwnd: HWND::default(),
            controls: vec![],
            font: HFONT::default(),
            monitors: vec![],
            settings: settings.clone(),
            selected: Some(preview.clone()),
            config: PathBuf::new(),
            config_valid: true,
            playback: None,
            reasons: PauseReasons::default(),
            requested_pause: false,
            actual_pause: false,
            loaded: false,
            loading_since: None,
            recovery: None,
            power: vec![],
            tray_present: false,
            error: None,
            library: vec![],
            page: 0,
            settings_open: false,
            thumbnail_rx,
            thumbnail_tx,
        };
        assert!(
            app.add_media(PathBuf::from("https://example.invalid/file.mp4"))
                .is_err()
        );
        assert!(app.start_path(&active).is_err());
        assert!(app.library.is_empty());
        assert_eq!(app.selected.as_ref(), Some(&preview));
        assert_eq!(app.settings, settings);
        app.recovery = Some((Instant::now(), 2, active));
        let error = app.tick().unwrap_err();
        assert!(error.contains("3 attempts"));
        assert!(app.recovery.is_none());
        assert_eq!(app.selected.as_ref(), Some(&preview));
        assert_eq!(app.settings, settings);
    }
}
