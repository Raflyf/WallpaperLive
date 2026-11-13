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
            Com::*, LibraryLoader::GetModuleHandleW, Power::*, ProcessStatus::*, Registry::*,
            RemoteDesktop::*, SystemServices::*, Threading::*,
        },
        UI::{
            Controls::{
                BST_CHECKED, DRAWITEMSTRUCT, Dialogs::*, ICC_STANDARD_CLASSES,
                INITCOMMONCONTROLSEX, InitCommonControlsEx,
            },
            HiDpi::*,
            Input::KeyboardAndMouse::{
                EnableWindow, GetDoubleClickTime, GetFocus, GetKeyState, SetFocus, VK_CONTROL,
                VK_ESCAPE, VK_LEFT, VK_RIGHT, VK_SPACE,
            },
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
const THUMBNAIL: u32 = WM_APP + 9;
const PREPARED: u32 = WM_APP + 10;
const UPDATE_READY: u32 = WM_APP + 11;
const UPDATE_RELEASE: usize = 126;
const CONTEXT_MENU: u32 = WM_APP + 12;
const CTX_SET: usize = 201;
const CTX_EXPLORER: usize = 202;
const CTX_COPY_PATH: usize = 203;
const CTX_REMOVE: usize = 204;
const CTX_RETRY_THUMBNAIL: usize = 205;
const WALLPAPER_SITES: [(&str, &str); 4] = [
    ("MyLiveWallpapers", "https://mylivewallpapers.com"),
    ("MoeWalls", "https://moewalls.com"),
    ("MotionBGs", "https://motionbgs.com"),
    ("DesktopHut", "https://www.desktophut.com"),
];
const PRODUCT_URL: &str = "https://livelydesk.vercel.app";
const SUPPORT_URL: &str = "https://yapp.ink/raflyf";
const SAWERIA_URL: &str = "https://saweria.co/BuyMatcha";
const CLASS: PCWSTR = w!("WallpaperLive.Panel");
const APP_ID: PCWSTR = w!("Raflyf.WallpaperLive");
static TASKBAR: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn playback_position(playback: &Playback) -> Option<f64> {
    playback
        .player
        .property(c"time-pos")
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|p| p.is_finite() && *p >= 0.0)
}
fn effective_source_fps(info: Option<&crate::cache::Prepared>, playback: &Playback) -> f64 {
    info.map(|p| p.fps)
        .filter(|f| f.is_finite() && *f > 0.0)
        .or_else(|| {
            playback
                .player
                .property(c"container-fps")
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|f| f.is_finite() && *f > 0.0)
        })
        .unwrap_or(0.0)
}
fn dormant_grace(reasons: &PauseReasons) -> Option<Duration> {
    if reasons.manual {
        None
    } else if reasons.locked
        || reasons.remote
        || reasons.disconnected
        || reasons.suspended
        || reasons.display_off
    {
        Some(Duration::from_secs(3))
    } else {
        // Keep maximized, fullscreen, and battery pauses warm on their last frame for instant desktop return.
        None
    }
}
fn is_shell_class(class: &[u16]) -> bool {
    const SHELL: [&str; 4] = [
        "Progman",
        "WorkerW",
        "Shell_TrayWnd",
        "Shell_SecondaryTrayWnd",
    ];
    SHELL.iter().any(|name| {
        class.len() == name.len() && class.iter().copied().eq(name.bytes().map(u16::from))
    })
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn native_error(error: windows::core::Error) -> String {
    error.to_string()
}
fn copy_to_clipboard(hwnd: HWND, text: &str) -> Result<(), String> {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    let wide = wide(text);
    let bytes = wide.len() * std::mem::size_of::<u16>();
    unsafe {
        OpenClipboard(hwnd).map_err(native_error)?;
        let _ = EmptyClipboard();
        let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(native_error)?;
        let ptr = GlobalLock(hmem);
        if ptr.is_null() {
            let _ = GlobalFree(hmem);
            let _ = CloseClipboard();
            return Err("Could not lock clipboard memory".into());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr.cast(), wide.len());
        let _ = GlobalUnlock(hmem);
        if SetClipboardData(13, HANDLE(hmem.0)).is_err() {
            let _ = GlobalFree(hmem);
            let _ = CloseClipboard();
            return Err("Could not set clipboard data".into());
        }
        let _ = CloseClipboard();
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum TrayEvent {
    Open,
    Menu,
    Update,
}
fn tray_event(payload: u32) -> Option<TrayEvent> {
    if payload >> 16 != 1 {
        return None;
    }
    match payload & 0xffff {
        NIN_SELECT | 1025 => Some(TrayEvent::Open),
        WM_CONTEXTMENU => Some(TrayEvent::Menu),
        NIN_BALLOONUSERCLICK => Some(TrayEvent::Update),
        _ => None,
    }
}

struct Icon(HICON);
impl Icon {
    fn load(size: i32) -> Result<Self, String> {
        // SAFETY: bundled PNG is immutable; this handle has one owner until its window is gone.
        unsafe {
            CreateIconFromResourceEx(
                include_bytes!("../assets/wallpaperlive.png"),
                true,
                0x00030000,
                size,
                size,
                LR_DEFAULTCOLOR,
            )
        }
        .map(Self)
        .map_err(native_error)
    }
}
impl Drop for Icon {
    fn drop(&mut self) {
        // SAFETY: icons outlive the panel and tray, and are not shared system resources.
        unsafe {
            let _ = DestroyIcon(self.0);
        }
    }
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
    audio_enabled: bool,
}
struct Dormant {
    path: PathBuf,
    position: f64,
}
struct DisplayState {
    monitor: String,
    pause_reason: Option<&'static str>,
    playback: Option<Playback>,
    actual_pause: bool,
    requested_pause: bool,
    loaded: bool,
    load_request: u64,
    load_entry: Option<i64>,
    file_loaded: Option<i64>,
    video_ready: Option<i64>,
    loading_since: Option<Instant>,
    recovery: Option<(Instant, u8, PathBuf)>,
    preparation: Option<(PathBuf, (u32, u32), u32)>,
    preparation_thread: Option<(
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        std::thread::JoinHandle<()>,
    )>,
    preparation_id: u64,
    preparation_rx: std::sync::mpsc::Receiver<(u64, Result<crate::cache::Prepared, String>)>,
    preparation_tx: std::sync::mpsc::Sender<(u64, Result<crate::cache::Prepared, String>)>,
    prepared_waiting: Option<(Instant, PathBuf, crate::cache::Prepared)>,
    playback_info: Option<crate::cache::Prepared>,
    dormant: Option<Dormant>,
    resume_position: f64,
    paused_since: Option<Instant>,
    pause_trimmed: bool,
}
impl DisplayState {
    fn new(monitor: String) -> Self {
        let (preparation_tx, preparation_rx) = std::sync::mpsc::channel();
        Self {
            monitor,
            pause_reason: None,
            playback: None,
            actual_pause: false,
            requested_pause: false,
            loaded: false,
            load_request: 0,
            load_entry: None,
            file_loaded: None,
            video_ready: None,
            loading_since: None,
            recovery: None,
            preparation: None,
            preparation_thread: None,
            preparation_id: 0,
            preparation_rx,
            preparation_tx,
            prepared_waiting: None,
            playback_info: None,
            dormant: None,
            resume_position: 0.0,
            paused_since: None,
            pause_trimmed: false,
        }
    }
    fn active(&self) -> bool {
        self.playback.is_some()
            || self.preparation.is_some()
            || self.recovery.is_some()
            || self.dormant.is_some()
            || self.preparation_thread.is_some()
            || self.prepared_waiting.is_some()
    }
}
impl Drop for DisplayState {
    fn drop(&mut self) {
        if let Some((cancelled, worker)) = self.preparation_thread.take() {
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = worker.join();
        }
    }
}
struct App {
    hwnd: HWND,
    controls: Vec<(usize, HWND)>,
    font: HFONT,
    icon: Icon,
    small_icon: Icon,
    monitors: Vec<Monitor>,
    settings: Settings,
    selected: Option<PathBuf>,
    config: PathBuf,
    config_valid: bool,
    playback: Option<Playback>,
    displays: Vec<DisplayState>,
    multi_dispatch: bool,
    active_monitor: Option<String>,
    audio_monitor: Option<String>,
    cache_busy: bool,
    reasons: PauseReasons,
    actual_pause: bool,
    requested_pause: bool,
    loaded: bool,
    load_request: u64,
    load_entry: Option<i64>,
    file_loaded: Option<i64>,
    video_ready: Option<i64>,
    loading_since: Option<Instant>,
    recovery: Option<(Instant, u8, PathBuf)>,
    power: Vec<HPOWERNOTIFY>,
    tray_present: bool,
    update_tag: Option<String>,
    update_manual: bool,
    update_tx: std::sync::mpsc::Sender<Result<Option<String>, String>>,
    update_rx: std::sync::mpsc::Receiver<Result<Option<String>, String>>,
    update_thread: Option<(
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        std::thread::JoinHandle<()>,
    )>,
    error: Option<String>,
    library: Vec<PathBuf>,
    folder_filter: Option<PathBuf>,
    search: String,
    sort: crate::library::Sort,
    folder_roots: Vec<PathBuf>,
    marked: Vec<PathBuf>,
    preparation: Option<(PathBuf, (u32, u32), u32)>,
    preparation_thread: Option<(
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        std::thread::JoinHandle<()>,
    )>,
    preparation_id: u64,
    preparation_rx: std::sync::mpsc::Receiver<(u64, Result<crate::cache::Prepared, String>)>,
    preparation_tx: std::sync::mpsc::Sender<(u64, Result<crate::cache::Prepared, String>)>,
    prepared_waiting: Option<(Instant, PathBuf, crate::cache::Prepared)>,
    playback_info: Option<crate::cache::Prepared>,
    dormant: Option<Dormant>,
    resume_position: f64,
    paused_since: Option<Instant>,
    pause_trimmed: bool,
    wts_tick: u8,
    status_cache: std::cell::RefCell<(String, &'static str, &'static str, Option<bool>)>,
    page: usize,
    tab: visuals::Tab,
    thumbnail_pending: Option<PathBuf>,
    thumbnail_thread: Option<(
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        std::thread::JoinHandle<()>,
    )>,
    thumbnail_failed: std::collections::HashSet<PathBuf>,
    thumbnail_rx: std::sync::mpsc::Receiver<(PathBuf, Result<Vec<u8>, String>)>,
    thumbnail_tx: std::sync::mpsc::Sender<(PathBuf, Result<Vec<u8>, String>)>,
    last_tile_click: Option<(usize, Instant)>,
}
impl App {
    fn swap_display(&mut self, state: &mut DisplayState) {
        macro_rules! swap { ($($field:ident),+ $(,)?) => { $(std::mem::swap(&mut self.$field, &mut state.$field);)+ }; }
        swap!(
            playback,
            actual_pause,
            requested_pause,
            loaded,
            load_request,
            load_entry,
            file_loaded,
            video_ready,
            loading_since,
            recovery,
            preparation,
            preparation_thread,
            preparation_id,
            preparation_rx,
            preparation_tx,
            prepared_waiting,
            playback_info,
            dormant,
            resume_position,
            paused_since,
            pause_trimmed
        );
    }
    fn with_display<T>(
        &mut self,
        index: usize,
        operation: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut state = self.displays.remove(index);
        let monitor = self.active_monitor.replace(state.monitor.clone());
        let dispatch = self.multi_dispatch;
        let busy = self.cache_busy;
        let reasons = self.reasons;
        self.multi_dispatch = true;
        self.cache_busy = self
            .displays
            .iter()
            .any(|display| display.preparation_thread.is_some());
        self.swap_display(&mut state);
        let result = operation(self);
        state.pause_reason = self.reasons.reason();
        self.swap_display(&mut state);
        self.displays.insert(index, state);
        self.active_monitor = monitor;
        self.multi_dispatch = dispatch;
        self.cache_busy = busy;
        self.reasons = reasons;
        result
    }
    fn target_monitors(&self) -> Vec<String> {
        self.monitors
            .iter()
            .filter(|monitor| self.settings.monitor == "*" || monitor.name == self.settings.monitor)
            .map(|monitor| monitor.name.clone())
            .collect()
    }
    fn active_paths(&self) -> Vec<PathBuf> {
        self.playback
            .iter()
            .map(|p| p.path.clone())
            .chain(
                self.displays
                    .iter()
                    .filter_map(|d| d.playback.as_ref().map(|p| p.path.clone())),
            )
            .collect()
    }
    fn display_audio(&self) -> bool {
        self.settings.audio_enabled
            && (self.active_monitor.is_none() || self.active_monitor == self.audio_monitor)
    }
    fn sync_audio(&mut self) -> Result<(), String> {
        if self.audio_monitor.as_ref().is_none_or(|name| {
            !self
                .displays
                .iter()
                .any(|d| &d.monitor == name && d.playback.is_some())
        }) {
            self.audio_monitor = self
                .displays
                .iter()
                .find(|d| d.playback.is_some())
                .map(|d| d.monitor.clone());
        }
        for display in &mut self.displays {
            if let Some(playback) = &mut display.playback
                && playback.audio_enabled
                && (!self.settings.audio_enabled
                    || self.audio_monitor.as_ref() != Some(&display.monitor))
            {
                playback.player.set_audio(false)?;
                playback.audio_enabled = false;
            }
        }
        for display in &mut self.displays {
            if let Some(playback) = &mut display.playback {
                let enabled = self.settings.audio_enabled
                    && self.audio_monitor.as_ref() == Some(&display.monitor);
                if playback.audio_enabled != enabled {
                    playback.player.set_audio(enabled)?;
                    playback.audio_enabled = enabled;
                }
            }
        }
        Ok(())
    }
    fn start_targets(&mut self, source: &Path) -> Result<(), String> {
        let targets = self.target_monitors();
        if targets.is_empty() {
            return Err("No selected display is connected".into());
        }
        if self
            .settings
            .wallpapers
            .iter()
            .map(|(monitor, _)| monitor)
            .chain(targets.iter())
            .collect::<std::collections::HashSet<_>>()
            .len()
            > 64
        {
            return Err("At most 64 display wallpaper assignments can be saved".into());
        }
        if self.audio_monitor.is_none() {
            self.audio_monitor = targets.first().cloned();
        }
        let mut error = None;
        for monitor in targets {
            let index = self
                .displays
                .iter()
                .position(|d| d.monitor == monitor)
                .unwrap_or_else(|| {
                    self.displays.push(DisplayState::new(monitor));
                    self.displays.len() - 1
                });
            if let Err(failure) = self.with_display(index, |app| {
                app.resume_position = 0.0;
                app.start_path(source)
            }) {
                error = Some(failure);
            }
        }
        self.sync_audio()?;
        self.arm_monitor()?;
        self.refresh_library()?;
        if let Some(error) = error {
            return Err(error);
        }
        Ok(())
    }
    fn restore_displays(&mut self) -> Result<(), String> {
        let assignments = self.settings.wallpapers.clone();
        for (monitor, path) in assignments {
            if self
                .displays
                .iter()
                .any(|display| display.monitor == monitor)
            {
                continue;
            }
            self.displays.push(DisplayState::new(monitor.clone()));
            let index = self.displays.len() - 1;
            if self.monitors.iter().any(|display| display.name == monitor) {
                if let Err(error) = self.with_display(index, |app| app.start_path(&path)) {
                    self.error = Some(error);
                }
            } else {
                self.displays[index].recovery = Some((Instant::now(), 0, path));
            }
        }
        self.sync_audio()?;
        if !self.displays.is_empty() {
            self.arm_monitor()?;
        }
        Ok(())
    }
    fn tick_displays(&mut self) -> Result<(), String> {
        self.update_reasons();
        self.reasons.fullscreen = false;
        self.reasons.maximized = false;
        let mut error = None;
        for index in 0..self.displays.len() {
            if !self
                .monitors
                .iter()
                .any(|m| m.name == self.displays[index].monitor)
            {
                let display = &mut self.displays[index];
                if display
                    .preparation_thread
                    .as_ref()
                    .is_some_and(|(_, worker)| worker.is_finished())
                {
                    if let Some((_, worker)) = display.preparation_thread.take() {
                        let _ = worker.join();
                    }
                    while display.preparation_rx.try_recv().is_ok() {}
                }
                continue;
            }
            if let Err(failure) = self.with_display(index, |app| app.tick()) {
                error = Some(failure);
            }
        }
        self.displays.retain(DisplayState::active);
        self.sync_audio()?;
        visuals::playing_all(&self.active_paths());
        let connected = self
            .displays
            .iter()
            .any(|d| d.active() && self.monitors.iter().any(|m| m.name == d.monitor));
        if connected
            || self
                .displays
                .iter()
                .any(|display| display.preparation_thread.is_some())
            || self.thumbnail_thread.is_some()
        {
            self.arm_monitor()?;
        } else {
            unsafe {
                let _ = KillTimer(self.hwnd, 1);
            }
        }
        self.request_thumbnails();
        self.update_status();
        if let Some(error) = error {
            return Err(error);
        }
        Ok(())
    }
    fn displays_changed(&mut self) -> Result<(), String> {
        self.refresh_monitors()?;
        for index in 0..self.displays.len() {
            self.with_display(index, |app| {
                let path = app
                    .playback
                    .as_ref()
                    .map(|p| p.path.clone())
                    .or_else(|| app.preparation.as_ref().map(|(p, _, _)| p.clone()))
                    .or_else(|| app.recovery.as_ref().map(|(_, _, p)| p.clone()));
                if let Some(path) = path {
                    app.resume_position = app
                        .playback
                        .as_ref()
                        .and_then(playback_position)
                        .unwrap_or(app.resume_position);
                    app.cancel_preparation();
                    app.release_playback();
                    app.recovery = Some((Instant::now() + Duration::from_millis(500), 0, path));
                }
                Ok(())
            })?;
        }
        self.restore_displays()?;
        self.arm_monitor()
    }
    fn control(&self, id: usize) -> HWND {
        self.controls
            .iter()
            .find(|c| c.0 == id)
            .map(|c| c.1)
            .unwrap_or_default()
    }
    fn text(&self, id: usize, text: &str) {
        if id == STATUS {
            self.status_cache.borrow_mut().0 = text.to_owned();
        } else if id == PAUSE {
            self.status_cache.borrow_mut().1 = "";
        } else if id == visuals::AUDIO {
            self.status_cache.borrow_mut().2 = "";
        }
        let text = wide(text);
        // SAFETY: synchronous native text copy into owned control.
        unsafe {
            let _ = SetWindowTextW(self.control(id), PCWSTR(text.as_ptr()));
        }
    }
    fn set_error(&mut self, error: String) {
        self.error = Some(error);
        if !self.multi_dispatch
            && !self.displays.iter().any(DisplayState::active)
            && self.playback.is_none()
            && self.preparation.is_none()
            && self.dormant.is_none()
            && self.recovery.is_none()
            && self.preparation_thread.is_none()
            && self.prepared_waiting.is_none()
            && self.thumbnail_thread.is_none()
        {
            // No wallpaper or worker remains to monitor after a failed preparation.
            unsafe {
                let _ = KillTimer(self.hwnd, 1);
            }
        }
        self.update_status();
    }
    fn screen(&self) -> &Monitor {
        self.monitors
            .iter()
            .find(|m| {
                self.active_monitor
                    .as_deref()
                    .unwrap_or(&self.settings.monitor)
                    == m.name
            })
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
                    visuals::SEARCH,
                    w!("EDIT"),
                    "",
                    ES_AUTOHSCROLL as u32 | WS_TABSTOP.0 | WS_BORDER.0,
                ),
                (
                    visuals::SORT,
                    w!("BUTTON"),
                    "Newest added",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    visuals::FIND_WALLPAPERS,
                    w!("BUTTON"),
                    "Where to find wallpapers",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    visuals::PRODUCT_WEBSITE,
                    w!("BUTTON"),
                    "Product website",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    visuals::SUPPORT,
                    w!("BUTTON"),
                    "Buy me a coffee",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    visuals::SAWERIA,
                    w!("BUTTON"),
                    "Saweria",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    visuals::SOURCES_BACK,
                    w!("BUTTON"),
                    "Back to Library",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    visuals::SOURCES_NOTE,
                    w!("STATIC"),
                    "WallpaperLive does not supply or download wallpapers.\r\nDownload media, not installers. Respect creator licenses and use files you trust. These third-party sites are not safety-verified.",
                    SS_LEFT.0 | SS_NOPREFIX.0,
                ),
                (
                    visuals::SETTINGS_NOTE,
                    w!("STATIC"),
                    "Session lock and display sleep always pause playback.",
                    SS_LEFT.0 | SS_NOPREFIX.0,
                ),
                (
                    visuals::SETTINGS_INFO,
                    w!("STATIC"),
                    "Memory readings may differ from Task Manager. Clear unused cache keeps imported media and active wallpapers.",
                    SS_LEFT.0 | SS_NOPREFIX.0,
                ),
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
                    w!("BUTTON"),
                    "Select display",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    FIT,
                    w!("BUTTON"),
                    "Fill (crop to screen)",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
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
                    visuals::TAB_LIBRARY,
                    w!("BUTTON"),
                    "Library",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    visuals::TAB_SETTINGS,
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
                    visuals::AUDIO,
                    w!("BUTTON"),
                    "Sound: Off",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    visuals::REMOVE,
                    w!("BUTTON"),
                    "Remove",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (visuals::PAGE, w!("STATIC"), "", 13),
                (
                    visuals::ADD_FOLDER,
                    w!("BUTTON"),
                    "Add folder",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    visuals::FOLDER_FILTER,
                    w!("BUTTON"),
                    "All folders",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    visuals::FPS,
                    w!("BUTTON"),
                    "Frame rate: 30 FPS",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    visuals::SELECT_ALL,
                    w!("BUTTON"),
                    "Select all",
                    BS_OWNERDRAW as u32 | WS_TABSTOP.0,
                ),
                (
                    121,
                    w!("BUTTON"),
                    "Pause for fullscreen or maximized apps",
                    BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0,
                ),
                (
                    visuals::VERSION,
                    w!("STATIC"),
                    concat!("Version ", env!("CARGO_PKG_VERSION")),
                    SS_ENDELLIPSIS.0,
                ),
                (
                    visuals::MEMORY_INFO,
                    w!("STATIC"),
                    "Memory: measuring...",
                    SS_LEFT.0 | SS_NOPREFIX.0,
                ),
                (
                    visuals::CACHE_INFO,
                    w!("STATIC"),
                    "Video cache: measuring...",
                    SS_ENDELLIPSIS.0,
                ),
                (
                    visuals::CLEAR_CACHE,
                    w!("BUTTON"),
                    "Clear unused cache",
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
                self.controls.push((id, child));
                if matches!(id, BATTERY | STARTUP | 121) {
                    let _ = windows::Win32::UI::Controls::SetWindowTheme(child, w!(""), w!(""));
                }
            }
            for (index, (name, url)) in WALLPAPER_SITES.iter().enumerate() {
                let label = wide(&format!(
                    "{name}  -  {}",
                    url.trim_start_matches("https://")
                ));
                let child = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("BUTTON"),
                    PCWSTR(label.as_ptr()),
                    WS_CHILD | WINDOW_STYLE(BS_OWNERDRAW as u32) | WS_TABSTOP,
                    0,
                    0,
                    0,
                    0,
                    self.hwnd,
                    HMENU((visuals::SITE_BASE + index) as *mut _),
                    instance,
                    None,
                )
                .map_err(native_error)?;
                self.controls.push((visuals::SITE_BASE + index, child));
            }
            let cue = wide("Search by filename");
            SendMessageW(
                self.control(visuals::SEARCH),
                windows::Win32::UI::Controls::EM_SETCUEBANNER,
                WPARAM(1),
                LPARAM(cue.as_ptr() as isize),
            );
            SendMessageW(
                self.control(visuals::SEARCH),
                windows::Win32::UI::Controls::EM_SETLIMITTEXT,
                WPARAM(256),
                LPARAM(0),
            );
            for i in 0..9 {
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
            for (_, control) in &self.controls {
                visuals::prepare_control(*control)?;
            }
            SendMessageW(
                self.hwnd,
                WM_CHANGEUISTATE,
                WPARAM(UIS_SET as usize | ((UISF_HIDEFOCUS as usize) << 16)),
                LPARAM(0),
            );
            self.text(
                FIT,
                if self.settings.fit == Fit::Contain {
                    "Fit (show full frame)"
                } else {
                    "Fill (crop to screen)"
                },
            );
            SendMessageW(
                self.control(121),
                BM_SETCHECK,
                WPARAM(self.settings.pause_fullscreen as usize),
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
        if self.settings.wallpapers.is_empty()
            && let Some(path) = self.settings.path.clone()
        {
            let monitor = if self.settings.monitor.is_empty() {
                monitors[0].name.clone()
            } else {
                self.settings.monitor.clone()
            };
            if monitor != "*" {
                self.settings.wallpapers.push((monitor, path));
            }
        }
        if self.settings.monitor != "*" && !monitors.iter().any(|m| m.name == self.settings.monitor)
        {
            self.settings.monitor = monitors[0].name.clone();
        }
        self.monitors = monitors;
        if self.settings.monitor == "*" {
            self.text(SCREEN, &format!("All displays ({})", self.monitors.len()));
            return Ok(());
        }
        self.text(
            SCREEN,
            &format!(
                "Display {}  {} x {}{}",
                self.monitors
                    .iter()
                    .position(|m| m.name == self.settings.monitor)
                    .unwrap_or(0)
                    + 1,
                self.screen().rect.right - self.screen().rect.left,
                self.screen().rect.bottom - self.screen().rect.top,
                if self.screen().primary {
                    " (primary)"
                } else {
                    ""
                }
            ),
        );
        Ok(())
    }
    fn cache_directory(&self) -> Option<PathBuf> {
        Some(self.config.parent()?.join("video-cache"))
    }
    fn active_cache(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self
            .playback_info
            .iter()
            .filter(|p| p.cached)
            .map(|p| p.path.clone())
            .collect();
        paths.extend(self.prepared_waiting.iter().map(|(_, _, p)| p.path.clone()));
        for display in &self.displays {
            paths.extend(
                display
                    .playback_info
                    .iter()
                    .filter(|p| p.cached)
                    .map(|p| p.path.clone()),
            );
            paths.extend(
                display
                    .prepared_waiting
                    .iter()
                    .map(|(_, _, p)| p.path.clone()),
            );
        }
        paths.sort();
        paths.dedup();
        paths
    }
    // Read-only diagnostics: one process query and one directory listing per Settings visit.
    fn refresh_diagnostics(&self) {
        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        // SAFETY: current-process pseudo handle and correctly sized output struct.
        let memory =
            unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) }
                .map(|_| {
                    format!(
                        "Memory: {:.0} MB in use\r\nPeak: {:.0} MB",
                        counters.WorkingSetSize as f64 / 1048576.0,
                        counters.PeakWorkingSetSize as f64 / 1048576.0
                    )
                })
                .unwrap_or_else(|_| "Memory: unavailable".into());
        self.text(visuals::MEMORY_INFO, &memory);
        let cache = self
            .cache_directory()
            .map(|d| crate::cache::cache_size(&d))
            .unwrap_or(0);
        self.text(
            visuals::CACHE_INFO,
            &format!("Video cache: {:.0} MB of 2048 MB", cache as f64 / 1048576.0),
        );
    }
    fn clear_cache(&mut self) -> Result<(), String> {
        if self.preparation.is_some()
            || self.preparation_thread.is_some()
            || self
                .displays
                .iter()
                .any(|d| d.preparation.is_some() || d.preparation_thread.is_some())
        {
            return Err("Wait for the background cache to finish before clearing cache".into());
        }
        let directory = self.cache_directory().ok_or("Invalid cache directory")?;
        let mut active = self.active_cache();
        active.extend(self.library.iter().cloned());
        active.extend(
            self.settings
                .wallpapers
                .iter()
                .map(|(_, path)| path.clone()),
        );
        active.extend(self.settings.path.iter().cloned());
        active.extend(self.active_paths());
        let prompt = wide(
            "Delete cached display copies that are not in use?\n\nOriginal media files are never touched. Caches are rebuilt automatically when needed.",
        );
        // SAFETY: modal confirmation owned by this window.
        if unsafe {
            MessageBoxW(
                self.hwnd,
                PCWSTR(prompt.as_ptr()),
                w!("Clear unused cache"),
                MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
            )
        } != IDYES
        {
            return Ok(());
        }
        let (count, freed) = crate::cache::clear_unused_cache(&directory, &active)?;
        self.refresh_diagnostics();
        self.error = None;
        self.text(
            STATUS,
            &format!(
                "Removed {count} cache file(s), freed {:.0} MB.",
                freed as f64 / 1048576.0
            ),
        );
        Ok(())
    }
    fn context_menu(&mut self, child: HWND, coord: LPARAM) -> Result<(), String> {
        let id = unsafe { GetDlgCtrlID(child) as usize };
        if !(visuals::TILE_BASE..visuals::TILE_BASE + 9).contains(&id) {
            return Ok(());
        }
        let slot = id - visuals::TILE_BASE;
        let index = self.page * 9 + slot;
        let Some(path) = self.library_view().get(index).cloned() else {
            return Ok(());
        };
        self.selected = Some(path.clone());
        self.marked = vec![path.clone()];
        self.refresh_library()?;

        let mut pt = POINT {
            x: (coord.0 as u16 as i16) as i32,
            y: ((coord.0 >> 16) as u16 as i16) as i32,
        };
        if pt.x == -1 && pt.y == -1 {
            let mut r = RECT::default();
            unsafe {
                let _ = GetWindowRect(child, &mut r);
            }
            pt.x = (r.left + r.right) / 2;
            pt.y = (r.top + r.bottom) / 2;
        }

        let chosen = crate::tray::popup_at(
            self.hwnd,
            pt,
            &[
                (CTX_SET, "Set as wallpaper"),
                (CTX_EXPLORER, "Open file location"),
                (CTX_COPY_PATH, "Copy file path"),
                (CTX_RETRY_THUMBNAIL, "Retry thumbnail"),
                (CTX_REMOVE, "Remove from library"),
            ],
            None,
        )?;
        match chosen {
            CTX_SET => {
                self.recovery = None;
                self.start()?;
            }
            CTX_EXPLORER => {
                let _ = std::process::Command::new("explorer.exe")
                    .arg(format!("/select,{}", path.display()))
                    .spawn();
            }
            CTX_COPY_PATH => {
                if let Err(e) = copy_to_clipboard(self.hwnd, &path.to_string_lossy()) {
                    self.set_error(e);
                } else {
                    self.text(STATUS, "File path copied to clipboard.");
                }
            }
            CTX_RETRY_THUMBNAIL => self.retry_thumbnail(&path)?,
            CTX_REMOVE => {
                self.remove_selected()?;
            }
            _ => {}
        }
        Ok(())
    }
    fn library_view(&self) -> Vec<PathBuf> {
        crate::library::view(
            &self.library,
            self.folder_filter.as_deref(),
            &self.search,
            self.sort,
        )
    }
    fn folder_options(&self) -> Vec<PathBuf> {
        let mut folders = crate::library::folders(&self.library);
        for root in &self.folder_roots {
            if !folders.contains(root)
                && !crate::library::filtered(&self.library, Some(root)).is_empty()
            {
                folders.push(root.clone());
            }
        }
        folders
    }
    fn layout(&mut self) -> Result<(), String> {
        self.apply_layout(true)
    }
    fn apply_layout(&mut self, repaint: bool) -> Result<(), String> {
        visuals::cancel_transition(self.hwnd);
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
                SendMessageW(
                    *control,
                    WM_SETFONT,
                    WPARAM(font.0 as usize),
                    LPARAM(isize::from(repaint)),
                );
            }
            if !self.font.0.is_null() {
                let _ = DeleteObject(self.font);
            }
            self.font = font;
            if IsIconic(self.hwnd).as_bool() {
                return Ok(());
            }
            let mut r = RECT::default();
            GetClientRect(self.hwnd, &mut r).map_err(native_error)?;
            if r.right <= r.left || r.bottom <= r.top {
                return Ok(());
            }
            let width = r.right * 96 / dpi;
            let height = r.bottom * 96 / dpi;
            let layout = visuals::Layout::new(width, height);
            let noredraw = if repaint {
                SET_WINDOW_POS_FLAGS(0)
            } else {
                SWP_NOREDRAW
            };
            for (id, control) in &self.controls {
                if let Some(area) = layout.control(*id) {
                    SetWindowPos(
                        *control,
                        None,
                        scale(area.left),
                        scale(area.top),
                        scale(area.right - area.left),
                        scale(area.bottom - area.top),
                        SWP_NOZORDER | SWP_NOACTIVATE | noredraw,
                    )
                    .map_err(native_error)?;
                    visuals::round_control(*control)?;
                }
            }
            let view = self.library_view();
            for (id, control) in &self.controls {
                let populated = if (visuals::TILE_BASE..visuals::TILE_BASE + 9).contains(id) {
                    view.get(self.page * 9 + id - visuals::TILE_BASE).is_some()
                } else if matches!(*id, visuals::PREVIOUS | visuals::NEXT | visuals::PAGE) {
                    !view.is_empty()
                } else {
                    true
                };
                let visible = visuals::visible_on(self.tab, *id) && populated;
                let was_visible = GetWindowLongW(*control, GWL_STYLE) as u32 & WS_VISIBLE.0 != 0;
                if was_visible != visible {
                    let _ = SetWindowPos(
                        *control,
                        None,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE
                            | SWP_NOSIZE
                            | SWP_NOZORDER
                            | SWP_NOACTIVATE
                            | noredraw
                            | if visible {
                                SWP_SHOWWINDOW
                            } else {
                                SWP_HIDEWINDOW
                            },
                    );
                }
            }
            let _ = EnableWindow(self.control(visuals::SELECT_ALL), !view.is_empty());
            let _ = EnableWindow(self.control(visuals::PREVIOUS), self.page > 0);
            let _ = EnableWindow(
                self.control(visuals::NEXT),
                (self.page + 1) * 9 < view.len(),
            );
            if repaint {
                for (_, control) in &self.controls {
                    let _ = InvalidateRect(*control, None, true);
                }
                let _ = InvalidateRect(self.hwnd, None, true);
                let _ = UpdateWindow(self.hwnd);
            }
        }
        Ok(())
    }
    fn body_clip(&self, only_library: bool) -> Option<RECT> {
        // SAFETY: read-only geometry query against the live App HWND.
        unsafe {
            if !IsWindowVisible(self.hwnd).as_bool()
                || IsIconic(self.hwnd).as_bool()
                || !visuals::animations_enabled(self.hwnd)
            {
                return None;
            }
            let mut r = RECT::default();
            if GetClientRect(self.hwnd, &mut r).is_err() || r.right <= r.left || r.bottom <= r.top {
                return None;
            }
            let dpi = GetDpiForWindow(self.hwnd).max(96) as i32;
            let scale = |n: i32| n * dpi / 96;
            let layout = visuals::Layout::new(r.right * 96 / dpi, r.bottom * 96 / dpi);
            let rect = if only_library {
                RECT {
                    left: scale(layout.library.left).max(0),
                    top: scale(layout.library.top).max(0),
                    right: scale(layout.library.right).min(r.right),
                    bottom: scale(layout.library.bottom).min(r.bottom),
                }
            } else {
                RECT {
                    left: 0,
                    top: scale(layout.header.bottom).max(0),
                    right: r.right,
                    bottom: scale(layout.footer.top).min(r.bottom),
                }
            };
            (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
        }
    }
    fn visible_controls_for(&self, filter_body: bool) -> Vec<HWND> {
        self.controls
            .iter()
            .filter_map(|(id, control)| {
                let matches_scope = if filter_body {
                    visuals::is_body_control(*id)
                } else {
                    (visuals::TILE_BASE..visuals::TILE_BASE + 9).contains(id)
                };
                if !matches_scope {
                    return None;
                }
                // SAFETY: child HWND belongs to this App and lives until window teardown.
                let visible =
                    unsafe { GetWindowLongW(*control, GWL_STYLE) as u32 & WS_VISIBLE.0 != 0 };
                visible.then_some(*control)
            })
            .collect()
    }
    fn refresh_library(&mut self) -> Result<(), String> {
        self.refresh_library_with_repaint(true)
    }
    fn refresh_library_with_repaint(&mut self, repaint: bool) -> Result<(), String> {
        let view = self.library_view();
        let pages = view.len().div_ceil(9).max(1);
        self.page = self.page.min(pages - 1);
        let start = self.page * 9;
        let paths: Vec<_> = view.iter().skip(start).take(9).cloned().collect();
        self.marked.retain(|p| self.library.contains(p));
        visuals::marked(&self.marked);
        self.text(visuals::REMOVE, &format!("Remove ({})", self.marked.len()));
        self.text(
            visuals::FPS,
            &format!("Frame rate: {} FPS", self.settings.fps),
        );
        self.text(
            visuals::FOLDER_FILTER,
            &self
                .folder_filter
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "All folders".into()),
        );
        visuals::tiles(&paths);
        visuals::retain_thumbnails(&self.library);
        visuals::library_empty(self.library.is_empty());
        self.text(visuals::SORT, self.sort.label());
        visuals::selected(self.selected.clone());
        visuals::playing_all(&self.active_paths());
        self.text(visuals::PAGE, &format!("Page {} of {pages}", self.page + 1));
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
            let _ = EnableWindow(self.control(visuals::REMOVE), !self.marked.is_empty());
            if repaint {
                let _ = InvalidateRect(self.hwnd, None, false);
                for (_, h) in &self.controls {
                    let _ = InvalidateRect(*h, None, false);
                }
            }
        }
        self.request_thumbnails();
        self.apply_layout(repaint)
    }
    fn step_page(&mut self, delta: isize) -> Result<(), String> {
        let pages = self.library_view().len().div_ceil(9).max(1);
        let next_page = (self.page as isize + delta).clamp(0, (pages - 1) as isize) as usize;
        if next_page == self.page {
            return Ok(());
        }
        visuals::cancel_transition(self.hwnd);
        let clip = self.body_clip(true);
        let old_layer = clip.and_then(|c| visuals::capture_region(self.hwnd, c, &self.controls));
        self.page = next_page;
        if old_layer.is_none() {
            return self.refresh_library();
        }
        self.refresh_library_with_repaint(false)?;
        let hidden = self.visible_controls_for(false);
        if let (Some(c), Some(old)) = (clip, old_layer)
            && let Some(new) = visuals::capture_region(self.hwnd, c, &self.controls)
        {
            let dir = if delta > 0 { -1 } else { 1 };
            visuals::begin_transition(
                self.hwnd,
                c,
                dir,
                visuals::PAGE_ANIM_MS,
                old,
                new,
                hidden,
                None,
                false,
            );
            // SAFETY: invalidate pager buttons while grid slides over body region.
            unsafe {
                for id in [visuals::PREVIOUS, visuals::NEXT, visuals::PAGE] {
                    let _ = InvalidateRect(self.control(id), None, false);
                }
            }
            return Ok(());
        }
        // SAFETY: fallback repaint when capture could not be acquired.
        unsafe {
            let _ = InvalidateRect(self.hwnd, None, true);
            let _ = UpdateWindow(self.hwnd);
        }
        Ok(())
    }
    fn add_media(&mut self, path: PathBuf) -> Result<(), String> {
        let path = validate_media(&path)?;
        let mut library = self.library.clone();
        library.retain(|p| p != &path);
        library.insert(0, path.clone());
        if library.len() > crate::library::MAX_FILES {
            return Err("Library is full (4096 files); remove entries before adding more".into());
        }
        self.save_library(&library)?;
        self.library = library;
        self.folder_filter = None;
        self.marked = vec![path.clone()];
        self.page = 0;
        self.selected = Some(path.clone());
        self.error = None;
        self.refresh_library()?;
        self.thumbnail_failed.remove(&path);
        self.request_thumbnails();
        self.update_status();
        Ok(())
    }
    fn save_library(&self, paths: &[PathBuf]) -> Result<(), String> {
        if paths.len() > crate::library::MAX_FILES {
            return Err("Library exceeds 4096 files; existing entries were kept".into());
        }
        if !self.config_valid {
            return Err(
                "Existing preferences could not be read; library was not overwritten.".into(),
            );
        }
        let parent = self.config.parent().ok_or("Invalid library directory")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut recent = String::from("version=2\n");
        for root in &self.folder_roots {
            crate::core::validate_path_syntax(root)?;
            recent.push_str("folder=");
            recent.push_str(root.to_str().ok_or("Folder name is not valid Unicode")?);
            recent.push('\n');
        }
        for path in paths {
            let text = path.to_str().ok_or("File name is not valid Unicode")?;
            crate::core::validate_path_syntax(path)?;
            recent.push_str("file=");
            recent.push_str(text);
            recent.push('\n');
        }
        let temporary = parent.join("recent.tmp");
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        file.write_all(recent.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("Could not save library: {e}"))?;
        drop(file);
        let from = wide(temporary.to_str().ok_or("Invalid library path")?);
        let to = wide(
            parent
                .join("recent.txt")
                .to_str()
                .ok_or("Invalid library path")?,
        );
        unsafe {
            MoveFileExW(
                PCWSTR(from.as_ptr()),
                PCWSTR(to.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(|e| format!("Previous library was kept: {e}"))
    }
    fn remove_selected(&mut self) -> Result<(), String> {
        let selected: Vec<_> = self
            .marked
            .iter()
            .filter(|p| self.library.contains(p))
            .cloned()
            .collect();
        if selected.is_empty() {
            return Err("Select wallpapers to remove".into());
        }
        let paths: Vec<_> = self
            .library
            .iter()
            .filter(|p| !selected.contains(p))
            .cloned()
            .collect();
        self.save_library(&paths)?;
        let must_stop = self
            .settings
            .path
            .as_ref()
            .is_some_and(|p| selected.contains(p))
            || self
                .playback
                .as_ref()
                .is_some_and(|p| selected.contains(&p.path))
            || self
                .recovery
                .as_ref()
                .is_some_and(|(_, _, p)| selected.contains(p))
            || self
                .preparation
                .as_ref()
                .is_some_and(|(p, _, _)| selected.contains(p))
            || self
                .dormant
                .as_ref()
                .is_some_and(|p| selected.contains(&p.path));
        self.library = paths;
        self.marked.clear();
        self.page = self
            .page
            .min(self.library_view().len().div_ceil(9).max(1) - 1);
        if self.selected.as_ref().is_some_and(|p| selected.contains(p)) {
            self.selected = self.library_view().first().cloned();
        }
        for path in &selected {
            self.thumbnail_failed.remove(path);
            visuals::remove_thumbnail(path);
        }
        if self.displays.is_empty() {
            if must_stop {
                self.stop()?;
            }
        } else {
            let previous = self.settings.clone();
            self.settings
                .wallpapers
                .retain(|(_, path)| !selected.contains(path));
            if self
                .settings
                .path
                .as_ref()
                .is_some_and(|path| selected.contains(path))
            {
                self.settings.path = self
                    .settings
                    .wallpapers
                    .first()
                    .map(|(_, path)| path.clone());
            }
            if let Err(error) = self.save() {
                self.settings = previous;
                return Err(error);
            }
            for index in 0..self.displays.len() {
                let display = &self.displays[index];
                let affected = display
                    .playback
                    .as_ref()
                    .is_some_and(|p| selected.contains(&p.path))
                    || display
                        .preparation
                        .as_ref()
                        .is_some_and(|(p, _, _)| selected.contains(p))
                    || display
                        .recovery
                        .as_ref()
                        .is_some_and(|(_, _, p)| selected.contains(p))
                    || display
                        .prepared_waiting
                        .as_ref()
                        .is_some_and(|(_, p, _)| selected.contains(p))
                    || display
                        .dormant
                        .as_ref()
                        .is_some_and(|p| selected.contains(&p.path));
                if affected {
                    self.with_display(index, |app| {
                        app.cancel_preparation();
                        app.recovery = None;
                        app.dormant = None;
                        app.release_playback();
                        Ok(())
                    })?;
                }
            }
            self.sync_audio()?;
            visuals::playing_all(&self.active_paths());
            if self.displays.iter().any(DisplayState::active) {
                self.arm_monitor()?;
            }
        }
        Ok(())
    }
    fn thumbnail_paths(&self) -> Vec<PathBuf> {
        if self.tab != visuals::Tab::Library
            || self.reasons.reason().is_some()
            || self.preparation.is_some()
            || self
                .displays
                .iter()
                .any(|display| display.preparation_thread.is_some())
            || (!self.displays.is_empty()
                && self
                    .displays
                    .iter()
                    .all(|display| display.pause_reason.is_some()))
            || self.dormant.is_some()
            || !unsafe { IsWindowVisible(self.hwnd) }.as_bool()
        {
            return Vec::new();
        }
        let mut paths: Vec<_> = self
            .library_view()
            .into_iter()
            .skip(self.page * 9)
            .take(9)
            .collect();
        if let Some(path) = &self.selected
            && !paths.contains(path)
        {
            paths.push(path.clone());
        }
        paths
    }
    fn cancel_thumbnails(&mut self) {
        if let Some((cancelled, _)) = &self.thumbnail_thread {
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    fn request_thumbnails(&mut self) {
        if self.multi_dispatch || unsafe { IsIconic(self.hwnd) }.as_bool() {
            return;
        }
        let paths = self.thumbnail_paths();
        if self
            .thumbnail_thread
            .as_ref()
            .is_some_and(|(_, worker)| worker.is_finished())
        {
            let Some((cancelled, worker)) = self.thumbnail_thread.take() else {
                return;
            };
            let _ = worker.join();
            self.thumbnail_pending = None;
            for (path, result) in self.thumbnail_rx.try_iter() {
                if !cancelled.load(std::sync::atomic::Ordering::Relaxed)
                    && self.library.contains(&path)
                {
                    match result.and_then(|bytes| crate::thumbnail::bitmap(&bytes)) {
                        Ok(bitmap) => visuals::thumbnail(path, bitmap),
                        Err(_) => {
                            self.thumbnail_failed.insert(path);
                        }
                    }
                }
            }
            if !visuals::has_transition() {
                // SAFETY: only thumbnail tiles and preview pane depend on newly decoded frames.
                unsafe {
                    for id in (visuals::TILE_BASE..visuals::TILE_BASE + 9)
                        .chain(std::iter::once(visuals::PREVIEW))
                    {
                        let _ = InvalidateRect(self.control(id), None, false);
                    }
                }
            }
        }
        if self.thumbnail_thread.is_some() {
            if paths.is_empty()
                || self
                    .thumbnail_pending
                    .as_ref()
                    .is_some_and(|pending| !paths.contains(pending))
            {
                self.cancel_thumbnails();
            }
            return;
        }
        let Some(path) = paths
            .into_iter()
            .find(|path| !visuals::has_thumbnail(path) && !self.thumbnail_failed.contains(path))
        else {
            return;
        };
        self.thumbnail_pending = Some(path.clone());
        let tx = self.thumbnail_tx.clone();
        let hwnd = self.hwnd.0 as usize;
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        self.thumbnail_thread = Some((
            cancelled,
            std::thread::spawn(move || {
                let result = crate::thumbnail::capture(&path, &worker_cancelled);
                if tx.send((path, result)).is_ok() {
                    unsafe {
                        let _ = PostMessageW(HWND(hwnd as *mut _), THUMBNAIL, WPARAM(0), LPARAM(0));
                    }
                }
            }),
        ));
        if let Err(error) = self.arm_monitor() {
            self.set_error(error);
        }
    }
    fn tray(&mut self, add: bool) -> Result<(), String> {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            // NOTIFYICON_VERSION_4 suppresses standard tooltips unless NIF_SHOWTIP is set.
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: TRAY_EVENT,
            ..Default::default()
        };
        data.hIcon = self.small_icon.0;
        let tip = wide(concat!("WallpaperLive v", env!("CARGO_PKG_VERSION")));
        data.szTip[..tip.len()].copy_from_slice(&tip);
        if !unsafe { Shell_NotifyIconW(if add { NIM_ADD } else { NIM_DELETE }, &data) }.as_bool() {
            return Err(
                "Windows tray is unavailable. Keep this window open to control playback.".into(),
            );
        }
        self.tray_present = add;
        if add {
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            if !unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) }.as_bool() {
                let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
                self.tray_present = false;
                return Err(
                    "Windows could not enable tray controls. Keep this window open.".into(),
                );
            }
        }
        Ok(())
    }
    fn check_updates(&mut self, manual: bool) -> Result<(), String> {
        self.update_manual |= manual;
        if self.update_thread.is_some() {
            return Ok(());
        }
        if unsafe { SetTimer(self.hwnd, 2, 100, None) } == 0 {
            return Err("Could not start the update check".into());
        }
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cancelled.clone();
        let tx = self.update_tx.clone();
        let hwnd = self.hwnd.0 as usize;
        self.update_thread = Some((
            cancelled,
            std::thread::spawn(move || {
                let result = crate::update::check(&flag);
                if !flag.load(std::sync::atomic::Ordering::Relaxed) && tx.send(result).is_ok() {
                    // SAFETY: completion targets only the owned panel, retained until the worker is joined.
                    unsafe {
                        let _ =
                            PostMessageW(HWND(hwnd as *mut _), UPDATE_READY, WPARAM(0), LPARAM(0));
                    }
                }
            }),
        ));
        Ok(())
    }
    fn update_ready(&mut self) -> Result<(), String> {
        if self
            .update_thread
            .as_ref()
            .is_some_and(|(_, worker)| !worker.is_finished())
        {
            return Ok(());
        }
        let finished = self
            .update_thread
            .take()
            .map(|(_, worker)| worker.join().is_err());
        let result = if finished == Some(true) {
            Err("Update checker stopped unexpectedly".into())
        } else if let Ok(result) = self.update_rx.try_recv() {
            result
        } else if finished.is_some() {
            Err("Update checker returned no result".into())
        } else {
            return Ok(());
        };
        unsafe {
            let _ = KillTimer(self.hwnd, 2);
        }
        if let Ok(tag) = &result {
            self.update_tag = tag.clone();
        }
        if std::mem::take(&mut self.update_manual) {
            let newer = matches!(result, Ok(Some(_)));
            let text = wide(&crate::update::message(&result));
            let flags = if newer {
                MB_YESNO | MB_ICONINFORMATION
            } else if result.is_err() {
                MB_OK | MB_ICONWARNING
            } else {
                MB_OK | MB_ICONINFORMATION
            };
            let choice = unsafe {
                MessageBoxW(
                    self.hwnd,
                    PCWSTR(text.as_ptr()),
                    w!("WallpaperLive"),
                    flags | MB_SETFOREGROUND,
                )
            };
            if choice.0 == 0 {
                return Err("Windows could not show the update result".into());
            }
            if newer && choice == IDYES {
                self.open_release()?;
            }
        } else if matches!(result, Ok(Some(_))) {
            self.notify_update()?;
        }
        Ok(())
    }
    fn notify_update(&self) -> Result<(), String> {
        if !self.tray_present || self.update_tag.is_none() {
            return Ok(());
        }
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_INFO,
            dwInfoFlags: NIIF_INFO,
            ..Default::default()
        };
        let title = wide("WallpaperLive update available");
        let info = wide(&format!(
            "A newer version is available ({}). Click to open its GitHub release page. No automatic download or installation.",
            self.update_tag.as_deref().unwrap_or_default()
        ));
        let title_len = title.len().min(data.szInfoTitle.len() - 1);
        data.szInfoTitle[..title_len].copy_from_slice(&title[..title_len]);
        let info_len = info.len().min(data.szInfo.len() - 1);
        data.szInfo[..info_len].copy_from_slice(&info[..info_len]);
        // SAFETY: modifies only this application's existing tray icon with fixed-size terminated text.
        if !unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool() {
            return Err(
                "Windows could not show the update notification; use the tray release menu".into(),
            );
        }
        Ok(())
    }
    fn open_release(&self) -> Result<(), String> {
        let url = wide(
            &self
                .update_tag
                .as_ref()
                .map(|tag| format!("https://github.com/Raflyf/WallpaperLive/releases/tag/{tag}"))
                .unwrap_or_else(|| "https://github.com/Raflyf/WallpaperLive/releases".into()),
        );
        // SAFETY: tag was validated by the update checker; no response-provided URL or command is executed.
        let result = unsafe {
            ShellExecuteW(
                self.hwnd,
                w!("open"),
                PCWSTR(url.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize <= 32 {
            return Err("Windows could not open the GitHub release page".into());
        }
        Ok(())
    }
    fn open_website(&self, url: &'static str) -> Result<(), String> {
        if url != PRODUCT_URL
            && url != SUPPORT_URL
            && url != SAWERIA_URL
            && !WALLPAPER_SITES.iter().any(|(_, site)| *site == url)
        {
            return Err("Website is not in the recommendation list".into());
        }
        let url = wide(url);
        // SAFETY: fixed HTTPS allowlist, only dispatched after a user's button click.
        let result = unsafe {
            ShellExecuteW(
                self.hwnd,
                w!("open"),
                PCWSTR(url.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize <= 32 {
            return Err("Windows could not open the website in your default browser".into());
        }
        Ok(())
    }
    fn find_wallpapers(&mut self) -> Result<(), String> {
        self.switch_tab(visuals::Tab::Sources)
    }
    fn retry_thumbnail(&mut self, path: &Path) -> Result<(), String> {
        if !self.library.iter().any(|entry| entry == path) {
            return Err("Wallpaper is not in your library".into());
        }
        validate_media(path)?;
        self.cancel_thumbnails();
        self.thumbnail_failed.remove(path);
        visuals::remove_thumbnail(path);
        self.request_thumbnails();
        self.refresh_library()
    }
    fn show(&mut self) {
        // SAFETY: own panel only; showing a hidden maximized window preserves its state.
        unsafe {
            let _ = ShowWindow(
                self.hwnd,
                if IsIconic(self.hwnd).as_bool() {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
            let _ = visuals::enable_backdrop(self.hwnd);
            let _ = SetForegroundWindow(self.hwnd);
            SendMessageW(
                self.hwnd,
                WM_CHANGEUISTATE,
                WPARAM(UIS_SET as usize | ((UISF_HIDEFOCUS as usize) << 16)),
                LPARAM(0),
            );
            let _ = SetFocus(self.control(PICK));
            let _ = RedrawWindow(
                self.hwnd,
                None,
                None,
                RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN,
            );
        }
        self.request_thumbnails();
    }
    fn tray_menu(&mut self, anchor: WPARAM) -> Result<bool, String> {
        let id = crate::tray::popup(
            self.hwnd,
            anchor,
            &[
                (OPEN, "Open controls"),
                (
                    PAUSE,
                    if self.reasons.manual {
                        "Resume"
                    } else {
                        "Pause"
                    },
                ),
                (
                    visuals::AUDIO,
                    if self.settings.audio_enabled {
                        "Turn sound off"
                    } else {
                        "Turn sound on"
                    },
                ),
                (STOP, "Stop wallpaper"),
                (UPDATE_RELEASE, "Check for updates"),
                (EXIT, "Exit"),
            ],
        )?;
        self.action(id, 0)
    }
    fn release_playback(&mut self) {
        self.playback = None;
        self.playback_info = None;
        visuals::playing(None);
        self.loaded = false;
        self.load_request = 0;
        self.load_entry = None;
        self.file_loaded = None;
        self.video_ready = None;
        self.loading_since = None;
        self.paused_since = None;
        self.pause_trimmed = false;
        self.actual_pause = false;
        self.requested_pause = false;
        // SAFETY: teardown cancels only the timer owned by this controller.
        if !self.multi_dispatch {
            unsafe {
                let _ = KillTimer(self.hwnd, 1);
            }
        }
    }
    fn start(&mut self) -> Result<(), String> {
        let path = self
            .selected
            .clone()
            .ok_or("Select a wallpaper file first")?;
        self.resume_position = 0.0;
        self.start_path(&path)
    }
    fn start_path(&mut self, source: &Path) -> Result<(), String> {
        let path = validate_media(source)?;
        let root = wide(&format!(
            "{}\\",
            &path.to_str().ok_or("Invalid file name")?[..2]
        ));
        // SAFETY: read-only local drive check; no remote media or command execution.
        if !matches!(unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) }, 2 | 3 | 5) {
            return Err("Media must be on a local drive, not a network drive".into());
        }
        if !self.multi_dispatch && !self.monitors.is_empty() {
            return self.start_targets(&path);
        }
        self.recovery = None;
        let rect = self.screen().rect;
        let display = (
            (rect.right - rect.left) as u32,
            (rect.bottom - rect.top) as u32,
        );
        self.cancel_preparation();
        self.cancel_thumbnails();
        self.dormant = None;
        self.paused_since = None;
        self.pause_trimmed = false;
        self.update_reasons();
        let prepared = crate::cache::Prepared {
            path: path.clone(),
            width: 0,
            height: 0,
            fps: 0.0,
            cached: false,
            note: "Original media".into(),
        };
        self.load_prepared(path.clone(), prepared, self.resume_position)?;
        self.preparation = Some((path, display, crate::cache::CACHE_FPS));
        self.launch_preparation()?;
        self.update_status();
        Ok(())
    }
    fn cancel_preparation(&mut self) {
        if let Some((cancelled, _)) = &self.preparation_thread {
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.preparation_id = self.preparation_id.wrapping_add(1);
        self.preparation = None;
        self.prepared_waiting = None;
        while self.preparation_rx.try_recv().is_ok() {}
    }
    fn launch_preparation(&mut self) -> Result<(), String> {
        if self
            .preparation_thread
            .as_ref()
            .is_some_and(|(_, worker)| worker.is_finished())
        {
            if let Some((_, worker)) = self.preparation_thread.take() {
                let _ = worker.join();
            }
            self.prepared()?;
        }
        if self.cache_busy || self.preparation_thread.is_some() || self.reasons.reason().is_some() {
            return Ok(());
        }
        let Some((path, display, fps)) = self.preparation.clone() else {
            return Ok(());
        };
        let dll = decoder_path()?;
        let directory = self
            .config
            .parent()
            .ok_or("Invalid cache directory")?
            .join("video-cache");
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cancelled.clone();
        let tx = self.preparation_tx.clone();
        let id = self.preparation_id;
        let hwnd = self.hwnd.0 as usize;
        let worker = std::thread::spawn(move || {
            let result = crate::cache::prepare(&dll, &path, &directory, display, fps, &flag);
            if tx.send((id, result)).is_ok() {
                // SAFETY: worker only posts completion; App owns and joins its lifetime on exit.
                unsafe {
                    let _ = PostMessageW(HWND(hwnd as *mut _), PREPARED, WPARAM(0), LPARAM(0));
                }
            }
        });
        self.preparation_thread = Some((cancelled, worker));
        Ok(())
    }
    fn prepared(&mut self) -> Result<(), String> {
        while let Ok((id, result)) = self.preparation_rx.try_recv() {
            if id != self.preparation_id {
                continue;
            }
            let Some((path, display, _)) = self.preparation.take() else {
                continue;
            };
            let prepared = match result {
                Ok(prepared) => prepared,
                Err(error) => {
                    self.error = Some(format!(
                        "Original playback kept; display cache unavailable: {error}"
                    ));
                    continue;
                }
            };
            let rect = self.screen().rect;
            if display
                != (
                    (rect.right - rect.left) as u32,
                    (rect.bottom - rect.top) as u32,
                )
            {
                return self.start_path(&path);
            }
            if !prepared.cached {
                if self.playback.as_ref().is_some_and(|p| p.path == path) {
                    self.playback_info = Some(prepared);
                }
                continue;
            }
            self.prepared_waiting = Some((Instant::now(), path, prepared));
        }
        self.update_status();
        Ok(())
    }
    fn load_prepared(
        &mut self,
        path: PathBuf,
        prepared: crate::cache::Prepared,
        position: f64,
    ) -> Result<(), String> {
        let rect = self.screen().rect;
        let audio_enabled = self.display_audio();
        let request;
        if let Some(playback) = &mut self.playback {
            playback.surface.place(rect)?;
            playback.player.pause(true)?;
            let _ = playback.player.poll();
            playback.player.set_audio(audio_enabled)?;
            playback.audio_enabled = audio_enabled;
            request = playback.player.load(&prepared.path)?;
            playback.path = path;
        } else {
            #[cfg(not(test))]
            let surface = Surface::create(rect)?;
            #[cfg(test)]
            let surface = Surface::isolated(self.hwnd, rect)?;
            let player =
                Mpv::open_at(&decoder_path()?, surface.hwnd, self.hwnd, self.settings.fit)?;
            player.pause(true)?;
            player.set_audio(audio_enabled)?;
            request = player.load(&prepared.path)?;
            self.playback = Some(Playback {
                player,
                surface,
                path,
                audio_enabled,
            });
        }
        self.playback_info = Some(prepared);
        self.resume_position = position;
        self.requested_pause = true;
        self.loaded = false;
        self.load_request = request;
        self.load_entry = None;
        self.file_loaded = None;
        self.video_ready = None;
        self.loading_since = Some(Instant::now());
        self.error = None;
        self.arm_monitor()?;
        self.request_thumbnails();
        self.update_status();
        Ok(())
    }
    fn stop(&mut self) -> Result<(), String> {
        if !self.multi_dispatch && !self.displays.is_empty() {
            let previous = self.settings.clone();
            self.settings.path = None;
            self.settings.wallpapers.clear();
            if let Err(error) = self.save() {
                self.settings = previous;
                return Err(error);
            }
            for index in 0..self.displays.len() {
                self.with_display(index, |app| {
                    app.cancel_preparation();
                    app.recovery = None;
                    app.dormant = None;
                    app.release_playback();
                    Ok(())
                })?;
            }
            self.displays
                .retain(|display| display.preparation_thread.is_some());
            self.reasons.manual = false;
            self.audio_monitor = None;
            visuals::playing_all(&[]);
            self.error = None;
            if !self.displays.is_empty() || self.thumbnail_thread.is_some() {
                self.arm_monitor()?;
            }
            self.refresh_library()?;
            self.update_status();
            return Ok(());
        }
        self.cancel_preparation();
        self.dormant = None;
        self.recovery = None;
        self.release_playback();
        self.reasons.manual = false;
        self.settings.path = None;
        unsafe {
            let _ = KillTimer(self.hwnd, 1);
        }
        if self.preparation_thread.is_some() || self.thumbnail_thread.is_some() {
            self.arm_monitor()?;
        }
        self.save()?;
        self.error = None;
        self.update_status();
        Ok(())
    }
    fn update_reasons(&mut self) {
        if self.monitors.is_empty() {
            return;
        }
        // SAFETY: read-only system and window queries; never injects into other processes.
        unsafe {
            if !self.multi_dispatch {
                let mut status = SYSTEM_POWER_STATUS::default();
                if GetSystemPowerStatus(&mut status).is_ok() {
                    self.reasons.battery = self.settings.pause_battery && status.ACLineStatus == 0;
                }
                self.reasons.remote = GetSystemMetrics(SM_REMOTESESSION) != 0;
                if self.wts_tick == 0 || self.reasons.remote || self.reasons.disconnected {
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
                }
                self.wts_tick = (self.wts_tick + 1) % 10;
            }
            let foreground = GetForegroundWindow();
            let mut pid = 0;
            GetWindowThreadProcessId(foreground, Some(&mut pid));
            let mut rect = RECT::default();
            let mut class = [0u16; 64];
            let n = GetClassNameW(foreground, &mut class).clamp(0, class.len() as i32) as usize;
            let eligible = self.settings.pause_fullscreen
                && pid != std::process::id()
                && !foreground.0.is_null()
                && !IsIconic(foreground).as_bool()
                && !is_shell_class(&class[..n])
                && GetWindowRect(foreground, &mut rect).is_ok();
            let area = Rect {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            };
            let display = self.screen().rect;
            self.reasons
                .foreground(eligible, IsZoomed(foreground).as_bool(), area, display);
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
        if !self.multi_dispatch && !self.displays.is_empty() {
            return self.tick_displays();
        }
        self.update_reasons();
        self.prepared()?;
        self.events()?;
        let paused = self.reasons.reason().is_some();
        if self.loaded && self.requested_pause != paused {
            if !paused && let Some((due, _, _)) = &mut self.prepared_waiting {
                *due = Instant::now() + Duration::from_secs(2);
            }
            if let Some(playback) = &self.playback {
                playback.player.pause(paused)?;
            }
            self.requested_pause = paused;
        }
        if paused {
            if let Some((_, path, _)) = self.prepared_waiting.take() {
                self.cancel_preparation();
                let rect = self.screen().rect;
                self.preparation = Some((
                    path,
                    (
                        (rect.right - rect.left) as u32,
                        (rect.bottom - rect.top) as u32,
                    ),
                    crate::cache::CACHE_FPS,
                ));
            }
            if self.preparation.is_some()
                && self
                    .preparation_thread
                    .as_ref()
                    .is_some_and(|(cancelled, _)| {
                        !cancelled.load(std::sync::atomic::Ordering::Relaxed)
                    })
            {
                let preparation = self.preparation.take();
                self.cancel_preparation();
                self.preparation = preparation;
            }
            if !self.multi_dispatch {
                self.cancel_thumbnails();
            }
            self.launch_preparation()?;
            self.request_thumbnails();
            if self.loaded && self.requested_pause && self.dormant.is_none() {
                let since = *self.paused_since.get_or_insert_with(Instant::now);
                if let Some(grace) = dormant_grace(&self.reasons) {
                    if since.elapsed() >= grace
                        && let Some(playback) = &self.playback
                    {
                        let position = playback_position(playback).unwrap_or(0.0);
                        let path = playback.path.clone();
                        self.release_playback();
                        self.dormant = Some(Dormant { path, position });
                        if !self.multi_dispatch {
                            self.arm_monitor()?;
                        }
                        // SAFETY: pseudo-handle for current process; best-effort working-set trim when idle.
                        unsafe {
                            let _ = SetProcessWorkingSetSize(
                                GetCurrentProcess(),
                                usize::MAX,
                                usize::MAX,
                            );
                        }
                    }
                } else if !self.pause_trimmed && since.elapsed() >= Duration::from_secs(1) {
                    self.pause_trimmed = true;
                    // SAFETY: trim resident working set after pause settles while keeping playback/frame ready.
                    unsafe {
                        let _ =
                            SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
                    }
                }
            } else {
                self.paused_since = None;
                self.pause_trimmed = false;
            }
            self.update_status();
            return Ok(());
        }
        self.paused_since = None;
        self.pause_trimmed = false;
        if (self.playback.is_none() || self.loaded)
            && !self.requested_pause
            && self
                .prepared_waiting
                .as_ref()
                .is_some_and(|(due, _, _)| Instant::now() >= *due)
            && let Some((_, path, prepared)) = self.prepared_waiting.take()
        {
            let position = self
                .playback
                .as_ref()
                .and_then(playback_position)
                .unwrap_or(0.0);
            self.load_prepared(path, prepared, position)?;
        }
        self.launch_preparation()?;
        if let Some(dormant) = self.dormant.take() {
            self.resume_position = dormant.position;
            self.start_path(&dormant.path)?;
        }
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
        if self
            .loading_since
            .is_some_and(|s| s.elapsed() > Duration::from_secs(15))
        {
            self.cancel_preparation();
            self.release_playback();
            self.arm_monitor()?;
            return Err("Media did not finish loading. Try another file or decoder.".into());
        }
        self.request_thumbnails();
        self.update_status();
        if self.playback.is_none()
            && self.preparation.is_none()
            && self.recovery.is_none()
            && self.preparation_thread.is_none()
            && self.prepared_waiting.is_none()
            && self.thumbnail_thread.is_none()
        {
            unsafe {
                let _ = KillTimer(self.hwnd, 1);
            }
        }
        Ok(())
    }
    fn events(&mut self) -> Result<(), String> {
        let events = self
            .playback
            .as_ref()
            .map(|p| p.player.poll())
            .unwrap_or_default();
        for event in events {
            self.event(event)?;
        }
        self.finish_load()?;
        self.update_status();
        Ok(())
    }
    fn event(&mut self, event: Event) -> Result<(), String> {
        match event {
            Event::LoadAccepted { request, entry } if request == self.load_request => {
                self.load_entry = Some(entry)
            }
            Event::LoadAccepted { .. } => {}
            Event::Started(entry) => {
                if self.load_entry == Some(entry) || self.load_entry.is_none() {
                    self.file_loaded = None;
                    self.video_ready = None;
                }
            }
            Event::Loaded(entry) => self.file_loaded = entry,
            Event::Ready(entry) => self.video_ready = entry,
            Event::FileFailed { entry, error } if self.load_entry == Some(entry) => {
                self.cancel_preparation();
                self.release_playback();
                self.arm_monitor()?;
                return Err(error);
            }
            Event::FileFailed { .. } => {}
            Event::Failed(error) => {
                self.cancel_preparation();
                self.release_playback();
                self.arm_monitor()?;
                return Err(error);
            }
            Event::FrameRateFailed(error) => {
                self.error = Some(format!("Frame-rate change failed; wallpaper kept: {error}"))
            }
            Event::Pause(paused) => {
                if paused && !self.actual_pause && self.loaded && self.reasons.reason().is_some() {
                    // ponytail: trims resident pages, not decoder allocations; retain frame for fast resume.
                    unsafe {
                        let _ =
                            SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
                    }
                }
                self.actual_pause = paused;
            }
        }
        Ok(())
    }
    fn finish_load(&mut self) -> Result<(), String> {
        if self.loaded
            || self.load_entry.is_none()
            || self.file_loaded != self.load_entry
            || self.video_ready != self.load_entry
        {
            return Ok(());
        }
        let Some(playback) = self.playback.as_ref() else {
            return Ok(());
        };
        if playback.player.property(c"video-format").is_none()
            || self.playback_info.as_ref().is_none_or(|info| {
                playback
                    .player
                    .property(c"path")
                    .is_none_or(|path| Path::new(&path) != info.path)
            })
        {
            return Ok(());
        }
        if let Err(error) = playback.surface.prepare_renderer() {
            self.release_playback();
            return Err(error);
        }
        if self.resume_position > 0.0 {
            playback.player.seek(self.resume_position)?;
            self.resume_position = 0.0;
        }
        let source_fps = effective_source_fps(self.playback_info.as_ref(), playback);
        playback.player.set_fps(self.settings.fps, source_fps)?;
        if let Some(info) = &mut self.playback_info {
            info.fps = source_fps;
            if let Some(width) = playback
                .player
                .property(c"video-params/w")
                .and_then(|s| s.parse::<u32>().ok())
            {
                info.width = width;
            }
            if let Some(height) = playback
                .player
                .property(c"video-params/h")
                .and_then(|s| s.parse::<u32>().ok())
            {
                info.height = height;
            }
        }
        let paused = self.reasons.reason().is_some();
        playback.player.pause(paused)?;
        self.requested_pause = paused;
        playback.surface.show();
        self.settings.path = Some(playback.path.clone());
        if let Some(monitor) = &self.active_monitor {
            self.settings.wallpapers.retain(|(name, _)| name != monitor);
            self.settings
                .wallpapers
                .push((monitor.clone(), playback.path.clone()));
        }
        self.loaded = true;
        visuals::playing(Some(playback.path.clone()));
        for i in 0..9 {
            unsafe {
                let _ = InvalidateRect(self.control(visuals::TILE_BASE + i), None, false);
            }
        }
        self.loading_since = None;
        // ponytail: trim one-off decoder and startup allocations now that playback is active and steady.
        unsafe {
            let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
        }
        if let Err(error) = self.save() {
            self.error = Some(format!("Playing, but settings were not saved: {error}"));
        }
        Ok(())
    }
    fn update_status(&self) {
        if self.multi_dispatch {
            return;
        }
        let status = if let Some(error) = &self.error {
            format!("Error: {error}")
        } else if self.displays.iter().any(DisplayState::active) {
            let ready = self
                .displays
                .iter()
                .filter(|d| d.loaded && d.playback.is_some())
                .count();
            let paused = self
                .displays
                .iter()
                .filter(|d| d.loaded && (d.requested_pause || d.actual_pause))
                .count();
            let loading = self
                .displays
                .iter()
                .filter(|d| d.playback.is_some() && !d.loaded)
                .count();
            let waiting = self
                .displays
                .iter()
                .filter(|d| d.recovery.is_some() && d.playback.is_none())
                .count();
            let preparation = if self
                .displays
                .iter()
                .any(|d| d.preparation.is_some() || d.prepared_waiting.is_some())
            {
                " Preparing display cache in background; temporary RAM may be higher."
            } else {
                ""
            };
            let reason = self
                .reasons
                .reason()
                .or_else(|| self.displays.iter().find_map(|d| d.pause_reason))
                .unwrap_or("Auto-pause per display");
            format!(
                "{} playing, {paused} paused, {loading} loading, {waiting} waiting. {} FPS. Sound: {}. {reason}.{preparation}",
                ready.saturating_sub(paused),
                self.settings.fps,
                if self.settings.audio_enabled {
                    "On (one display)"
                } else {
                    "Off"
                }
            )
        } else if self.recovery.is_some() {
            "Desktop changed. Restoring wallpaper...".into()
        } else if self.reasons.reason().is_some() && self.playback.is_some() {
            format!(
                "{}. Last frame kept; resume does not reload.",
                self.reasons.reason().unwrap_or("Paused")
            )
        } else if self.preparation.is_some() {
            "Playing original; preparing display cache in background. Temporary RAM may be higher."
                .into()
        } else if self.dormant.is_some() {
            format!(
                "{}. Decoder released; resume reloads wallpaper.",
                self.reasons.reason().unwrap_or("Waiting to resume")
            )
        } else if self.playback.is_none() {
            "Choose a file from your library, then Set wallpaper.".into()
        } else if !self.loaded {
            "Loading media...".into()
        } else if let Some(reason) = self.reasons.reason() {
            reason.into()
        } else if self.actual_pause {
            "Resuming playback...".into()
        } else {
            let detail = self
                .playback_info
                .as_ref()
                .map(|p| {
                    format!(
                        "{}: {} x {}, cap {:.1} FPS. ",
                        p.note,
                        p.width,
                        p.height,
                        if p.fps > 0.0 {
                            p.fps.min(f64::from(self.settings.fps))
                        } else {
                            f64::from(self.settings.fps)
                        }
                    )
                })
                .unwrap_or_default();
            format!(
                "{detail}Sound: {}. Auto-pause: {}.",
                if self.settings.audio_enabled {
                    "On"
                } else {
                    "Off"
                },
                if self.settings.pause_fullscreen {
                    "On"
                } else {
                    "Off"
                }
            )
        };
        if self.status_cache.borrow().0 != status {
            self.text(STATUS, &status);
        }
        let label = if self.reasons.manual {
            "Resume"
        } else {
            "Pause"
        };
        if self.status_cache.borrow().1 != label {
            self.text(PAUSE, label);
            self.status_cache.borrow_mut().1 = label;
        }
        let sound = if self.settings.audio_enabled {
            "Sound: On"
        } else {
            "Sound: Off"
        };
        if self.status_cache.borrow().2 != sound {
            self.text(visuals::AUDIO, sound);
            self.status_cache.borrow_mut().2 = sound;
        }
        let active = self.playback.is_some()
            || self.recovery.is_some()
            || self.preparation.is_some()
            || self.dormant.is_some()
            || self.displays.iter().any(DisplayState::active);
        if self.status_cache.borrow().3 != Some(active) {
            unsafe {
                let _ = EnableWindow(self.control(PAUSE), active);
                let _ = EnableWindow(self.control(STOP), active);
            }
            self.status_cache.borrow_mut().3 = Some(active);
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
    fn import_folder(&mut self, source: &Path) -> Result<(), String> {
        let files = crate::library::scan_folder(source)?;
        if files.is_empty() {
            return Err("No supported wallpaper files found in this folder".into());
        }
        let added: Vec<_> = files
            .iter()
            .filter(|file| !self.library.contains(file))
            .cloned()
            .collect();
        let mut library = added;
        library.extend(self.library.iter().cloned());
        if library.len() > crate::library::MAX_FILES {
            return Err("Library would exceed 4096 files; existing entries were kept".into());
        }
        let root = source.canonicalize().map_err(|e| e.to_string())?;
        let root = root.to_str().ok_or("Folder name is not valid Unicode")?;
        let root = PathBuf::from(root.strip_prefix(r"\\?\").unwrap_or(root));
        let previous = self.folder_roots.clone();
        if !self.folder_roots.contains(&root) {
            self.folder_roots.push(root.clone());
        }
        if let Err(error) = self.save_library(&library) {
            self.folder_roots = previous;
            return Err(error);
        }
        self.library = library;
        self.folder_filter = Some(root);
        self.marked.clear();
        self.selected = files.first().cloned();
        self.page = 0;
        self.error = None;
        self.refresh_library()?;
        self.update_status();
        Ok(())
    }
    fn pick_folder(&mut self) -> Result<(), String> {
        let folder = unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .map_err(native_error)?;
            let result = (|| -> Result<Option<PathBuf>, String> {
                let dialog: IFileOpenDialog =
                    CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
                        .map_err(native_error)?;
                dialog
                    .SetOptions(
                        FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR,
                    )
                    .map_err(native_error)?;
                dialog
                    .SetTitle(w!("Add wallpaper folder"))
                    .map_err(native_error)?;
                if let Err(error) = dialog.Show(self.hwnd) {
                    if error.code().0 == 0x800704c7u32 as i32 {
                        return Ok(None);
                    }
                    return Err(native_error(error));
                }
                let item = dialog.GetResult().map_err(native_error)?;
                let path = item
                    .GetDisplayName(SIGDN_FILESYSPATH)
                    .map_err(native_error)?;
                let text = path.to_string().map_err(|e| e.to_string());
                CoTaskMemFree(Some(path.0.cast()));
                Ok(Some(PathBuf::from(text?)))
            })();
            CoUninitialize();
            result?
        };
        if let Some(folder) = folder {
            self.import_folder(&folder)?;
        }
        Ok(())
    }
    fn selector(
        &self,
        id: usize,
        labels: &[String],
        selected: usize,
    ) -> Result<Option<usize>, String> {
        let mut rect = RECT::default();
        // SAFETY: read-only screen bounds of owned child selector button.
        unsafe {
            GetWindowRect(self.control(id), &mut rect).map_err(native_error)?;
        }
        let items: Vec<(usize, &str)> = labels
            .iter()
            .enumerate()
            .map(|(index, label)| (index + 1, label.as_str()))
            .collect();
        let choice = crate::tray::dropdown_at(self.hwnd, rect, &items, Some(selected + 1))?;
        Ok(choice.checked_sub(1))
    }
    fn change_fps(&mut self, fps: u32) -> Result<(), String> {
        if !crate::core::FPS_OPTIONS.contains(&fps) {
            return Err("Invalid frame-rate limit".into());
        }
        if self.settings.fps == fps {
            return Ok(());
        }
        let old = self.settings.fps;
        self.settings.fps = fps;
        if let Err(error) = self.save() {
            self.settings.fps = old;
            return Err(error);
        }
        self.text(visuals::FPS, &format!("Frame rate: {fps} FPS"));
        for display in &self.displays {
            if display.loaded
                && let Some(playback) = &display.playback
            {
                playback.player.set_fps(
                    fps,
                    display.playback_info.as_ref().map(|p| p.fps).unwrap_or(0.0),
                )?;
            }
        }
        if self.loaded
            && let Some(playback) = &self.playback
        {
            let source_fps = effective_source_fps(self.playback_info.as_ref(), playback);
            playback.player.set_fps(fps, source_fps)?;
        }
        self.update_status();
        Ok(())
    }
    fn action(&mut self, id: usize, notification: u32) -> Result<bool, String> {
        if id == visuals::SEARCH && notification == EN_CHANGE {
            let search = window_text(self.control(visuals::SEARCH));
            if search != self.search {
                self.search = search;
                self.marked.clear();
                self.last_tile_click = None;
                self.page = 0;
                self.refresh_library()?;
            }
            return Ok(false);
        }
        if notification != BN_CLICKED {
            return Ok(false);
        }
        visuals::cancel_transition(self.hwnd);
        match id {
            visuals::FIND_WALLPAPERS => self.find_wallpapers()?,
            visuals::SOURCES_BACK => self.switch_tab(visuals::Tab::Library)?,
            id if (visuals::SITE_BASE..visuals::SITE_BASE + WALLPAPER_SITES.len())
                .contains(&id) =>
            {
                self.open_website(WALLPAPER_SITES[id - visuals::SITE_BASE].1)?;
            }
            visuals::PRODUCT_WEBSITE => self.open_website(PRODUCT_URL)?,
            visuals::SUPPORT => self.open_website(SUPPORT_URL)?,
            visuals::SAWERIA => self.open_website(SAWERIA_URL)?,
            visuals::SORT => {
                let options = crate::library::Sort::OPTIONS;
                let labels: Vec<_> = options.iter().map(|sort| sort.label().to_owned()).collect();
                let selected = options
                    .iter()
                    .position(|sort| *sort == self.sort)
                    .unwrap_or(0);
                if let Some(index) = self.selector(visuals::SORT, &labels, selected)?
                    && let Some(sort) = options.get(index)
                {
                    self.sort = *sort;
                    self.last_tile_click = None;
                    self.page = 0;
                    self.refresh_library()?;
                }
            }
            UPDATE_RELEASE => self.check_updates(true)?,
            visuals::TAB_LIBRARY => self.switch_tab(visuals::Tab::Library)?,
            visuals::TAB_SETTINGS => self.switch_tab(visuals::Tab::Settings)?,
            visuals::CLEAR_CACHE => self.clear_cache()?,
            visuals::ADD_FOLDER => self.pick_folder()?,
            visuals::FOLDER_FILTER => {
                let folders = self.folder_options();
                let labels: Vec<_> = std::iter::once("All folders".to_owned())
                    .chain(folders.iter().map(|p| p.to_string_lossy().into_owned()))
                    .collect();
                let selected = self
                    .folder_filter
                    .as_ref()
                    .and_then(|p| folders.iter().position(|f| f == p))
                    .map(|i| i + 1)
                    .unwrap_or(0);
                if let Some(index) = self.selector(visuals::FOLDER_FILTER, &labels, selected)? {
                    self.folder_filter = index.checked_sub(1).and_then(|i| folders.get(i)).cloned();
                    self.marked.clear();
                    self.page = 0;
                    self.selected = self.library_view().first().cloned();
                    self.refresh_library()?;
                }
            }
            visuals::SELECT_ALL => {
                let view = self.library_view();
                if view.iter().all(|p| self.marked.contains(p)) {
                    self.marked.clear();
                } else {
                    self.marked = view;
                }
                self.refresh_library()?;
            }
            visuals::FPS => {
                let labels: Vec<_> = crate::core::FPS_OPTIONS
                    .iter()
                    .map(|fps| format!("{fps} FPS"))
                    .collect();
                let selected = crate::core::FPS_OPTIONS
                    .iter()
                    .position(|fps| *fps == self.settings.fps)
                    .unwrap_or(1);
                if let Some(index) = self.selector(visuals::FPS, &labels, selected)?
                    && let Some(fps) = crate::core::FPS_OPTIONS.get(index)
                {
                    self.change_fps(*fps)?;
                }
            }
            visuals::PREVIOUS => self.step_page(-1)?,
            visuals::NEXT => self.step_page(1)?,
            id if (visuals::TILE_BASE..visuals::TILE_BASE + 9).contains(&id) => {
                if let Some(path) = self
                    .library_view()
                    .get(self.page * 9 + id - visuals::TILE_BASE)
                    .cloned()
                {
                    let double_click =
                        self.last_tile_click
                            .as_ref()
                            .is_some_and(|(prev_id, time)| {
                                *prev_id == id
                                    && time.elapsed()
                                        <= Duration::from_millis(
                                            unsafe { GetDoubleClickTime() } as u64
                                        )
                            });
                    if double_click {
                        self.last_tile_click = None;
                        self.selected = Some(path.clone());
                        self.marked = vec![path];
                        self.error = None;
                        self.refresh_library()?;
                        self.recovery = None;
                        self.start()?;
                        return Ok(false);
                    }
                    self.last_tile_click = Some((id, Instant::now()));

                    if unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0 {
                        if let Some(index) = self.marked.iter().position(|p| p == &path) {
                            self.marked.remove(index);
                        } else {
                            self.marked.push(path.clone());
                        }
                    } else {
                        self.marked = vec![path.clone()];
                    }
                    self.selected = Some(path);
                    self.error = None;
                    self.refresh_library()?;
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
            visuals::AUDIO => {
                let old = self.settings.audio_enabled;
                let enabled = !old;
                self.settings.audio_enabled = enabled;
                if let Err(error) = self.save() {
                    self.settings.audio_enabled = old;
                    return Err(error);
                }
                if let Some(playback) = &self.playback {
                    playback.player.set_audio(enabled)?;
                }
                self.sync_audio()?;
                self.update_status();
            }
            visuals::REMOVE => {
                if self.marked.is_empty() {
                    return Ok(false);
                }
                let prompt = wide(&format!(
                    "Remove {} selected wallpaper(s) from the library?\n\nOriginal media files will not be deleted. Any selected active or preparing wallpaper will be stopped.",
                    self.marked.len()
                ));
                if unsafe {
                    MessageBoxW(
                        self.hwnd,
                        PCWSTR(prompt.as_ptr()),
                        w!("Remove wallpaper"),
                        MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
                    )
                } == IDYES
                {
                    self.remove_selected()?;
                    self.refresh_library()?;
                    self.update_status();
                }
            }
            121 => {
                self.settings.pause_fullscreen =
                    unsafe { SendMessageW(self.control(121), BM_GETCHECK, WPARAM(0), LPARAM(0)).0 }
                        == BST_CHECKED.0 as isize;
                self.save()?;
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
                    self.cancel_thumbnails();
                    visuals::retain_thumbnails(&[]);
                    // ponytail: trim panel memory while running in tray; active display keeps running.
                    unsafe {
                        let _ =
                            SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
                    }
                } else {
                    return Ok(true);
                }
            }
            SCREEN => {
                let mut labels: Vec<_> = self
                    .monitors
                    .iter()
                    .enumerate()
                    .map(|(i, monitor)| {
                        format!(
                            "Display {}  {} x {}{}",
                            i + 1,
                            monitor.rect.right - monitor.rect.left,
                            monitor.rect.bottom - monitor.rect.top,
                            if monitor.primary { " (primary)" } else { "" }
                        )
                    })
                    .collect();
                labels.insert(0, format!("All displays ({})", self.monitors.len()));
                let selected = if self.settings.monitor == "*" {
                    0
                } else {
                    self.monitors
                        .iter()
                        .position(|m| m.name == self.settings.monitor)
                        .map(|index| index + 1)
                        .unwrap_or(1)
                };
                if let Some(index) = self.selector(SCREEN, &labels, selected)? {
                    let monitor = if index == 0 {
                        "*".to_owned()
                    } else {
                        self.monitors
                            .get(index - 1)
                            .ok_or("Display selection changed")?
                            .name
                            .clone()
                    };
                    let old = std::mem::replace(&mut self.settings.monitor, monitor);
                    if let Err(error) = self.save() {
                        self.settings.monitor = old;
                        return Err(error);
                    }
                    self.text(SCREEN, &labels[index]);
                    self.update_status();
                }
            }
            FIT => {
                let labels = [
                    "Fill (crop to screen)".to_owned(),
                    "Fit (show full frame)".to_owned(),
                ];
                if let Some(index) =
                    self.selector(FIT, &labels, usize::from(self.settings.fit == Fit::Contain))?
                {
                    let fit = if index == 0 { Fit::Fill } else { Fit::Contain };
                    let old = self.settings.fit;
                    self.settings.fit = fit;
                    if let Err(error) = self.save() {
                        self.settings.fit = old;
                        return Err(error);
                    }
                    for display in &self.displays {
                        if let Some(playback) = &display.playback {
                            playback.player.set_fit(fit)?;
                        }
                    }
                    if let Some(playback) = &self.playback {
                        playback.player.set_fit(fit)?;
                    }
                    self.text(FIT, &labels[index]);
                }
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
    fn switch_tab(&mut self, tab: visuals::Tab) -> Result<(), String> {
        let previous = self.tab;
        if previous != tab {
            visuals::cancel_transition(self.hwnd);
        }
        let clip = if previous != tab {
            self.body_clip(false)
        } else {
            None
        };
        let old_layer = clip.and_then(|c| visuals::capture_region(self.hwnd, c, &self.controls));
        let repaint_now = old_layer.is_none();

        if previous != tab {
            visuals::start_tab_pill_anim(self.hwnd, previous, tab);
        }
        self.tab = tab;
        visuals::set_tab(tab);
        if tab == visuals::Tab::Library {
            self.refresh_library_with_repaint(repaint_now)?;
        } else {
            self.cancel_thumbnails();
            if tab == visuals::Tab::Settings {
                self.refresh_diagnostics();
            }
            self.apply_layout(repaint_now)?;
        }
        let focus = match tab {
            visuals::Tab::Sources => visuals::SOURCES_BACK,
            visuals::Tab::Settings => visuals::TAB_SETTINGS,
            visuals::Tab::Library if previous == visuals::Tab::Sources => visuals::FIND_WALLPAPERS,
            visuals::Tab::Library => visuals::TAB_LIBRARY,
        };
        if let (Some(c), Some(old)) = (clip, old_layer) {
            let hidden = self.visible_controls_for(true);
            // SAFETY: keep focus on a non-hidden HWND during the 200ms body slide, then restore target focus.
            unsafe {
                let holder = if visuals::is_body_control(focus) {
                    self.hwnd
                } else {
                    self.control(focus)
                };
                let _ = SetFocus(holder);
            }
            if let Some(new) = visuals::capture_region(self.hwnd, c, &self.controls) {
                let dir = if previous == visuals::Tab::Library {
                    -1
                } else {
                    1
                };
                visuals::begin_transition(
                    self.hwnd,
                    c,
                    dir,
                    visuals::TAB_TRANSITION_MS,
                    old,
                    new,
                    hidden,
                    Some(self.control(focus)),
                    true,
                );
                // SAFETY: invalidate header tab pills on frame 0 so crossfade begins immediately.
                unsafe {
                    let _ = InvalidateRect(self.control(visuals::TAB_LIBRARY), None, false);
                    let _ = InvalidateRect(self.control(visuals::TAB_SETTINGS), None, false);
                }
                return Ok(());
            }
        }
        // SAFETY: move focus off controls hidden by this tab change and force clean repaint.
        unsafe {
            let _ = SetFocus(self.control(focus));
            let _ = InvalidateRect(self.hwnd, None, true);
            let _ = UpdateWindow(self.hwnd);
        }
        Ok(())
    }
}
impl Drop for App {
    fn drop(&mut self) {
        if let Some((cancelled, _)) = &self.update_thread {
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.cancel_preparation();
        if let Some((_, worker)) = self.preparation_thread.take() {
            let _ = worker.join();
        }
        if let Some((cancelled, worker)) = self.thumbnail_thread.take() {
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = worker.join();
        }
        self.displays.clear();
        self.release_playback();
        if let Some((_, worker)) = self.update_thread.take() {
            let _ = worker.join();
        }
        // SAFETY: unregister only notifications/timer/tray and HWND owned by this App.
        unsafe {
            let _ = KillTimer(self.hwnd, 1);
            let _ = KillTimer(self.hwnd, 2);
            let _ = KillTimer(self.hwnd, visuals::ANIM_TIMER_ID);
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

fn register_control_class(
    instance: HINSTANCE,
    icon: HICON,
    small_icon: HICON,
) -> Result<(), String> {
    // SAFETY: called on the UI thread; icon owners outlive windows of this class.
    unsafe {
        SetCurrentProcessExplicitAppUserModelID(APP_ID).map_err(native_error)?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: CLASS,
            hIcon: icon,
            hIconSm: small_icon,
            hCursor: LoadCursorW(None, IDC_ARROW).map_err(native_error)?,
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err("Could not register the control window".into());
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
        let icon = Icon::load(GetSystemMetrics(SM_CXICON))?;
        let small_icon = Icon::load(GetSystemMetrics(SM_CXSMICON))?;
        register_control_class(instance.into(), icon.0, small_icon.0)?;
        let dpi = GetDpiForSystem() as i32;
        let hwnd = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            CLASS,
            w!("WallpaperLive"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1040 * dpi / 96,
            720 * dpi / 96,
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
        let (library, folder_roots, library_error) =
            match fs::read_to_string(config.with_file_name("recent.txt")) {
                Ok(text) => match crate::library::decode(&text) {
                    Ok((files, roots)) => (files, roots, None),
                    Err(error) => (
                        Vec::new(),
                        Vec::new(),
                        Some(format!("Existing library was kept: {error}")),
                    ),
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    (Vec::new(), Vec::new(), None)
                }
                Err(error) => (
                    Vec::new(),
                    Vec::new(),
                    Some(format!(
                        "Could not read library; existing data was kept: {error}"
                    )),
                ),
            };
        let config_valid = config_valid && library_error.is_none();
        let error = error.or(library_error);
        let (thumbnail_tx, thumbnail_rx) = std::sync::mpsc::channel();
        let (preparation_tx, preparation_rx) = std::sync::mpsc::channel();
        let (update_tx, update_rx) = std::sync::mpsc::channel();
        let mut app = App {
            hwnd,
            controls: vec![],
            font: HFONT::default(),
            icon,
            small_icon,
            monitors: desktop::monitors()?,
            settings,
            selected,
            config,
            config_valid,
            playback: None,
            displays: Vec::new(),
            multi_dispatch: false,
            active_monitor: None,
            audio_monitor: None,
            cache_busy: false,
            reasons: PauseReasons::default(),
            actual_pause: false,
            requested_pause: false,
            loaded: false,
            load_request: 0,
            load_entry: None,
            file_loaded: None,
            video_ready: None,
            loading_since: None,
            recovery: None,
            power: vec![],
            tray_present: false,
            update_tag: None,
            update_manual: false,
            update_tx: update_tx.clone(),
            update_rx,
            update_thread: None,
            error,
            library,
            folder_filter: None,
            search: String::new(),
            sort: crate::library::Sort::default(),
            folder_roots,
            marked: vec![],
            preparation: None,
            preparation_thread: None,
            preparation_id: 0,
            preparation_rx,
            preparation_tx,
            prepared_waiting: None,
            playback_info: None,
            dormant: None,
            resume_position: 0.0,
            paused_since: None,
            pause_trimmed: false,
            wts_tick: 0,
            status_cache: std::cell::RefCell::new((String::new(), "", "", None)),
            page: 0,
            tab: visuals::Tab::Library,
            thumbnail_pending: None,
            thumbnail_thread: None,
            thumbnail_failed: std::collections::HashSet::new(),
            thumbnail_tx,
            thumbnail_rx,
            last_tile_click: None,
        };
        SendMessageW(
            hwnd,
            WM_SETICON,
            WPARAM(ICON_BIG as usize),
            LPARAM(app.icon.0.0 as isize),
        );
        SendMessageW(
            hwnd,
            WM_SETICON,
            WPARAM(ICON_SMALL as usize),
            LPARAM(app.small_icon.0.0 as isize),
        );
        let _ = visuals::enable_backdrop(hwnd);
        app.create_controls()?;
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
        if !app.settings.wallpapers.is_empty() {
            app.restore_displays()?;
        }
        if app.selected.is_some()
            && (app.settings.wallpapers.is_empty() || args.iter().any(|a| a != "--background"))
            && let Err(e) = app.start()
        {
            app.set_error(e);
        }
        if !background || app.error.is_some() || !app.tray_present {
            app.show();
        }
        app.update_status();
        if let Err(error) = app.check_updates(false) {
            app.set_error(error);
        }
        let mut message = MSG::default();
        loop {
            let result = GetMessageW(&mut message, None, 0, 0).0;
            if result == 0 {
                break;
            }
            if result < 0 {
                return Err("Windows message loop failed".into());
            }
            if message.message == WM_KEYDOWN {
                let vk = message.wParam.0 as u16;
                let focus = GetFocus();
                if focus == app.control(visuals::SEARCH)
                    && (vk == VK_SPACE.0 || vk == VK_LEFT.0 || vk == VK_RIGHT.0)
                {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                    continue;
                }
                if app.tab == visuals::Tab::Sources && vk == VK_ESCAPE.0 {
                    if let Err(error) = app.switch_tab(visuals::Tab::Library) {
                        app.set_error(error);
                    }
                    continue;
                }
                if app.tab == visuals::Tab::Library && vk == VK_SPACE.0 {
                    let focused_id = GetDlgCtrlID(focus) as usize;
                    if focus == hwnd
                        || (visuals::TILE_BASE..visuals::TILE_BASE + 9).contains(&focused_id)
                    {
                        app.reasons.manual = !app.reasons.manual;
                        if let Err(error) = app.tick() {
                            app.set_error(error);
                        }
                        continue;
                    }
                } else if app.tab == visuals::Tab::Library && vk == VK_LEFT.0 {
                    let _ = app.step_page(-1);
                    continue;
                } else if app.tab == visuals::Tab::Library && vk == VK_RIGHT.0 {
                    let _ = app.step_page(1);
                    continue;
                }
            }
            if (WM_KEYFIRST..=WM_KEYLAST).contains(&message.message)
                && IsDialogMessageW(hwnd, &message).as_bool()
            {
                continue;
            }
            let _ = TranslateMessage(&message);
            let handled = message.hwnd == hwnd;
            let action: Result<bool, String> = if handled {
                match message.message {
                    CONTEXT_MENU => {
                        app.context_menu(HWND(message.wParam.0 as *mut _), message.lParam)?;
                        Ok(false)
                    }
                    ACTION => app.action(
                        message.wParam.0 & 0xffff,
                        ((message.wParam.0 >> 16) & 0xffff) as u32,
                    ),
                    THUMBNAIL => {
                        app.request_thumbnails();
                        Ok(false)
                    }
                    PREPARED => app.tick().map(|_| false),
                    UPDATE_READY => app.update_ready().map(|_| false),
                    LAYOUT => {
                        app.request_thumbnails();
                        app.layout().map(|_| false)
                    }
                    mpv::WM_MPV => app.tick().map(|_| false),
                    WM_TIMER if message.wParam.0 == 2 => app.update_ready().map(|_| false),
                    WM_TIMER if message.wParam.0 == visuals::ANIM_TIMER_ID => {
                        let _ = visuals::tick_animations(hwnd);
                        Ok(false)
                    }
                    WM_TIMER => app.tick().map(|_| false),
                    POWER => {
                        app.wts_tick = 0;
                        match message.wParam.0 {
                            1 => app.update_reasons(),
                            2 => app.reasons.display_off = message.lParam.0 == 0,
                            3 => app.reasons.suspended = message.lParam.0 != 0,
                            _ => {}
                        }
                        app.tick().map(|_| false)
                    }
                    SESSION => {
                        app.wts_tick = 0;
                        app.reasons.session_event(message.wParam.0 as u32);
                        app.tick().map(|_| false)
                    }
                    DISPLAY => (|| {
                        if app.tray_present {
                            let _ = app.tray(true);
                        }
                        app.displays_changed()?;
                        Ok(false)
                    })(),
                    TRAY => match tray_event(message.lParam.0 as u32) {
                        Some(TrayEvent::Update) => app.open_release().map(|_| false),
                        Some(TrayEvent::Menu) => app.tray_menu(message.wParam),
                        Some(TrayEvent::Open) => {
                            app.show();
                            Ok(false)
                        }
                        None => Ok(false),
                    },
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
                let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
                if !crate::tray::draw(item) {
                    visuals::draw_item(item);
                }
                LRESULT(1)
            }
            WM_MEASUREITEM if lparam.0 != 0 => {
                if crate::tray::measure(
                    &mut *(lparam.0 as *mut windows::Win32::UI::Controls::MEASUREITEMSTRUCT),
                ) {
                    LRESULT(1)
                } else {
                    DefWindowProcW(window, message, wparam, lparam)
                }
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
                visuals::control_colors(message, HDC(wparam.0 as *mut _), HWND(lparam.0 as *mut _))
                    .unwrap_or_else(|| DefWindowProcW(window, message, wparam, lparam))
            }
            WM_COMMAND => {
                post(ACTION, wparam, lparam);
                LRESULT(0)
            }
            WM_CONTEXTMENU => {
                post(CONTEXT_MENU, wparam, lparam);
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
                visuals::cancel_transition(window);
                if wparam.0 == SIZE_MINIMIZED as usize {
                    return LRESULT(0);
                }
                if matches!(wparam.0 as u32, SIZE_RESTORED | SIZE_MAXIMIZED)
                    && visuals::glass_enabled(window)
                {
                    let _ = visuals::enable_backdrop(window);
                    let _ = InvalidateRect(window, None, true);
                }
                post(LAYOUT, WPARAM(0), LPARAM(0));
                LRESULT(0)
            }
            WM_DWMCOMPOSITIONCHANGED
            | WM_DWMCOLORIZATIONCOLORCHANGED
            | WM_THEMECHANGED
            | WM_SETTINGCHANGE => {
                let was_glass = visuals::glass_enabled(window);
                let is_glass = visuals::enable_backdrop(window);
                if message != WM_SETTINGCHANGE || is_glass != was_glass {
                    let _ = RedrawWindow(
                        window,
                        None,
                        None,
                        RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN,
                    );
                }
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
                let _ = visuals::enable_backdrop(window);
                post(LAYOUT, WPARAM(0), LPARAM(0));
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                if lparam.0 != 0 {
                    let info = &mut *(lparam.0 as *mut MINMAXINFO);
                    let dpi = GetDpiForWindow(window).max(96) as i32;
                    let mut rect = RECT {
                        left: 0,
                        top: 0,
                        right: visuals::MIN_CLIENT_WIDTH * dpi / 96,
                        bottom: visuals::MIN_CLIENT_HEIGHT * dpi / 96,
                    };
                    let _ = AdjustWindowRectExForDpi(
                        &mut rect,
                        WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                        false,
                        WS_EX_CONTROLPARENT,
                        dpi as u32,
                    );
                    info.ptMinTrackSize = POINT {
                        x: rect.right - rect.left,
                        y: rect.bottom - rect.top,
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
                if tray_event(lparam.0 as u32).is_some() {
                    post(TRAY, wparam, lparam);
                }
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
    fn tray_hover_and_popup_notifications_do_not_open_controls() {
        let event = |message| (1 << 16) | message;
        for message in [
            WM_MOUSEMOVE,
            WM_LBUTTONDOWN,
            WM_RBUTTONDOWN,
            NIN_POPUPOPEN,
            NIN_POPUPCLOSE,
        ] {
            assert_eq!(tray_event(event(message)), None);
        }
        assert_eq!(tray_event(event(NIN_SELECT)), Some(TrayEvent::Open));
        assert_eq!(
            tray_event(event(NIN_SELECT | NINF_KEY)),
            Some(TrayEvent::Open)
        );
        assert_eq!(tray_event(event(WM_CONTEXTMENU)), Some(TrayEvent::Menu));
        assert_eq!(tray_event((2 << 16) | NIN_SELECT), None);
        assert_eq!(
            tray_event(event(NIN_BALLOONUSERCLICK)),
            Some(TrayEvent::Update)
        );
    }

    #[test]
    fn embedded_brand_icon_decodes_at_windows_sizes() {
        for size in [16, 20, 24, 32, 48, 64, 256] {
            let icon = Icon::load(size).expect("bundled logo must decode as a Windows icon");
            assert!(!icon.0.0.is_null());
        }
    }

    #[test]
    fn taskbar_identity_and_class_icons_are_set_before_window_creation() {
        let icon = Icon::load(32).expect("large icon");
        let small_icon = Icon::load(16).expect("small icon");
        // SAFETY: isolated test process registers then removes an unused class; no app window opens.
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None).expect("module").into();
            register_control_class(instance, icon.0, small_icon.0).expect("control class");
            let mut registered = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                ..Default::default()
            };
            let class_result = GetClassInfoExW(instance, CLASS, &mut registered);
            let identity = GetCurrentProcessExplicitAppUserModelID().expect("taskbar identity");
            let text = identity.to_string();
            CoTaskMemFree(Some(identity.0.cast()));
            UnregisterClassW(CLASS, instance).expect("remove test class");
            class_result.expect("read registered class");
            assert_eq!(registered.hIcon, icon.0);
            assert_eq!(registered.hIconSm, small_icon.0);
            let id = text.expect("taskbar identity text");
            assert_eq!(id, "Raflyf.WallpaperLive");
            assert!(
                include_str!("../installer/WallpaperLive.iss")
                    .contains(&format!("AppUserModelID: \"{id}\""))
            );
        }
    }

    fn fixture() -> App {
        let (thumbnail_tx, thumbnail_rx) = std::sync::mpsc::channel();
        let (preparation_tx, preparation_rx) = std::sync::mpsc::channel();
        let (update_tx, update_rx) = std::sync::mpsc::channel();
        App {
            hwnd: HWND::default(),
            controls: vec![],
            font: HFONT::default(),
            icon: Icon(HICON::default()),
            small_icon: Icon(HICON::default()),
            monitors: vec![],
            settings: Settings::default(),
            selected: None,
            config: PathBuf::new(),
            config_valid: true,
            playback: None,
            displays: Vec::new(),
            multi_dispatch: false,
            active_monitor: None,
            audio_monitor: None,
            cache_busy: false,
            reasons: PauseReasons::default(),
            actual_pause: false,
            requested_pause: false,
            loaded: false,
            load_request: 0,
            load_entry: None,
            file_loaded: None,
            video_ready: None,
            loading_since: None,
            recovery: None,
            power: vec![],
            tray_present: false,
            update_tag: None,
            update_manual: false,
            update_tx,
            update_rx,
            update_thread: None,
            error: None,
            library: vec![],
            folder_filter: None,
            search: String::new(),
            sort: crate::library::Sort::default(),
            folder_roots: vec![],
            marked: vec![],
            preparation: None,
            preparation_thread: None,
            preparation_id: 0,
            preparation_rx,
            preparation_tx,
            prepared_waiting: None,
            playback_info: None,
            dormant: None,
            resume_position: 0.0,
            paused_since: None,
            pause_trimmed: false,
            wts_tick: 0,
            status_cache: std::cell::RefCell::new((String::new(), "", "", None)),
            page: 0,
            tab: visuals::Tab::Library,
            thumbnail_pending: None,
            thumbnail_thread: None,
            thumbnail_failed: std::collections::HashSet::new(),
            thumbnail_rx,
            thumbnail_tx,
            last_tile_click: None,
        }
    }
    unsafe fn proof_thumbnail() -> Result<HBITMAP, String> {
        let mut bits = std::ptr::null_mut();
        let bitmap = unsafe {
            CreateDIBSection(
                None,
                &BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: 320,
                        biHeight: -180,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            )
            .map_err(native_error)?
        };
        let pixels = unsafe { std::slice::from_raw_parts_mut(bits as *mut u32, 320 * 180) };
        for y in 0..180 {
            for x in 0..320 {
                pixels[y * 320 + x] = match x / 80 {
                    0 => 0x00e53935,
                    1 => 0x0043a047,
                    2 => 0x001e88e5,
                    _ => 0x00fdd835,
                };
            }
        }
        Ok(bitmap)
    }

    struct DesktopProof {
        backdrop: HWND,
        previous: HWND,
        instance: HINSTANCE,
    }
    impl DesktopProof {
        unsafe fn new() -> Result<Self, String> {
            // SAFETY: fixture owns this window and class; no desktop or app settings change.
            unsafe {
                let instance: HINSTANCE = GetModuleHandleW(None).map_err(native_error)?.into();
                let class = w!("WallpaperLive.DesktopProofBackdrop");
                if RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(proof_backdrop_proc),
                    hInstance: instance,
                    lpszClassName: class,
                    ..Default::default()
                }) == 0
                {
                    return Err("Could not register desktop proof backdrop".into());
                }
                let backdrop = match CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    class,
                    w!("UI proof backdrop"),
                    WS_POPUP,
                    0,
                    0,
                    1,
                    1,
                    None,
                    None,
                    instance,
                    None,
                ) {
                    Ok(window) => window,
                    Err(error) => {
                        let _ = UnregisterClassW(class, instance);
                        return Err(native_error(error));
                    }
                };
                Ok(Self {
                    backdrop,
                    previous: GetForegroundWindow(),
                    instance,
                })
            }
        }
        unsafe fn capture(
            &self,
            window: HWND,
            mode: isize,
            dc: HDC,
            width: i32,
            height: i32,
        ) -> Result<(), String> {
            use windows::Win32::Graphics::Dwm::DwmFlush;
            unsafe {
                let mut origin = POINT::default();
                MapWindowPoints(window, None, std::slice::from_mut(&mut origin));
                SetWindowPos(
                    self.backdrop,
                    window,
                    origin.x,
                    origin.y,
                    width,
                    height,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                )
                .map_err(native_error)?;
                SetWindowLongPtrW(self.backdrop, GWLP_USERDATA, mode);
                let _ = ShowWindow(window, SW_SHOWNORMAL);
                let _ = SetForegroundWindow(window);
                if GetForegroundWindow() != window {
                    return Err("Desktop proof could not activate its own window".into());
                }
                for _ in 0..6 {
                    let _ = RedrawWindow(self.backdrop, None, None, RDW_INVALIDATE | RDW_UPDATENOW);
                    let _ = RedrawWindow(
                        window,
                        None,
                        None,
                        RDW_INVALIDATE | RDW_UPDATENOW | RDW_ALLCHILDREN,
                    );
                    DwmFlush().map_err(native_error)?;
                }
                // Exercise native direct repaint after the buffered paint, without a forced redraw.
                let focus = GetFocus();
                let id = GetDlgCtrlID(focus) as usize;
                if id == visuals::SEARCH {
                    SendMessageW(
                        focus,
                        windows::Win32::UI::Controls::EM_SETSEL,
                        WPARAM(1),
                        LPARAM(1),
                    );
                    SendMessageW(focus, WM_CHAR, WPARAM('x' as usize), LPARAM(1));
                    SendMessageW(focus, WM_CHAR, WPARAM(8), LPARAM(1));
                    SendMessageW(
                        focus,
                        windows::Win32::UI::Controls::EM_SETSEL,
                        WPARAM(0),
                        LPARAM(1),
                    );
                    assert_eq!(window_text(focus), "z");
                } else if id == BATTERY {
                    let checked = SendMessageW(focus, BM_GETCHECK, WPARAM(0), LPARAM(0));
                    SendMessageW(focus, WM_KEYDOWN, WPARAM(VK_SPACE.0 as usize), LPARAM(1));
                    SendMessageW(focus, WM_KEYUP, WPARAM(VK_SPACE.0 as usize), LPARAM(1));
                    assert_ne!(
                        SendMessageW(focus, BM_GETCHECK, WPARAM(0), LPARAM(0)),
                        checked
                    );
                    SendMessageW(focus, WM_KEYDOWN, WPARAM(VK_SPACE.0 as usize), LPARAM(1));
                    SendMessageW(focus, WM_KEYUP, WPARAM(VK_SPACE.0 as usize), LPARAM(1));
                    assert_eq!(
                        SendMessageW(focus, BM_GETCHECK, WPARAM(0), LPARAM(0)),
                        checked
                    );
                }
                DwmFlush().map_err(native_error)?;
                if !visuals::glass_enabled(window) {
                    return Err("Desktop proof fell back to opaque rendering".into());
                }
                let screen = GetDC(None);
                if screen.0.is_null() {
                    return Err("Desktop capture DC unavailable".into());
                }
                let copied = BitBlt(
                    dc,
                    0,
                    0,
                    width,
                    height,
                    screen,
                    origin.x,
                    origin.y,
                    SRCCOPY | CAPTUREBLT,
                );
                let _ = ReleaseDC(None, screen);
                copied.map_err(native_error)?;
                if !GdiFlush().as_bool() {
                    return Err("Desktop capture GDI flush failed".into());
                }
                Ok(())
            }
        }
    }
    impl Drop for DesktopProof {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.backdrop);
                let _ = UnregisterClassW(w!("WallpaperLive.DesktopProofBackdrop"), self.instance);
                if IsWindow(self.previous).as_bool() {
                    let _ = SetForegroundWindow(self.previous);
                }
            }
        }
    }
    unsafe extern "system" fn proof_backdrop_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            if message != WM_PAINT {
                return DefWindowProcW(window, message, wparam, lparam);
            }
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(window, &mut ps);
            let mut area = RECT::default();
            let _ = GetClientRect(window, &mut area);
            let mode = GetWindowLongPtrW(window, GWLP_USERDATA);
            visuals::fill(dc, area, if mode == 1 { 0xffffff } else { 0 });
            if mode == 2 {
                visuals::fill(
                    dc,
                    RECT {
                        top: area.bottom / 2,
                        ..area
                    },
                    0xffffff,
                );
            }
            let _ = EndPaint(window, &ps);
            LRESULT(0)
        }
    }
    fn proof_bmp(pixels: &[u8], width: i32, height: i32) -> Vec<u8> {
        let mut bmp = Vec::with_capacity(54 + pixels.len());
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&(54 + pixels.len() as u32).to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&width.to_le_bytes());
        bmp.extend_from_slice(&(-height).to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&32u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 24]);
        bmp.extend_from_slice(pixels);
        bmp
    }

    #[test]
    fn native_tabs_keep_library_state_and_capture_layouts() -> Result<(), String> {
        // SAFETY: hidden test-owned window only; no app run(), tray, media decoder or desktop attachment.
        unsafe {
            let instance = GetModuleHandleW(None).map_err(native_error)?;
            let class = w!("WallpaperLive.UiProof");
            assert_ne!(
                RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: instance.into(),
                    lpszClassName: class,
                    ..Default::default()
                }),
                0
            );
            let mut app = fixture();
            app.hwnd = CreateWindowExW(
                WS_EX_CONTROLPARENT,
                class,
                w!("UI proof"),
                WS_POPUP | WS_CLIPCHILDREN,
                0,
                0,
                1080,
                740,
                None,
                None,
                instance,
                None,
            )
            .map_err(native_error)?;
            app.reasons.manual = true;
            app.config =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("scratch/ui-proof/settings.ini");
            app.create_controls()?;
            let folder = PathBuf::from(r"C:\UI-proof");
            app.library = (0..18)
                .map(|i| folder.join(format!("Wallpaper {i:02}.mp4")))
                .collect();
            app.selected = Some(app.library[10].clone());
            app.marked = vec![app.library[10].clone()];
            app.folder_filter = Some(folder);
            app.search = "Wallpaper".into();
            app.sort = crate::library::Sort::NameAsc;
            app.page = 1;
            app.refresh_library()?;
            let settings = app.settings.encode()?;
            let selected = app.selected.clone();
            let marked = app.marked.clone();
            let filter = app.folder_filter.clone();
            let visible =
                |id| GetWindowLongW(app.control(id), GWL_STYLE) as u32 & WS_VISIBLE.0 != 0;
            assert!(visible(visuals::PREVIEW) && visible(FILE));
            app.action(visuals::FIND_WALLPAPERS, BN_CLICKED)?;
            assert_eq!(app.tab, visuals::Tab::Sources);
            for id in visuals::SITE_BASE..visuals::SITE_BASE + WALLPAPER_SITES.len() {
                assert!(GetWindowLongW(app.control(id), GWL_STYLE) as u32 & WS_VISIBLE.0 != 0);
            }
            for id in [visuals::PREVIEW, FILE, visuals::SEARCH, SCREEN] {
                assert_eq!(
                    GetWindowLongW(app.control(id), GWL_STYLE) as u32 & WS_VISIBLE.0,
                    0
                );
            }
            app.action(visuals::TAB_SETTINGS, BN_CLICKED)?;
            assert_eq!(app.tab, visuals::Tab::Settings);
            assert_eq!(
                GetWindowLongW(app.control(visuals::PREVIEW), GWL_STYLE) as u32 & WS_VISIBLE.0,
                0
            );
            assert_eq!(
                GetWindowLongW(app.control(FILE), GWL_STYLE) as u32 & WS_VISIBLE.0,
                0
            );
            assert_eq!(
                window_text(app.control(visuals::SUPPORT)),
                "Buy me a coffee"
            );
            assert_eq!(window_text(app.control(visuals::SAWERIA)), "Saweria");
            assert_ne!(
                GetWindowLongW(app.control(visuals::SAWERIA), GWL_STYLE) as u32 & WS_VISIBLE.0,
                0
            );
            app.action(visuals::FIND_WALLPAPERS, BN_CLICKED)?;
            app.action(visuals::SOURCES_BACK, BN_CLICKED)?;
            assert_eq!(app.tab, visuals::Tab::Library);
            assert_eq!(app.page, 1);
            assert_eq!(app.selected, selected);
            assert_eq!(app.marked, marked);
            assert_eq!(app.folder_filter, filter);
            assert_eq!(app.search, "Wallpaper");
            assert_eq!(app.sort, crate::library::Sort::NameAsc);
            assert_eq!(app.settings.encode()?, settings);
            assert!(app.displays.is_empty() && app.playback.is_none());
            assert!(app.open_website("https://invalid.example/").is_err());
            assert_eq!(PRODUCT_URL, "https://livelydesk.vercel.app");
            assert_eq!(SUPPORT_URL, "https://yapp.ink/raflyf");
            let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("scratch/ui-proof");
            fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
            let desktop_requested = std::env::var_os("WALLPAPERLIVE_UI_PROOF_DESKTOP")
                .is_some_and(|value| value == "1");
            let desktop = if desktop_requested {
                Some(DesktopProof::new()?)
            } else {
                None
            };
            for (logical_width, logical_height) in [(920, 640), (1080, 740)] {
                let dpi = GetDpiForWindow(app.hwnd).max(96) as i32;
                let width = logical_width * dpi / 96;
                let height = logical_height * dpi / 96;
                SetWindowPos(
                    app.hwnd,
                    None,
                    0,
                    0,
                    width,
                    height,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
                .map_err(native_error)?;
                for (tab, name) in [
                    (visuals::Tab::Library, "library"),
                    (visuals::Tab::Settings, "settings"),
                    (visuals::Tab::Sources, "sources"),
                ] {
                    app.switch_tab(tab)?;
                    if tab == visuals::Tab::Library {
                        visuals::thumbnail(
                            app.selected.clone().expect("selected fixture wallpaper"),
                            proof_thumbnail()?,
                        );
                    }
                    let dc = CreateCompatibleDC(None);
                    assert!(!dc.0.is_null());
                    let mut bits = std::ptr::null_mut();
                    let bitmap = CreateDIBSection(
                        dc,
                        &BITMAPINFO {
                            bmiHeader: BITMAPINFOHEADER {
                                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                                biWidth: width,
                                biHeight: -height,
                                biPlanes: 1,
                                biBitCount: 32,
                                biCompression: BI_RGB.0,
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        DIB_RGB_COLORS,
                        &mut bits,
                        None,
                        0,
                    )
                    .map_err(native_error)?;
                    let previous = SelectObject(dc, bitmap);
                    visuals::paint_into(app.hwnd, dc);
                    let _ = GdiFlush();
                    let pixels = std::slice::from_raw_parts(
                        bits as *const u8,
                        (width * height * 4) as usize,
                    );
                    let pixel = |x: i32, y: i32| {
                        let i = ((y * width + x) * 4) as usize;
                        u32::from(pixels[i + 2]) << 16
                            | u32::from(pixels[i + 1]) << 8
                            | u32::from(pixels[i])
                    };
                    let header_height =
                        visuals::HEADER_HEIGHT * GetDpiForWindow(app.hwnd).max(96) as i32 / 96;
                    assert_eq!(pixel(8, 8), visuals::BODY, "header fallback must be Ink");
                    assert_eq!(
                        pixel(8, header_height + 8),
                        visuals::BODY,
                        "body fallback must be Ink"
                    );
                    visuals::paint_children_into(app.hwnd, dc, &app.controls, None, None);
                    let _ = GdiFlush();
                    let pixels = std::slice::from_raw_parts(
                        bits as *const u8,
                        (width * height * 4) as usize,
                    );
                    let bmp = proof_bmp(pixels, width, height);
                    fs::write(directory.join(format!("{name}-{logical_width}.bmp")), bmp)
                        .map_err(|e| e.to_string())?;
                    if desktop.is_some() {
                        if tab == visuals::Tab::Library {
                            let search = app.control(visuals::SEARCH);
                            let _ = ShowWindow(app.hwnd, SW_SHOWNORMAL);
                            SetFocus(search).map_err(native_error)?;
                            SendMessageW(search, WM_CHAR, WPARAM('z' as usize), LPARAM(1));
                            assert_eq!(window_text(search), "z");
                            SendMessageW(
                                search,
                                windows::Win32::UI::Controls::EM_SETSEL,
                                WPARAM(0),
                                LPARAM(1),
                            );
                            let mut start = 0u32;
                            let mut end = 0u32;
                            SendMessageW(
                                search,
                                windows::Win32::UI::Controls::EM_GETSEL,
                                WPARAM((&mut start as *mut u32) as usize),
                                LPARAM((&mut end as *mut u32) as isize),
                            );
                            assert_eq!((start, end), (0, 1));
                        } else if tab == visuals::Tab::Settings {
                            let checkbox = app.control(BATTERY);
                            let checked = SendMessageW(checkbox, BM_GETCHECK, WPARAM(0), LPARAM(0));
                            let _ = ShowWindow(app.hwnd, SW_SHOWNORMAL);
                            SetFocus(checkbox).map_err(native_error)?;
                            SendMessageW(
                                checkbox,
                                WM_KEYDOWN,
                                WPARAM(VK_SPACE.0 as usize),
                                LPARAM(1),
                            );
                            SendMessageW(
                                checkbox,
                                WM_KEYUP,
                                WPARAM(VK_SPACE.0 as usize),
                                LPARAM(1),
                            );
                            assert_ne!(
                                SendMessageW(checkbox, BM_GETCHECK, WPARAM(0), LPARAM(0)),
                                checked
                            );
                            SendMessageW(
                                checkbox,
                                BM_SETCHECK,
                                WPARAM(checked.0 as usize),
                                LPARAM(0),
                            );
                        }
                    }
                    // Exercise composited alpha even when CI has no interactive foreground.
                    if visuals::enable_backdrop(app.hwnd) {
                        visuals::paint_into(app.hwnd, dc);
                        assert!(GdiFlush().as_bool());
                        let glass_pixels = std::slice::from_raw_parts(
                            bits as *const u8,
                            (width * height * 4) as usize,
                        );
                        let canvas_pixel = |x: i32, y: i32| {
                            let i = ((y * width + x) * 4) as usize;
                            u32::from_le_bytes(
                                glass_pixels[i..i + 4].try_into().expect("BGRA pixel"),
                            )
                        };
                        let bg_channel = (17u32 * u32::from(visuals::PANEL_ALPHA) + 127) / 255;
                        let expected_acrylic = (u32::from(visuals::PANEL_ALPHA) << 24)
                            | (bg_channel << 16)
                            | (bg_channel << 8)
                            | bg_channel;
                        let card_b =
                            ((visuals::CARD & 255) * u32::from(visuals::CARD_ALPHA) + 127) / 255;
                        let card_g =
                            (((visuals::CARD >> 8) & 255) * u32::from(visuals::CARD_ALPHA) + 127)
                                / 255;
                        let card_r =
                            (((visuals::CARD >> 16) & 255) * u32::from(visuals::CARD_ALPHA) + 127)
                                / 255;
                        let expected_card = (u32::from(visuals::CARD_ALPHA) << 24)
                            | (card_r << 16)
                            | (card_g << 8)
                            | card_b;
                        let layout = visuals::Layout::new(logical_width, logical_height);
                        assert_eq!(
                            canvas_pixel(8, 8),
                            expected_acrylic,
                            "header must transmit acrylic Ink backdrop"
                        );
                        assert_eq!(
                            canvas_pixel(8, height / 2),
                            expected_acrylic,
                            "canvas must transmit acrylic Ink backdrop"
                        );
                        assert_eq!(
                            canvas_pixel(8, height - 40 * dpi / 96),
                            expected_acrylic,
                            "footer must transmit acrylic Ink backdrop"
                        );
                        match tab {
                            visuals::Tab::Library => assert_eq!(
                                canvas_pixel(
                                    (layout.sidebar.left + 8) * dpi / 96,
                                    (layout.sidebar.top + 8) * dpi / 96,
                                ),
                                expected_card,
                                "library sidebar must use frosted Card acrylic"
                            ),
                            visuals::Tab::Settings => {
                                assert_eq!(
                                    canvas_pixel(
                                        (layout.settings_preferences_panel.left + 6) * dpi / 96,
                                        (layout.settings_preferences_panel.top + 6) * dpi / 96,
                                    ),
                                    expected_card,
                                    "settings card must use frosted Card acrylic"
                                );
                                assert_eq!(
                                    canvas_pixel(
                                        layout.settings_preferences_panel.left * dpi / 96,
                                        layout.settings_preferences_panel.top * dpi / 96,
                                    ),
                                    expected_acrylic,
                                    "settings rounded card corners must match acrylic canvas"
                                );
                            }
                            visuals::Tab::Sources => {
                                assert_eq!(
                                    canvas_pixel(
                                        (layout.sources_panel.left + 6) * dpi / 96,
                                        (layout.sources_panel.top + 6) * dpi / 96,
                                    ),
                                    expected_card,
                                    "sources card must use frosted Card acrylic"
                                );
                                assert_eq!(
                                    canvas_pixel(
                                        layout.sources_panel.left * dpi / 96,
                                        layout.sources_panel.top * dpi / 96,
                                    ),
                                    expected_acrylic,
                                    "sources rounded card corners must match acrylic canvas"
                                );
                            }
                        }
                        // All non-premultiplied channel invariants still apply below regardless of PANEL_ALPHA.
                        assert!(
                            glass_pixels
                                .as_chunks::<4>()
                                .0
                                .iter()
                                .all(|pixel| pixel[..3].iter().all(|channel| *channel <= pixel[3])),
                            "glass pixels must be premultiplied"
                        );
                        for (_, child) in &app.controls {
                            if GetWindowLongW(*child, GWL_STYLE) as u32 & WS_VISIBLE.0 == 0 {
                                continue;
                            }
                            let mut area = RECT::default();
                            GetClientRect(*child, &mut area).map_err(native_error)?;
                            SendMessageW(
                                *child,
                                WM_PRINTCLIENT,
                                WPARAM(dc.0 as usize),
                                LPARAM(PRF_CLIENT as isize),
                            );
                            assert!(GdiFlush().as_bool());
                            let control_pixels = std::slice::from_raw_parts(
                                bits as *const u8,
                                (width * height * 4) as usize,
                            );
                            let mut max_alpha = 0u8;
                            for y in 0..area.bottom {
                                for x in 0..area.right {
                                    let i = ((y * width + x) * 4) as usize;
                                    let pixel = &control_pixels[i..i + 4];
                                    max_alpha = max_alpha.max(pixel[3]);
                                    assert!(
                                        pixel[..3].iter().all(|channel| *channel <= pixel[3]),
                                        "control {} must preserve premultiplied alpha: {pixel:?} at ({x}, {y})",
                                        GetDlgCtrlID(*child)
                                    );
                                }
                            }
                            let ctrl_id = GetDlgCtrlID(*child);
                            if ctrl_id != STATUS as i32 && ctrl_id != visuals::PREVIEW as i32 {
                                assert!(
                                    max_alpha >= 42,
                                    "control {ctrl_id} must render visible alpha (got max {max_alpha})"
                                );
                            }
                        }
                    }
                    if let Some(desktop) = &desktop {
                        for (mode, label) in [(0, "black"), (1, "white"), (2, "edge")] {
                            desktop.capture(app.hwnd, mode, dc, width, height)?;
                            let pixels = std::slice::from_raw_parts(
                                bits as *const u8,
                                (width * height * 4) as usize,
                            );
                            let status_i = ((((logical_height - 40) * dpi / 96) * width
                                + 32 * dpi / 96)
                                * 4) as usize;
                            let footer_i = ((((logical_height - 40) * dpi / 96) * width
                                + 12 * dpi / 96)
                                * 4) as usize;
                            for c in 0..3 {
                                assert!(
                                    pixels[status_i + c].abs_diff(pixels[footer_i + c]) <= 1,
                                    "empty status must match acrylic Ink footer"
                                );
                            }
                            if tab == visuals::Tab::Library {
                                let x = 150 * dpi / 96;
                                let y = 172 * dpi / 96;
                                let i = ((y * width + x) * 4) as usize;
                                let (b, g, r) = (pixels[i], pixels[i + 1], pixels[i + 2]);
                                assert!(
                                    r <= 0x48 && g <= 0x48 && b <= 0x48,
                                    "native typing must not wash out the search background: ({r}, {g}, {b})"
                                );
                            }
                            fs::write(
                                directory
                                    .join(format!("{name}-{logical_width}-desktop-{label}.bmp")),
                                proof_bmp(pixels, width, height),
                            )
                            .map_err(|e| e.to_string())?;
                        }
                        if tab == visuals::Tab::Library {
                            app.text(visuals::SEARCH, "");
                        }
                        let _ = ShowWindow(app.hwnd, SW_HIDE);
                    }
                    if visuals::glass_enabled(app.hwnd) {
                        let _ = ShowWindow(app.hwnd, SW_SHOWNA);
                        let _ = ShowWindow(app.hwnd, SW_MINIMIZE);
                        visuals::paint_into(app.hwnd, dc);
                        assert!(
                            visuals::glass_enabled(app.hwnd),
                            "minimizing must preserve acrylic backdrop"
                        );
                        let _ = ShowWindow(app.hwnd, SW_RESTORE);
                        assert!(
                            visuals::glass_enabled(app.hwnd),
                            "restoring from minimize must preserve acrylic backdrop"
                        );
                        let _ = ShowWindow(app.hwnd, SW_HIDE);
                    }
                    let none = windows::Win32::Graphics::Dwm::DWMSBT_NONE.0;
                    let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                        app.hwnd,
                        windows::Win32::Graphics::Dwm::DWMWA_SYSTEMBACKDROP_TYPE,
                        (&none as *const i32).cast(),
                        std::mem::size_of_val(&none) as u32,
                    );
                    let _ = windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea(
                        app.hwnd,
                        &windows::Win32::UI::Controls::MARGINS::default(),
                    );
                    let _ = RemovePropW(app.hwnd, w!("WallpaperLive.Glass"));
                    SelectObject(dc, previous);
                    let _ = DeleteObject(bitmap);
                    let _ = DeleteDC(dc);
                }
            }
            drop(app);
            UnregisterClassW(class, instance).map_err(native_error)?;
        }
        Ok(())
    }

    #[test]
    fn display_target_and_state_survive_switching_and_errors() {
        let mut app = fixture();
        for (name, left, width) in [("first", -1920, 1920), ("second", 0, 2712)] {
            app.monitors.push(Monitor {
                name: name.into(),
                rect: Rect {
                    left,
                    top: 0,
                    right: left + width,
                    bottom: 1080,
                },
                primary: left < 0,
            });
            app.displays.push(DisplayState::new(name.into()));
        }
        app.settings.monitor = "*".into();
        assert_eq!(app.target_monitors(), ["first", "second"]);
        app.settings.monitor = "second".into();
        assert_eq!(app.target_monitors(), ["second"]);
        app.load_request = 99;
        app.displays[0].load_request = 10;
        app.displays[1].load_request = 20;
        app.displays[1].loaded = true;
        app.reasons.manual = true;
        let source = PathBuf::from(r"C:\media\pending.mp4");
        app.displays[0]
            .preparation_tx
            .send((7, Err("first-only".into())))
            .unwrap();
        let result: Result<(), String> = app.with_display(0, |app| {
            assert_eq!(app.screen().name, "first");
            assert_eq!(app.load_request, 10);
            assert_eq!(app.preparation_rx.try_recv().unwrap().0, 7);
            assert!(app.reasons.manual);
            app.load_request = 11;
            app.recovery = Some((Instant::now(), 1, source.clone()));
            Err("expected failure".into())
        });
        assert!(result.is_err());
        assert_eq!(app.displays[0].load_request, 11);
        assert_eq!(app.displays[0].recovery.as_ref().unwrap().2, source);
        assert_eq!(app.displays[1].load_request, 20);
        assert!(app.displays[1].loaded && app.displays[1].preparation_rx.try_recv().is_err());
        assert_eq!(app.load_request, 99);
        assert!(app.recovery.is_none() && app.active_monitor.is_none() && !app.multi_dispatch);
        assert_eq!(app.settings.monitor, "second");
        assert!(app.reasons.manual);
    }

    #[test]
    fn removing_queued_wallpaper_keeps_other_display() {
        let mut app = fixture();
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scratch")
            .join(format!(
                "queued-remove-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&directory).unwrap();
        app.config = directory.join("settings.ini");
        let removed = PathBuf::from(r"C:\media\removed.mp4");
        let keep = PathBuf::from(r"C:\media\keep.mp4");
        app.library = vec![removed.clone(), keep.clone()];
        app.marked = vec![removed.clone()];
        app.settings.wallpapers = vec![
            ("first".into(), removed.clone()),
            ("second".into(), keep.clone()),
        ];
        let mut first = DisplayState::new("first".into());
        first.prepared_waiting = Some((
            Instant::now(),
            removed,
            crate::cache::Prepared {
                path: directory.join("cache.mp4"),
                width: 1920,
                height: 1080,
                fps: 30.0,
                cached: true,
                note: "test".into(),
            },
        ));
        let mut second = DisplayState::new("second".into());
        second.load_request = 20;
        second.recovery = Some((Instant::now(), 0, keep.clone()));
        app.displays = vec![first, second];
        app.remove_selected().unwrap();
        assert!(app.displays[0].prepared_waiting.is_none() && app.displays[0].dormant.is_none());
        assert_eq!(app.displays[1].load_request, 20);
        assert_eq!(app.displays[1].recovery.as_ref().unwrap().2, keep);
        assert_eq!(app.settings.wallpapers.len(), 1);
        assert_eq!(app.library.len(), 1);
    }

    #[test]
    fn active_cache_protects_current_and_waiting_files() {
        let mut app = fixture();
        let current = PathBuf::from(r"C:\test\cache-playing.mp4");
        let waiting = PathBuf::from(r"C:\test\cache-waiting.mp4");
        app.playback_info = Some(crate::cache::Prepared {
            path: current.clone(),
            width: 1920,
            height: 1080,
            fps: 60.0,
            cached: true,
            note: "test".into(),
        });
        app.prepared_waiting = Some((
            Instant::now(),
            PathBuf::from(r"C:\test\source.mp4"),
            crate::cache::Prepared {
                path: waiting.clone(),
                width: 1920,
                height: 1080,
                fps: 60.0,
                cached: true,
                note: "test".into(),
            },
        ));
        let other = PathBuf::from(r"C:\test\cache-second-display.mp4");
        let mut second = DisplayState::new("second".into());
        second.playback_info = Some(crate::cache::Prepared {
            path: other.clone(),
            width: 2712,
            height: 1220,
            fps: 60.0,
            cached: true,
            note: "test".into(),
        });
        app.displays.push(second);
        let active = app.active_cache();
        assert_eq!(active.len(), 3);
        assert!(active.contains(&other));
        assert!(active.contains(&current));
        assert!(active.contains(&waiting));
    }

    #[test]
    fn manual_update_clicks_share_one_running_check() {
        let mut app = fixture();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        app.update_thread = Some((
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::thread::spawn(move || {
                let _ = done_rx.recv();
            }),
        ));
        let id = app.update_thread.as_ref().unwrap().1.thread().id();
        app.check_updates(true).unwrap();
        app.check_updates(true).unwrap();
        assert!(app.update_manual);
        assert_eq!(app.update_thread.as_ref().unwrap().1.thread().id(), id);
        app.update_ready().unwrap();
        assert!(app.update_thread.is_some());
        app.update_manual = false;
        app.update_tx.send(Ok(None)).unwrap();
        done_tx.send(()).unwrap();
        let (_, worker) = app.update_thread.take().unwrap();
        worker.join().unwrap();
        app.update_ready().unwrap();
        assert!(app.update_tag.is_none());
    }
    #[test]
    fn obsolete_decoder_entries_cannot_fail_a_new_load() {
        let mut app = fixture();
        app.load_request = 18;
        app.event(Event::LoadAccepted {
            request: 17,
            entry: 2,
        })
        .unwrap();
        app.event(Event::Started(2)).unwrap();
        app.event(Event::Loaded(Some(2))).unwrap();
        app.event(Event::Ready(Some(2))).unwrap();
        app.event(Event::FileFailed {
            entry: 2,
            error: "obsolete source".into(),
        })
        .unwrap();
        assert!(app.load_entry.is_none() && app.error.is_none());
        app.event(Event::LoadAccepted {
            request: 18,
            entry: 3,
        })
        .unwrap();
        app.event(Event::Started(3)).unwrap();
        app.event(Event::Loaded(Some(3))).unwrap();
        app.event(Event::Ready(Some(3))).unwrap();
        app.event(Event::FileFailed {
            entry: 2,
            error: "late old error".into(),
        })
        .unwrap();
        assert_eq!(app.load_entry, Some(3));
        assert_eq!(app.file_loaded, Some(3));
        assert_eq!(app.video_ready, Some(3));
        assert_eq!(app.load_request, 18);
    }
    #[test]
    fn cancellation_is_nonblocking_and_stale_cache_is_ignored() {
        let mut app = fixture();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cancelled.clone();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        app.preparation_thread = Some((
            cancelled,
            std::thread::spawn(move || {
                let _ = release_rx.recv();
                assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
            }),
        ));
        app.preparation = Some((
            PathBuf::from(r"C:\test\original.mp4"),
            (1920, 1080),
            crate::cache::CACHE_FPS,
        ));
        let stale = app.preparation_id;
        let started = Instant::now();
        app.cancel_preparation();
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(app.preparation_thread.is_some());
        app.preparation_tx
            .send((stale, Err("obsolete worker".into())))
            .unwrap();
        app.prepared().unwrap();
        assert!(app.error.is_none() && app.preparation.is_none());
        release_tx.send(()).unwrap();
        if let Some((_, worker)) = app.preparation_thread.take() {
            worker.join().unwrap();
        }
        app.monitors.push(Monitor {
            name: "owned-test".into(),
            rect: Rect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            primary: true,
        });
        app.reasons.manual = true;
        let original = PathBuf::from(r"C:\test\original.mp4");
        app.preparation = Some((original.clone(), (1920, 1080), crate::cache::CACHE_FPS));
        app.preparation_tx
            .send((
                app.preparation_id,
                Ok(crate::cache::Prepared {
                    path: PathBuf::from(r"C:\test\cache.mp4"),
                    width: 1920,
                    height: 1080,
                    fps: 60.0,
                    cached: true,
                    note: "Screen-sized cache".into(),
                }),
            ))
            .unwrap();
        app.prepared().unwrap();
        assert!(app.preparation.is_none());
        assert_eq!(app.prepared_waiting.as_ref().unwrap().1, original);
        app.tick().unwrap();
        assert!(app.prepared_waiting.is_none() && app.preparation_thread.is_none());
        assert_eq!(app.preparation.as_ref().unwrap().0, original);
        drop(app);
    }
    #[test]
    #[ignore = "Requires pinned decoder and owned fixtures; isolated HWND only, no app/tray/desktop interaction"]
    fn native_switch_fps_and_retained_pause() -> Result<(), String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let source = root.join("scratch/media/sample.mp4");
        let source_bytes = fs::read(&source).map_err(|e| e.to_string())?;
        let directory = root.join(format!(
            "scratch/switch-proof-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let mut app = fixture();
        app.multi_dispatch = true;
        app.config = directory.join("settings.ini");
        app.settings.pause_battery = false;
        app.settings.pause_fullscreen = false;
        app.monitors.push(Monitor {
            name: "owned-test".into(),
            rect: Rect {
                left: 0,
                top: 0,
                right: 640,
                bottom: 360,
            },
            primary: true,
        });
        // SAFETY: owned test HWND; never enters run(), mutex, startup, tray or Explorer discovery.
        app.hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("WallpaperLive isolated switch proof"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                680,
                420,
                None,
                None,
                GetModuleHandleW(None).map_err(|e| e.to_string())?,
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        let surface = Surface::isolated(app.hwnd, app.screen().rect)?;
        let surface_hwnd = surface.hwnd;
        let player = Mpv::open_at(
            &root.join("runtime/libmpv-2.dll"),
            surface_hwnd,
            app.hwnd,
            Fit::Fill,
        )?;
        app.playback = Some(Playback {
            player,
            surface,
            path: source.clone(),
            audio_enabled: false,
        });
        let original = || crate::cache::Prepared {
            path: source.clone(),
            width: 640,
            height: 360,
            fps: 30.0,
            cached: false,
            note: "Original media".into(),
        };
        app.load_prepared(source.clone(), original(), 0.0)?;
        let pump = |app: &mut App, duration: Duration| -> Result<(), String> {
            let end = Instant::now() + duration;
            while Instant::now() < end {
                let mut msg = MSG::default();
                unsafe {
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                app.events()?;
                std::thread::park_timeout(Duration::from_millis(5));
            }
            Ok(())
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.loaded {
            pump(&mut app, Duration::from_millis(20))?;
            assert!(Instant::now() < deadline, "First owned video did not load");
        }
        let position = |app: &App| {
            app.playback
                .as_ref()
                .unwrap()
                .player
                .property(c"time-pos")
                .unwrap()
                .parse::<f64>()
                .unwrap()
        };
        for fps in crate::core::FPS_OPTIONS {
            let started = Instant::now();
            app.change_fps(fps)?;
            assert!(started.elapsed() < Duration::from_millis(500));
            assert!(app.preparation.is_none() && app.preparation_thread.is_none());
            assert_eq!(app.playback.as_ref().unwrap().surface.hwnd, surface_hwnd);
            pump(&mut app, Duration::from_millis(50))?;
            assert!(app.error.is_none(), "{:?}", app.error);
        }
        app.reasons.manual = true;
        app.tick()?;
        pump(&mut app, Duration::from_millis(150))?;
        let paused_position = position(&app);
        pump(&mut app, Duration::from_millis(300))?;
        assert!((position(&app) - paused_position).abs() < 0.01);
        assert!(app.actual_pause && app.playback.is_some());
        assert!(unsafe { IsWindow(surface_hwnd) }.as_bool());
        app.reasons.manual = false;
        let resumed = Instant::now();
        app.tick()?;
        pump(&mut app, Duration::from_millis(180))?;
        assert!(!app.actual_pause && position(&app) > paused_position);
        assert!(resumed.elapsed() < Duration::from_millis(500));
        assert!(app.preparation.is_none());
        app.reasons.manual = true;
        app.tick()?;
        pump(&mut app, Duration::from_millis(100))?;
        let old_request = app.load_request;
        app.prepared_waiting = Some((Instant::now(), source.clone(), original()));
        app.reasons.manual = false;
        app.tick()?;
        assert_eq!(app.load_request, old_request);
        assert!(app.prepared_waiting.as_ref().unwrap().0 > Instant::now());
        pump(&mut app, Duration::from_millis(150))?;
        assert!(!app.actual_pause);
        app.prepared_waiting = None;
        app.load_prepared(source.clone(), original(), 0.0)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.loaded {
            pump(&mut app, Duration::from_millis(20))?;
            assert!(Instant::now() < deadline);
        }
        assert_eq!(app.playback.as_ref().unwrap().surface.hwnd, surface_hwnd);
        let bad = directory.join("bad.mp4");
        fs::write(&bad, b"owned malformed media fixture").map_err(|e| e.to_string())?;
        for _ in 0..3 {
            let mut broken = original();
            broken.path = bad.clone();
            app.load_prepared(bad.clone(), broken, 0.0)?;
            app.load_prepared(source.clone(), original(), 0.0)?;
            let deadline = Instant::now() + Duration::from_secs(5);
            while !app.loaded {
                pump(&mut app, Duration::from_millis(20))?;
                assert!(Instant::now() < deadline, "Rapid bad A/good B did not load");
            }
            assert!(app.error.is_none(), "{:?}", app.error);
        }
        let gif = root.join("scratch/media/sample.gif");
        let video_4k = root.join("scratch/media/cache-proof-4k60.mp4");
        let gif_bytes = fs::read(&gif).map_err(|e| e.to_string())?;
        let video_4k_bytes = fs::read(&video_4k).map_err(|e| e.to_string())?;
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cancelled.clone();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel::<()>();
        app.preparation_thread = Some((
            cancelled,
            std::thread::spawn(move || {
                let _ = finish_rx.recv();
                assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
            }),
        ));
        app.preparation = Some((source.clone(), (640, 360), crate::cache::CACHE_FPS));
        let request_id = app.preparation_id;
        let started = Instant::now();
        app.change_fps(24)?;
        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(app.preparation_id, request_id);
        app.reasons.manual = true;
        let started = Instant::now();
        app.tick()?;
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(app.playback.is_some() && app.preparation_thread.is_some());
        app.cancel_preparation();
        finish_tx.send(()).unwrap();
        if let Some((_, worker)) = app.preparation_thread.take() {
            worker.join().unwrap();
        }
        app.reasons.manual = false;
        for (media, width, height, fps) in [
            (&gif, 320, 180, 10.0),
            (&video_4k, 3840, 2160, 60.0),
            (&source, 640, 360, 30.0),
        ] {
            let started = Instant::now();
            app.load_prepared(
                media.clone(),
                crate::cache::Prepared {
                    path: media.clone(),
                    width,
                    height,
                    fps,
                    cached: false,
                    note: "Owned original".into(),
                },
                0.0,
            )?;
            assert!(started.elapsed() < Duration::from_millis(500));
            let deadline = Instant::now() + Duration::from_secs(5);
            while !app.loaded {
                pump(&mut app, Duration::from_millis(20))?;
                assert!(Instant::now() < deadline, "{:?}", media);
            }
            assert_eq!(app.playback.as_ref().unwrap().surface.hwnd, surface_hwnd);
            assert!(app.error.is_none(), "{:?}", app.error);
        }
        let bad_bytes = fs::read(&bad).map_err(|e| e.to_string())?;
        let final_cache = directory.join("finalized-cache.mp4");
        fs::write(&final_cache, &source_bytes).map_err(|e| e.to_string())?;
        let mut broken = original();
        broken.path = bad.clone();
        app.load_prepared(bad.clone(), broken, 0.0)?;
        app.preparation = Some((bad.clone(), (640, 360), crate::cache::CACHE_FPS));
        let deadline = Instant::now() + Duration::from_secs(5);
        let error = loop {
            match pump(&mut app, Duration::from_millis(20)) {
                Ok(()) => assert!(
                    Instant::now() < deadline,
                    "Malformed original was not rejected"
                ),
                Err(error) => break error,
            }
        };
        app.set_error(error);
        assert!(app.playback.is_none());
        app.reasons.manual = true;
        let mut cached = original();
        cached.path = final_cache.clone();
        cached.cached = true;
        app.preparation_tx
            .send((app.preparation_id, Ok(cached)))
            .unwrap();
        app.tick()?;
        assert!(app.playback.is_none() && app.prepared_waiting.is_some());
        app.reasons.manual = false;
        app.tick()?;
        assert!(app.playback.is_some() && app.prepared_waiting.is_none());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.loaded {
            pump(&mut app, Duration::from_millis(20))?;
            assert!(
                Instant::now() < deadline,
                "Finalized cache was stranded after original failure"
            );
        }
        assert_eq!(
            app.playback
                .as_ref()
                .unwrap()
                .player
                .property(c"path")
                .map(PathBuf::from),
            Some(final_cache.clone())
        );
        assert_eq!(fs::read(&bad).map_err(|e| e.to_string())?, bad_bytes);
        assert_eq!(fs::read(&gif).map_err(|e| e.to_string())?, gif_bytes);
        assert_eq!(
            fs::read(&video_4k).map_err(|e| e.to_string())?,
            video_4k_bytes
        );
        assert_eq!(fs::read(&source).map_err(|e| e.to_string())?, source_bytes);
        println!(
            "Owned renderer reused across GIF/4K/video; FPS, pause/cancel and resume below500ms without blocking worker; original files unchanged"
        );
        Ok(())
    }
    #[test]
    fn invalid_media_preserves_preview_and_last_good_recovery() {
        let active = PathBuf::from(r"C:\wallpaperlive-missing-regression-87b81342\active.mp4");
        let preview = PathBuf::from(r"C:\wallpaperlive-missing-regression-87b81342\preview.mp4");
        let settings = Settings {
            path: Some(active.clone()),
            ..Settings::default()
        };
        let (thumbnail_tx, thumbnail_rx) = std::sync::mpsc::channel();
        let (preparation_tx, preparation_rx) = std::sync::mpsc::channel();
        // No windows or decoder are created: every exercised path fails validation first.
        let (update_tx, update_rx) = std::sync::mpsc::channel();
        let mut app = App {
            hwnd: HWND::default(),
            controls: vec![],
            font: HFONT::default(),
            icon: Icon(HICON::default()),
            small_icon: Icon(HICON::default()),
            monitors: vec![],
            settings: settings.clone(),
            selected: Some(preview.clone()),
            config: PathBuf::new(),
            config_valid: true,
            playback: None,
            displays: Vec::new(),
            multi_dispatch: false,
            active_monitor: None,
            audio_monitor: None,
            cache_busy: false,
            reasons: PauseReasons::default(),
            actual_pause: false,
            requested_pause: false,
            loaded: false,
            load_request: 0,
            load_entry: None,
            file_loaded: None,
            video_ready: None,
            loading_since: None,
            recovery: None,
            power: vec![],
            tray_present: false,
            update_tag: None,
            update_manual: false,
            update_tx: update_tx.clone(),
            update_rx,
            update_thread: None,
            error: None,
            library: vec![],
            folder_filter: None,
            search: String::new(),
            sort: crate::library::Sort::default(),
            folder_roots: vec![],
            marked: vec![],
            preparation: None,
            preparation_thread: None,
            preparation_id: 0,
            preparation_rx,
            preparation_tx,
            prepared_waiting: None,
            playback_info: None,
            dormant: None,
            resume_position: 0.0,
            paused_since: None,
            pause_trimmed: false,
            wts_tick: 0,
            status_cache: std::cell::RefCell::new((String::new(), "", "", None)),
            page: 0,
            tab: visuals::Tab::Library,
            thumbnail_pending: None,
            thumbnail_thread: None,
            thumbnail_failed: std::collections::HashSet::new(),
            thumbnail_rx,
            thumbnail_tx,
            last_tile_click: None,
        };
        update_tx.send(Ok(Some("v0.1.2".into()))).unwrap();
        app.update_ready().unwrap();
        assert!(!app.update_manual);
        assert_eq!(app.update_tag.as_deref(), Some("v0.1.2"));
        assert!(app.update_thread.is_none());
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
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scratch")
            .join(format!(
                "library-regression-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&directory).unwrap();
        let original = directory.join("original.mp4");
        fs::write(&original, b"owned regression fixture, not decoder media").unwrap();
        app.config = directory.join("settings.ini");
        app.library = vec![original.clone(), preview.clone()];
        app.selected = Some(original.clone());
        app.marked = vec![original.clone()];
        app.page = 2;
        app.settings.path = None;
        app.save_library(&app.library).unwrap();
        app.remove_selected().unwrap();
        assert_eq!(
            fs::read(&original).unwrap(),
            b"owned regression fixture, not decoder media"
        );
        assert_eq!(app.library.as_slice(), std::slice::from_ref(&preview));
        assert_eq!(app.selected.as_ref(), Some(&preview));
        assert_eq!(app.page, 0);
        let saved = fs::read(directory.join("recent.txt")).unwrap();
        assert!(!String::from_utf8_lossy(&saved).contains("original.mp4"));
        app.config_valid = false;
        app.selected = Some(preview.clone());
        app.marked = vec![preview.clone()];
        assert!(app.remove_selected().is_err());
        assert_eq!(fs::read(directory.join("recent.txt")).unwrap(), saved);
        assert_eq!(app.library.as_slice(), std::slice::from_ref(&preview));
        assert!(!app.settings.audio_enabled);
        assert!(!app.action(visuals::AUDIO, BN_SETFOCUS).unwrap());
        assert!(!app.settings.audio_enabled);
        app.config_valid = true;
        app.library = vec![
            original.clone(),
            preview.clone(),
            directory.join("third.mp4"),
        ];
        app.folder_filter = Some(directory.clone());
        app.marked = app.library_view();
        assert_eq!(app.marked.len(), 2);
        app.remove_selected().unwrap();
        assert_eq!(app.library.as_slice(), std::slice::from_ref(&preview));
        assert_eq!(
            fs::read(&original).unwrap(),
            b"owned regression fixture, not decoder media"
        );
        app.reasons.manual = true;
        app.preparation = Some((preview.clone(), (1920, 1080), 30));
        app.resume_position = 1.25;
        app.tick().unwrap();
        assert!(app.preparation.is_some() && app.playback.is_none() && app.playback_info.is_none());
        assert!(app.dormant.is_none());
        assert_eq!(app.resume_position, 1.25);
        let preparation_id = app.preparation_id;
        app.change_fps(45).unwrap();
        assert_eq!(app.settings.fps, 45);
        assert_eq!(app.preparation_id, preparation_id);
        assert_eq!(app.preparation.as_ref().unwrap().0, preview);
        assert_eq!(app.resume_position, 1.25);
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        app.thumbnail_thread = Some((
            cancelled,
            std::thread::spawn(move || {
                while !worker_cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::park_timeout(Duration::from_millis(10));
                }
                finished_tx.send(()).unwrap();
            }),
        ));
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let (prepare_finished_tx, prepare_finished_rx) = std::sync::mpsc::channel();
        app.preparation_thread = Some((
            cancelled,
            std::thread::spawn(move || {
                while !worker_cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::park_timeout(Duration::from_millis(10));
                }
                prepare_finished_tx.send(()).unwrap();
            }),
        ));
        drop(app);
        assert_eq!(prepare_finished_rx.try_recv(), Ok(()));
        assert_eq!(finished_rx.try_recv(), Ok(()));
    }

    #[test]
    fn dormant_grace_and_shell_class_rules() {
        let mut reasons = PauseReasons::default();
        assert_eq!(dormant_grace(&reasons), None);

        reasons.battery = true;
        assert_eq!(dormant_grace(&reasons), None);

        reasons.fullscreen = true;
        assert_eq!(dormant_grace(&reasons), None);

        reasons.maximized = true;
        assert_eq!(dormant_grace(&reasons), None);

        reasons.locked = true;
        assert_eq!(dormant_grace(&reasons), Some(Duration::from_secs(3)));

        reasons.manual = true;
        assert_eq!(dormant_grace(&reasons), None);

        let progman: Vec<u16> = "Progman".encode_utf16().collect();
        let tray: Vec<u16> = "Shell_SecondaryTrayWnd".encode_utf16().collect();
        let app_win: Vec<u16> = "Notepad".encode_utf16().collect();
        assert!(is_shell_class(&progman));
        assert!(is_shell_class(&tray));
        assert!(!is_shell_class(&app_win));
    }
}
