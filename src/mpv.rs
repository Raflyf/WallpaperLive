use crate::core::Fit;
#[path = "gpu.rs"]
mod gpu;
use std::{
    cell::Cell,
    ffi::{CStr, CString, c_char, c_int, c_ulong, c_void},
    fs::OpenOptions,
    path::{Path, PathBuf},
    ptr,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{FreeLibrary, HMODULE, HWND, LPARAM, WPARAM},
        System::LibraryLoader::{
            GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
            LoadLibraryExW,
        },
        UI::WindowsAndMessaging::PostMessageW,
    },
    core::{PCSTR, PCWSTR},
};

pub const WM_MPV: u32 = 0x8000 + 42;

#[repr(C)]
struct RawEvent {
    id: c_int,
    error: c_int,
    request: u64,
    data: *mut c_void,
}
#[repr(C)]
struct EndFile {
    reason: c_int,
    error: c_int,
    entry: i64,
    insert: i64,
    count: c_int,
}
#[repr(C)]
struct Property {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct LogMessage {
    prefix: *const c_char,
    level: *const c_char,
    text: *const c_char,
    log_level: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
union NodeValue {
    integer: i64,
    list: *const NodeList,
}
#[repr(C)]
struct Node {
    value: NodeValue,
    format: c_int,
}
#[repr(C)]
struct NodeList {
    count: c_int,
    values: *const Node,
    keys: *const *const c_char,
}
unsafe fn load_entry(data: *const c_void) -> Option<i64> {
    if data.is_null() {
        return None;
    }
    // SAFETY: COMMAND_REPLY borrows mpv_event_command.result until the next wait_event.
    unsafe {
        let node = &*data.cast::<Node>();
        if node.format != 8 || node.value.list.is_null() {
            return None;
        }
        let list = &*node.value.list;
        if !(1..=16).contains(&list.count) || list.values.is_null() || list.keys.is_null() {
            return None;
        }
        for i in 0..list.count as usize {
            let key = *list.keys.add(i);
            if !key.is_null() && CStr::from_ptr(key) == c"playlist_entry_id" {
                let value = &*list.values.add(i);
                if value.format != 4 {
                    return None;
                }
                return (value.value.integer >= 0).then_some(value.value.integer);
            }
        }
        None
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    LoadAccepted { request: u64, entry: i64 },
    Started(i64),
    Loaded(Option<i64>),
    Ready(Option<i64>),
    FileFailed { entry: i64, error: String },
    Failed(String),
    FrameRateFailed(String),
    Pause(bool),
}
impl Event {
    fn video(id: c_int, entry: Option<i64>) -> Option<Self> {
        match id {
            8 => Some(Self::Loaded(entry)),
            17 => Some(Self::Ready(entry)),
            _ => None,
        }
    }
}

struct Library(HMODULE);
impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: this owns one LoadLibrary reference, retained until all mpv handles are gone.
        unsafe {
            let _ = FreeLibrary(self.0);
        }
    }
}

struct Api {
    create: unsafe extern "C" fn() -> *mut c_void,
    initialize: unsafe extern "C" fn(*mut c_void) -> c_int,
    destroy: unsafe extern "C" fn(*mut c_void),
    option: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    command: unsafe extern "C" fn(*mut c_void, u64, *const *const c_char) -> c_int,
    set: unsafe extern "C" fn(*mut c_void, u64, *const c_char, c_int, *mut c_void) -> c_int,
    get: unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_void) -> c_int,
    observe: unsafe extern "C" fn(*mut c_void, u64, *const c_char, c_int) -> c_int,
    wait: unsafe extern "C" fn(*mut c_void, f64) -> *const RawEvent,
    wakeup:
        unsafe extern "C" fn(*mut c_void, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void),
    error: unsafe extern "C" fn(c_int) -> *const c_char,
    free: unsafe extern "C" fn(*mut c_void),
    logs: unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int,
    _library: Library,
}

impl Api {
    fn load(path: &Path) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err("Decoder path must be absolute".into());
        }
        let text = path.to_str().ok_or("Decoder path is not valid Unicode")?;
        let wide: Vec<_> = text.encode_utf16().chain(Some(0)).collect();
        // SAFETY: absolute UTF-16 path; dependencies searched only beside DLL and in System32.
        let library = Library(
            unsafe {
                LoadLibraryExW(
                    PCWSTR(wide.as_ptr()),
                    None,
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
                )
            }
            .map_err(|e| format!("Decoder unavailable. Run scripts/Get-Decoder.ps1. ({e})"))?,
        );
        macro_rules! symbol {
            ($name:literal, $ty:ty) => {{
                // SAFETY: symbol types match include/mpv/client.h API 2; library outlives pointers.
                let address =
                    unsafe { GetProcAddress(library.0, PCSTR(concat!($name, "\0").as_ptr())) }
                        .ok_or(concat!("Missing decoder symbol: ", $name))?;
                unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(address) }
            }};
        }
        let version = symbol!("mpv_client_api_version", unsafe extern "C" fn() -> c_ulong);
        // SAFETY: loaded function has checked upstream ABI, no handle needed.
        if unsafe { version() } >> 16 != 2 {
            return Err("Incompatible decoder API version (libmpv API 2 required)".into());
        }
        Ok(Self {
            create: symbol!("mpv_create", unsafe extern "C" fn() -> *mut c_void),
            initialize: symbol!("mpv_initialize", unsafe extern "C" fn(*mut c_void) -> c_int),
            destroy: symbol!("mpv_terminate_destroy", unsafe extern "C" fn(*mut c_void)),
            option: symbol!(
                "mpv_set_option_string",
                unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int
            ),
            command: symbol!(
                "mpv_command_async",
                unsafe extern "C" fn(*mut c_void, u64, *const *const c_char) -> c_int
            ),
            set: symbol!(
                "mpv_set_property_async",
                unsafe extern "C" fn(*mut c_void, u64, *const c_char, c_int, *mut c_void) -> c_int
            ),
            get: symbol!(
                "mpv_get_property",
                unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_void) -> c_int
            ),
            observe: symbol!(
                "mpv_observe_property",
                unsafe extern "C" fn(*mut c_void, u64, *const c_char, c_int) -> c_int
            ),
            wait: symbol!(
                "mpv_wait_event",
                unsafe extern "C" fn(*mut c_void, f64) -> *const RawEvent
            ),
            wakeup: symbol!(
                "mpv_set_wakeup_callback",
                unsafe extern "C" fn(
                    *mut c_void,
                    Option<unsafe extern "C" fn(*mut c_void)>,
                    *mut c_void,
                )
            ),
            error: symbol!(
                "mpv_error_string",
                unsafe extern "C" fn(c_int) -> *const c_char
            ),
            free: symbol!("mpv_free", unsafe extern "C" fn(*mut c_void)),
            logs: symbol!(
                "mpv_request_log_messages",
                unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int
            ),
            _library: library,
        })
    }
    fn check(&self, result: c_int) -> Result<(), String> {
        if result >= 0 {
            return Ok(());
        }
        // SAFETY: mpv_error_string returns static NUL-terminated string for any error code.
        let error = unsafe { CStr::from_ptr((self.error)(result)) }.to_string_lossy();
        Err(format!("Decoder: {error} ({result})"))
    }
}

#[derive(Clone, Debug)]
pub struct MediaInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
    pub image: bool,
    pub hdr: bool,
    pub audio: bool,
}

fn display_dimensions(width: u32, height: u32, rotation: i32) -> Result<(u32, u32), String> {
    if width == 0 || height == 0 || width > 32768 || height > 32768 {
        return Err("Invalid video dimensions".into());
    }
    match rotation.rem_euclid(360) {
        0 | 180 => Ok((width, height)),
        90 | 270 => Ok((height, width)),
        _ => Err("Video rotation must be a multiple of 90 degrees".into()),
    }
}

fn conversion_plan(
    info: &MediaInfo,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(u32, u32, f64), String> {
    if info.hdr {
        return Err("HDR video is not supported by cache conversion; use the original".into());
    }
    if info.image {
        return Err("Still images do not need video conversion".into());
    }
    if !info.duration.is_finite()
        || info.duration <= 0.0
        || !info.fps.is_finite()
        || info.fps <= 0.0
        || info.fps > 1000.0
    {
        return Err("Video duration or frame rate is invalid".into());
    }
    if !(2..=16384).contains(&width)
        || !(2..=16384).contains(&height)
        || ![24, 30, 45, 60].contains(&fps)
    {
        return Err("Invalid cache dimensions or frame rate".into());
    }
    if info.width < 2 || info.height < 2 {
        return Err("Invalid video dimensions".into());
    }
    let (w, h) = if width <= info.width && height <= info.height {
        (width & !1, height & !1)
    } else {
        let scale = (width as f64 / info.width as f64)
            .min(height as f64 / info.height as f64)
            .min(1.0);
        (
            ((info.width as f64 * scale).floor() as u32) & !1,
            ((info.height as f64 * scale).floor() as u32) & !1,
        )
    };
    if w < 2 || h < 2 {
        return Err("Video aspect ratio is too narrow for this display".into());
    }
    Ok((w, h, info.fps.min(fps as f64)))
}

struct Probed {
    info: MediaInfo,
    rotation: i32,
}

fn probe(dll: &Path, source: &Path) -> Result<Probed, String> {
    let source = crate::core::validate_media(source)?;
    let player = Mpv::headless(dll)?;
    for (name, value) in [
        ("vo", "null"),
        ("ao", "null"),
        ("aid", "no"),
        ("pause", "yes"),
        ("keep-open", "yes"),
        ("idle", "yes"),
        ("video-rotate", "no"),
    ] {
        player.option(name, value)?;
    }
    // SAFETY: owned headless context initialized once with no output device or window.
    player
        .api
        .check(unsafe { (player.api.initialize)(player.handle) })?;
    player.load(&source)?;
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut loaded = false;
    loop {
        let event = unsafe { &*(player.api.wait)(player.handle, 0.05) };
        worker_error(&player, event)?;
        if event.id == 1 {
            return Err("Video probe stopped before metadata was available".into());
        }
        loaded |= event.id == 8;
        if loaded {
            let property = |name: &str| {
                CString::new(name)
                    .ok()
                    .and_then(|name| player.property(&name))
            };
            let number = |name: &str| property(name).and_then(|s| s.parse::<f64>().ok());
            if let (Some(dw), Some(dh)) =
                (number("video-out-params/dw"), number("video-out-params/dh"))
            {
                let rotation = number("video-dec-params/rotate").unwrap_or(0.0) as i32;
                let (width, height) = display_dimensions(dw as u32, dh as u32, rotation)?;
                let count = number("track-list/count").unwrap_or(0.0).clamp(0.0, 256.0) as usize;
                let mut audio = false;
                let mut image = false;
                for n in 0..count {
                    let kind = property(&format!("track-list/{n}/type"));
                    audio |= kind.as_deref() == Some("audio");
                    if kind.as_deref() == Some("video")
                        && property(&format!("track-list/{n}/selected")).as_deref() == Some("yes")
                    {
                        image |= property(&format!("track-list/{n}/image")).as_deref()
                            == Some("yes")
                            || property(&format!("track-list/{n}/albumart")).as_deref()
                                == Some("yes");
                    }
                }
                let gamma = property("video-out-params/gamma")
                    .or_else(|| property("video-dec-params/gamma"))
                    .ok_or("Video color information is unavailable")?;
                let hdr = matches!(gamma.as_str(), "pq" | "hlg" | "st2084" | "smpte2084")
                    || number("video-out-params/sig-peak")
                        .or_else(|| number("video-dec-params/sig-peak"))
                        .is_some_and(|n| n > 1.0);
                let fps = number("container-fps").unwrap_or(0.0);
                let duration = number("duration").unwrap_or(0.0);
                if !image
                    && (!fps.is_finite() || fps <= 0.0 || !duration.is_finite() || duration <= 0.0)
                {
                    return Err("Video duration or frame rate is unavailable".into());
                }
                return Ok(Probed {
                    info: MediaInfo {
                        width,
                        height,
                        fps,
                        duration,
                        image,
                        hdr,
                        audio,
                    },
                    rotation,
                });
            }
        }
        if Instant::now() >= deadline {
            return Err("Video probe timed out after 8 seconds".into());
        }
    }
}

pub fn probe_media(dll: &Path, source: &Path) -> Result<MediaInfo, String> {
    probe(dll, source).map(|p| p.info)
}

fn worker_error(player: &Mpv, event: &RawEvent) -> Result<(), String> {
    if matches!(event.id, 4 | 5) && event.error < 0 {
        return player.api.check(event.error);
    }
    if event.id == 7 && !event.data.is_null() {
        // SAFETY: END_FILE payload valid until the next wait on this owned context.
        let end = unsafe { &*event.data.cast::<EndFile>() };
        match end.reason {
            4 => {
                return Err(player
                    .api
                    .check(end.error)
                    .err()
                    .unwrap_or_else(|| "Could not decode video".into()));
            }
            5 => return Err("Playlists and external references are not allowed".into()),
            _ => {}
        }
    }
    if event.id == 2 && !event.data.is_null() {
        // SAFETY: log payload and strings are borrowed from the current event only.
        let log = unsafe { &*event.data.cast::<LogMessage>() };
        if log.log_level <= 20 && !log.text.is_null() {
            return Err(format!(
                "Native conversion: {}",
                unsafe { CStr::from_ptr(log.text) }.to_string_lossy().trim()
            ));
        }
    }
    Ok(())
}

pub fn encode(
    dll: &Path,
    source: &Path,
    output: &Path,
    width: u32,
    height: u32,
    fps: u32,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Conversion cancelled".into());
    }
    if !output.is_absolute() || output.to_str().is_none() {
        return Err("Cache output path must be absolute Unicode".into());
    }
    crate::core::validate_path_syntax(output)?;
    if output.exists() {
        return Err("Existing media or cache file will not be overwritten".into());
    }
    let source = crate::core::validate_media(source)?;
    let parsed = probe(dll, &source)?;
    let (width, height, rate) = conversion_plan(&parsed.info, width, height, fps)?;
    let parent = output
        .parent()
        .ok_or("Cache output folder is missing")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let parent = parent
        .to_str()
        .ok_or("Cache output folder is not Unicode")?;
    let parent = PathBuf::from(parent.strip_prefix(r"\\?\").unwrap_or(parent));
    crate::core::validate_path_syntax(&parent)?;
    let output = parent.join(
        output
            .file_name()
            .ok_or("Cache output filename is missing")?,
    );
    if output == source || output.exists() {
        return Err("Existing media or cache file will not be overwritten".into());
    }
    let player = Mpv::headless(dll)?;
    let rotate = match parsed.rotation.rem_euclid(360) {
        90 => "transpose=clock,",
        180 => "hflip,vflip,",
        270 => "transpose=cclock,",
        _ => "",
    };
    let filter = format!(
        "lavfi=graph=[{rotate}scale=w={width}:h={height}:flags=bicubic,setsar=1,fps={rate:.6}:round=near,format=yuv420p]:o=[threads=2]"
    );
    for (name, value) in [
        ("of", "mp4"),
        ("ovc", "libx264"),
        ("ovcopts", "preset=veryfast,crf=23,threads=2"),
        ("oac", "aac"),
        ("oacopts", "b=128000,threads=2"),
        ("aid", "auto"),
        ("vo", "lavc"),
        ("ao", "lavc"),
        ("video-rotate", "no"),
        ("loop-file", "no"),
        ("idle", "yes"),
        ("keep-open", "no"),
        ("pause", "no"),
        ("ocopy-metadata", "no"),
        ("untimed", "yes"),
    ] {
        player.option(name, value)?;
    }
    player.option("vf", &filter)?;
    player.option(
        "o",
        output
            .to_str()
            .ok_or("Cache output filename is not Unicode")?,
    )?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Conversion cancelled".into());
    }
    // Exclusive reservation avoids silently truncating any pre-existing user/cache file.
    drop(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|e| format!("Could not reserve cache output: {e}"))?,
    );
    // SAFETY: encoding options are set before initialization on a fresh headless context.
    player
        .api
        .check(unsafe { (player.api.initialize)(player.handle) })?;
    player.load(&source)?;
    let deadline = Instant::now() + Duration::from_secs(15 * 60);
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Conversion cancelled; partial cache is not reusable".into());
        }
        if Instant::now() >= deadline {
            return Err("Conversion timed out after 15 minutes".into());
        }
        if std::fs::metadata(&output).map_err(|e| e.to_string())?.len() > 512 * 1024 * 1024 {
            return Err("Conversion exceeded the 512 MiB cache-file limit".into());
        }
        // SAFETY: one event consumer; event fields read before the next wait invalidates them.
        let event = unsafe { &*(player.api.wait)(player.handle, 0.1) };
        worker_error(&player, event)?;
        if event.id == 7 && !event.data.is_null() {
            let end = unsafe { &*event.data.cast::<EndFile>() };
            if end.reason != 0 {
                return Err("Conversion did not reach natural end of file".into());
            }
            break;
        }
        if event.id == 1 {
            return Err("Encoder stopped before natural end of file".into());
        }
    }
    // Destruction flushes delayed packets, writes the MP4 trailer and closes the file.
    drop(player);
    if cancelled.load(Ordering::Relaxed) {
        return Err("Conversion cancelled; partial cache is not reusable".into());
    }
    let bytes = std::fs::metadata(&output).map_err(|e| e.to_string())?.len();
    if !(1..=512 * 1024 * 1024).contains(&bytes) {
        return Err("Encoded cache is empty or exceeds 512 MiB".into());
    }
    let cache = probe_media(dll, &output)?;
    if cache.width != width
        || cache.height != height
        || cache.hdr
        || cache.image
        || cache.audio != parsed.info.audio
        // mpv's MP4 tail can omit two frame durations; container-fps reports an inflated average on short loops.
        || (cache.fps - rate).abs() > 0.2f64.max(2.0 / cache.duration)
        || (cache.duration - parsed.info.duration).abs() > 0.5f64.max(2.0 / rate)
    {
        return Err(format!(
            "Encoded cache metadata mismatch: output={cache:?}, source={:?}, target={width}x{height} at {rate} FPS",
            parsed.info
        ));
    }
    Ok(())
}

pub struct Mpv {
    handle: *mut c_void,
    api: Api,
    load_request: Cell<u64>,
    current_entry: Cell<Option<i64>>,
}
impl Mpv {
    fn from_handle(handle: *mut c_void, api: Api) -> Self {
        Self {
            handle,
            api,
            load_request: Cell::new(15),
            current_entry: Cell::new(None),
        }
    }
    fn headless(dll: &Path) -> Result<Self, String> {
        let api = Api::load(dll)?;
        // SAFETY: handle belongs to this isolated worker and is destroyed on every exit path.
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err("Could not create decoder".into());
        }
        let player = Self::from_handle(handle, api);
        for (name, value) in [
            ("config", "no"),
            ("load-scripts", "no"),
            ("osc", "no"),
            ("ytdl", "no"),
            ("terminal", "no"),
            ("media-controls", "no"),
            ("stop-screensaver", "no"),
            ("sid", "no"),
            ("autoload-files", "no"),
            ("access-references", "no"),
            ("load-unsafe-playlists", "no"),
            ("demuxer-lavf-o", "protocol_whitelist=[file]"),
            ("cache", "no"),
            ("demuxer-max-bytes", "8MiB"),
            ("demuxer-max-back-bytes", "0"),
            ("demuxer-readahead-secs", "0.5"),
            ("vd-lavc-threads", "2"),
            ("hwdec", "no"),
        ] {
            player.option(name, value)?;
        }
        // SAFETY: logs are copied from the event queue before the next native wait.
        player
            .api
            .check(unsafe { (player.api.logs)(handle, c"error".as_ptr()) })?;
        Ok(player)
    }
    pub fn open_at(dll: &Path, host: HWND, notify: HWND, fit: Fit) -> Result<Self, String> {
        if let Some(adapter) = gpu::preferred_adapter() {
            match Self::open_with_adapter(dll, host, notify, fit, Some(&adapter)) {
                Ok(player) => return Ok(player),
                Err(_) => return Self::open_with_adapter(dll, host, notify, fit, None),
            }
        }
        Self::open_with_adapter(dll, host, notify, fit, None)
    }
    fn open_with_adapter(
        dll: &Path,
        host: HWND,
        notify: HWND,
        fit: Fit,
        adapter: Option<&str>,
    ) -> Result<Self, String> {
        let api = Api::load(dll)?;
        // SAFETY: no concurrent access during initialization; libmpv handle owned by this object.
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err("Could not create decoder".into());
        }
        let player = Self::from_handle(handle, api);
        for (name, value) in [
            ("config", "no"),
            ("load-scripts", "no"),
            ("osc", "no"),
            ("ytdl", "no"),
            ("input-default-bindings", "no"),
            ("input-cursor", "no"),
            ("input-cursor-passthrough", "yes"),
            ("input-vo-keyboard", "no"),
            ("terminal", "no"),
            ("media-controls", "no"),
            ("stop-screensaver", "no"),
            ("aid", "no"),
            ("mute", "yes"),
            ("sid", "no"),
            ("autoload-files", "no"),
            ("access-references", "no"),
            ("load-unsafe-playlists", "no"),
            ("demuxer-lavf-o", "protocol_whitelist=[file]"),
            ("cache", "no"),
            ("demuxer-max-bytes", "8MiB"),
            ("demuxer-max-back-bytes", "0"),
            ("demuxer-readahead-secs", "0.5"),
            // ponytail: four software decode threads bound frame pools; revisit for measured high-resolution drops.
            ("vd-lavc-threads", "4"),
            ("loop-file", "inf"),
            ("image-display-duration", "inf"),
            ("keep-open", "yes"),
            ("vo", "gpu"),
            ("gpu-api", "d3d11"),
            ("gpu-context", "d3d11"),
            ("hwdec", "auto-safe"),
            ("d3d11-flip", "no"),
            ("border", "no"),
            ("osd-level", "0"),
            ("cursor-autohide", "no"),
            ("video-unscaled", "no"),
        ] {
            player.option(name, value)?;
        }
        if let Some(adapter) = adapter {
            player.option("d3d11-adapter", adapter)?;
        }
        player.option("wid", &(host.0 as usize as u32).to_string())?;
        player.option("panscan", if fit == Fit::Fill { "1.0" } else { "0.0" })?;
        // SAFETY: initialized once on owning thread, wakeup only posts HWND notification.
        player
            .api
            .check(unsafe { (player.api.initialize)(handle) })?;
        player
            .api
            .check(unsafe { (player.api.observe)(handle, 0, c"pause".as_ptr(), 3) })?;
        unsafe { (player.api.wakeup)(handle, Some(wakeup), notify.0) };
        Ok(player)
    }
    fn option(&self, name: &str, value: &str) -> Result<(), String> {
        let name = CString::new(name).map_err(|_| "Invalid decoder option")?;
        let value = CString::new(value).map_err(|_| "Invalid decoder option value")?;
        // SAFETY: arguments stay alive during call; no shell string parsing.
        self.api
            .check(unsafe { (self.api.option)(self.handle, name.as_ptr(), value.as_ptr()) })
            .map_err(|error| {
                format!(
                    "Option {}={}: {error}",
                    name.to_string_lossy(),
                    value.to_string_lossy()
                )
            })
    }
    pub fn load(&self, path: &Path) -> Result<u64, String> {
        let filename = CString::new(path.to_str().ok_or("Filename is not valid Unicode")?)
            .map_err(|_| "Invalid filename")?;
        let args = [
            c"loadfile".as_ptr(),
            filename.as_ptr(),
            c"replace".as_ptr(),
            ptr::null(),
        ];
        let request = self
            .load_request
            .get()
            .checked_add(1)
            .ok_or("Decoder load request overflow")?;
        // SAFETY: arguments copied by async API; monotonic request rejects obsolete replies.
        self.api
            .check(unsafe { (self.api.command)(self.handle, request, args.as_ptr()) })?;
        self.load_request.set(request);
        Ok(request)
    }
    pub fn set_fps(&self, fps: u32, source_fps: f64) -> Result<(), String> {
        if !crate::core::FPS_OPTIONS.contains(&fps) || !source_fps.is_finite() || source_fps <= 0.0
        {
            return Err("Invalid playback frame-rate limit or source rate".into());
        }
        let filter = if source_fps <= f64::from(fps) + 0.05 {
            CString::new("").map_err(|_| "Invalid frame-rate filter")?
        } else {
            CString::new(format!("lavfi=[fps=fps={fps}:round=near]"))
                .map_err(|_| "Invalid frame-rate filter")?
        };
        let args = [
            c"vf".as_ptr(),
            c"set".as_ptr(),
            filter.as_ptr(),
            ptr::null(),
        ];
        // SAFETY: async native API copies arguments; a pure FPS graph preserves hardware frame formats.
        self.api
            .check(unsafe { (self.api.command)(self.handle, 7, args.as_ptr()) })
    }
    pub fn seek(&self, position: f64) -> Result<(), String> {
        if !position.is_finite() || position < 0.0 {
            return Err("Seek position must be finite and nonnegative".into());
        }
        let position = CString::new(position.to_string()).map_err(|_| "Invalid seek position")?;
        let args = [
            c"seek".as_ptr(),
            position.as_ptr(),
            c"absolute+exact".as_ptr(),
            ptr::null(),
        ];
        // SAFETY: async command copies the NUL-terminated argument list before returning.
        self.api
            .check(unsafe { (self.api.command)(self.handle, 6, args.as_ptr()) })
    }
    pub fn pause(&self, paused: bool) -> Result<(), String> {
        let mut flag: c_int = paused.into();
        // SAFETY: MPV_FORMAT_FLAG=3 expects int*, copied before async call returns.
        self.api.check(unsafe {
            (self.api.set)(
                self.handle,
                2,
                c"pause".as_ptr(),
                3,
                (&mut flag as *mut c_int).cast(),
            )
        })
    }
    pub fn set_audio(&self, enabled: bool) -> Result<(), String> {
        let mut track = if enabled {
            c"auto".as_ptr()
        } else {
            c"no".as_ptr()
        };
        let mut muted: c_int = (!enabled).into();
        // SAFETY: STRING=1 expects char**, FLAG=3 expects int*; async API copies both values.
        self.api.check(unsafe {
            (self.api.set)(
                self.handle,
                4,
                c"aid".as_ptr(),
                1,
                (&mut track as *mut *const c_char).cast(),
            )
        })?;
        self.api.check(unsafe {
            (self.api.set)(
                self.handle,
                5,
                c"mute".as_ptr(),
                3,
                (&mut muted as *mut c_int).cast(),
            )
        })
    }
    pub fn set_fit(&self, fit: Fit) -> Result<(), String> {
        let mut panscan: f64 = if fit == Fit::Fill { 1.0 } else { 0.0 };
        // SAFETY: MPV_FORMAT_DOUBLE=5 expects double*, copied before the async call returns.
        self.api.check(unsafe {
            (self.api.set)(
                self.handle,
                3,
                c"panscan".as_ptr(),
                5,
                (&mut panscan as *mut f64).cast(),
            )
        })
    }
    pub fn poll(&self) -> Vec<Event> {
        let mut output = Vec::new();
        loop {
            // SAFETY: single event consumer, values copied before next wait_event invalidates pointers.
            let event = unsafe { &*(self.api.wait)(self.handle, 0.0) };
            match event.id {
                0 => break,
                1 => output.push(Event::Failed("Decoder stopped".into())),
                5 if event.request >= 16 => {
                    if event.request != self.load_request.get() {
                        continue;
                    }
                    if event.error < 0 {
                        output.push(Event::Failed(
                            self.api
                                .check(event.error)
                                .err()
                                .unwrap_or_else(|| "Decoder load failed".into()),
                        ));
                    } else if let Some(entry) = unsafe { load_entry(event.data) } {
                        output.push(Event::LoadAccepted {
                            request: event.request,
                            entry,
                        });
                    } else {
                        output.push(Event::Failed(
                            "Decoder load reply has no playlist entry identity".into(),
                        ));
                    }
                }
                6 if !event.data.is_null() => {
                    // SAFETY: START_FILE contains one int64 playlist_entry_id.
                    let entry = unsafe { *event.data.cast::<i64>() };
                    self.current_entry.set(Some(entry));
                    output.push(Event::Started(entry));
                }
                5 if event.request == 7 && event.error < 0 => output.push(Event::FrameRateFailed(
                    self.api
                        .check(event.error)
                        .err()
                        .unwrap_or_else(|| "Frame-rate filter failed".into()),
                )),
                4 | 5 if event.error < 0 => output.push(Event::Failed(
                    self.api
                        .check(event.error)
                        .err()
                        .unwrap_or_else(|| "Decoder failed".into()),
                )),
                7 if !event.data.is_null() => {
                    // SAFETY: END_FILE payload matches repr(C) header layout.
                    let end = unsafe { &*event.data.cast::<EndFile>() };
                    if end.reason == 4 {
                        output.push(Event::FileFailed {
                            entry: end.entry,
                            error: self
                                .api
                                .check(end.error)
                                .err()
                                .unwrap_or_else(|| "Could not play file".into()),
                        });
                    } else if end.reason == 5 {
                        output.push(Event::FileFailed {
                            entry: end.entry,
                            error: "Playlists and external references are not allowed".into(),
                        });
                    }
                }
                id @ (8 | 17) => {
                    // VIDEO_RECONFIG may repeat; consumers must verify current video output before showing.
                    output.extend(Event::video(id, self.current_entry.get()));
                }
                22 if !event.data.is_null() => {
                    // SAFETY: observed only pause in FLAG format; payload valid until next wait.
                    let property = unsafe { &*event.data.cast::<Property>() };
                    if property.format == 3 && !property.data.is_null() {
                        output.push(Event::Pause(unsafe { *property.data.cast::<c_int>() } != 0));
                    }
                }
                _ => {}
            }
        }
        output
    }
    pub fn property(&self, name: &CStr) -> Option<String> {
        let mut value: *mut c_char = ptr::null_mut();
        // SAFETY: STRING=1 writes mpv-owned pointer, freed only through mpv_free below.
        let result = unsafe {
            (self.api.get)(
                self.handle,
                name.as_ptr(),
                1,
                (&mut value as *mut *mut c_char).cast(),
            )
        };
        if result < 0 || value.is_null() {
            return None;
        }
        let text = unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned();
        unsafe { (self.api.free)(value.cast()) };
        Some(text)
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        // SAFETY: no mpv calls race destruction; callback only posts messages and captures no Rust state.
        unsafe {
            (self.api.wakeup)(self.handle, None, ptr::null_mut());
            (self.api.destroy)(self.handle);
        }
    }
}
unsafe extern "C" fn wakeup(data: *mut c_void) {
    // SAFETY: HWND is only used as PostMessage target, no dereference; destroyed HWND causes harmless failure.
    unsafe {
        let _ = PostMessageW(HWND(data), WM_MPV, WPARAM(0), LPARAM(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_load_result_identity_is_typed_and_bounded() {
        let parse = |integer, format| {
            let keys = [c"playlist_entry_id".as_ptr()];
            let value = Node {
                value: NodeValue { integer },
                format,
            };
            let list = NodeList {
                count: 1,
                values: &value,
                keys: keys.as_ptr(),
            };
            let node = Node {
                value: NodeValue { list: &list },
                format: 8,
            };
            unsafe { load_entry((&node as *const Node).cast()) }
        };
        assert_eq!(std::mem::size_of::<Node>(), 16);
        assert_eq!(std::mem::offset_of!(Node, format), 8);
        assert_eq!(parse(42, 4), Some(42));
        assert_eq!(parse(42, 1), None);
        assert_eq!(parse(-1, 4), None);
        assert_eq!(unsafe { load_entry(ptr::null()) }, None);
        assert_eq!(Event::video(8, Some(42)), Some(Event::Loaded(Some(42))));
        assert_eq!(Event::video(17, Some(42)), Some(Event::Ready(Some(42))));
    }
    #[test]
    fn video_events_preserve_order_and_reconfiguration() {
        let classify = |ids: &[c_int]| {
            ids.iter()
                .filter_map(|id| Event::video(*id, None))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            classify(&[6, 8, 17, 21]),
            [Event::Loaded(None), Event::Ready(None)]
        );
        assert_eq!(
            classify(&[17, 8, 17]),
            [Event::Ready(None), Event::Loaded(None), Event::Ready(None)]
        );
        assert_eq!(
            classify(&[7, 6, 8, 17, 17]),
            [Event::Loaded(None), Event::Ready(None), Event::Ready(None)]
        );
    }
    #[test]
    #[ignore = "Requires runtime/libmpv-2.dll; no media or UI is opened"]
    fn native_panscan_async_contract() -> Result<(), String> {
        let api = Api::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/libmpv-2.dll"))?;
        // SAFETY: handle owned by this test, no window or media is created.
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err("Could not create decoder".into());
        }
        let player = Mpv::from_handle(handle, api);
        for name in ["config", "load-scripts", "osc", "ytdl", "audio", "terminal"] {
            player.option(name, "no")?;
        }
        player.option("vo", "null")?;
        player
            .api
            .check(unsafe { (player.api.initialize)(handle) })?;
        for (fit, expected) in [(Fit::Fill, 1.0), (Fit::Contain, 0.0)] {
            player.set_fit(fit)?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                // SAFETY: same test thread consumes events; event copied before next wait.
                let event = unsafe { &*(player.api.wait)(handle, 0.1) };
                if event.id == 4 && event.request == 3 {
                    player.api.check(event.error)?;
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "Missing panscan reply"
                );
            }
            assert_eq!(
                player.property(c"panscan").unwrap().parse::<f64>().unwrap(),
                expected
            );
        }
        Ok(())
    }
    #[test]
    #[ignore = "Requires pinned runtime DLL; headless, no media or audio output is opened"]
    fn native_audio_and_memory_contract() -> Result<(), String> {
        let api = Api::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/libmpv-2.dll"))?;
        // SAFETY: test owns its handle and uses null audio/video outputs, never a desktop window.
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err("Could not create decoder".into());
        }
        let player = Mpv::from_handle(handle, api);
        for (name, value) in [
            ("config", "no"),
            ("load-scripts", "no"),
            ("osc", "no"),
            ("ytdl", "no"),
            ("terminal", "no"),
            ("aid", "no"),
            ("mute", "yes"),
            ("vo", "null"),
            ("ao", "null"),
            ("cache", "no"),
            ("demuxer-max-bytes", "8MiB"),
            ("demuxer-max-back-bytes", "0"),
            ("demuxer-readahead-secs", "0.5"),
            ("vd-lavc-threads", "4"),
        ] {
            player.option(name, value)?;
        }
        player
            .api
            .check(unsafe { (player.api.initialize)(handle) })?;
        assert_eq!(player.property(c"aid").as_deref(), Some("no"));
        assert_eq!(player.property(c"mute").as_deref(), Some("yes"));
        for enabled in [true, false, true, false] {
            player.set_audio(enabled)?;
            let mut replies = 0;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while replies < 2 {
                let event = unsafe { &*(player.api.wait)(handle, 0.1) };
                if event.id == 4 && matches!(event.request, 4 | 5) {
                    player.api.check(event.error)?;
                    replies += 1;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "Missing audio property reply"
                );
            }
            assert_eq!(
                player.property(c"aid").as_deref(),
                Some(if enabled { "auto" } else { "no" })
            );
            assert_eq!(
                player.property(c"mute").as_deref(),
                Some(if enabled { "no" } else { "yes" })
            );
        }
        for (name, expected) in [
            (c"options/vd-lavc-threads", "4"),
            (c"options/demuxer-max-bytes", "8388608"),
            (c"options/demuxer-max-back-bytes", "0"),
        ] {
            assert_eq!(player.property(name).as_deref(), Some(expected));
        }
        assert_eq!(
            player
                .property(c"options/demuxer-readahead-secs")
                .unwrap()
                .parse::<f64>()
                .unwrap(),
            0.5
        );
        Ok(())
    }
    #[test]
    #[ignore = "Requires pinned runtime DLL; synthesized PCM uses null audio/video outputs"]
    fn native_audio_toggle_keeps_loaded_media() -> Result<(), String> {
        use std::{
            fs,
            time::{Duration, Instant, SystemTime, UNIX_EPOCH},
        };
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "scratch/native-audio-{}-{stamp}.wav",
            std::process::id()
        ));
        let data_bytes = 8000u32 * 2 * 5;
        let mut wave = Vec::with_capacity(44 + data_bytes as usize);
        wave.extend_from_slice(b"RIFF");
        wave.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        wave.extend_from_slice(b"WAVEfmt ");
        wave.extend_from_slice(&16u32.to_le_bytes());
        wave.extend_from_slice(&1u16.to_le_bytes());
        wave.extend_from_slice(&1u16.to_le_bytes());
        wave.extend_from_slice(&8000u32.to_le_bytes());
        wave.extend_from_slice(&16000u32.to_le_bytes());
        wave.extend_from_slice(&2u16.to_le_bytes());
        wave.extend_from_slice(&16u16.to_le_bytes());
        wave.extend_from_slice(b"data");
        wave.extend_from_slice(&data_bytes.to_le_bytes());
        wave.resize(44 + data_bytes as usize, 0);
        // ponytail: keep unique test media in ignored scratch; no automatic user-data deletion.
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(&path, wave).map_err(|e| e.to_string())?;
        let api = Api::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/libmpv-2.dll"))?;
        // SAFETY: owned native context, no window and null audio output; no user process touched.
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err("Could not create decoder".into());
        }
        let player = Mpv::from_handle(handle, api);
        for (name, value) in [
            ("config", "no"),
            ("load-scripts", "no"),
            ("osc", "no"),
            ("ytdl", "no"),
            ("terminal", "no"),
            ("vo", "null"),
            ("ao", "null"),
            ("aid", "auto"),
            ("mute", "yes"),
            ("pause", "yes"),
            ("keep-open", "always"),
            ("cache", "no"),
            ("access-references", "no"),
            ("autoload-files", "no"),
        ] {
            player.option(name, value)?;
        }
        player
            .api
            .check(unsafe { (player.api.initialize)(handle) })?;
        let request = player.load(&path)?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let event = unsafe { &*(player.api.wait)(handle, 0.1) };
            if event.id == 8 {
                break;
            }
            if event.id == 5 && event.request == request {
                player.api.check(event.error)?;
            }
            assert!(Instant::now() < deadline, "Native PCM did not load");
        }
        let loaded_path = player.property(c"path");
        assert!(loaded_path.is_some());
        for enabled in [true, false, true, false] {
            player.set_audio(enabled)?;
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut replies = 0;
            while replies < 2 {
                let event = unsafe { &*(player.api.wait)(handle, 0.1) };
                assert_ne!(event.id, 6, "Audio toggle restarted media loading");
                if event.id == 4 && matches!(event.request, 4 | 5) {
                    player.api.check(event.error)?;
                    replies += 1;
                }
                assert!(Instant::now() < deadline, "Missing audio toggle reply");
            }
            let track = player.property(c"aid").unwrap();
            if enabled {
                assert!(
                    track.parse::<u64>().is_ok(),
                    "Audio track not selected: {track}"
                );
            } else {
                assert_eq!(track, "no");
            }
            assert_eq!(
                player.property(c"mute").as_deref(),
                Some(if enabled { "no" } else { "yes" })
            );
            assert_eq!(player.property(c"path"), loaded_path);
            assert_eq!(player.property(c"pause").as_deref(), Some("yes"));
        }
        println!(
            "Native PCM audio toggle passed without reload; fixture={}",
            path.display()
        );
        Ok(())
    }
    #[test]
    fn conversion_plan_caps_size_fps_and_rejects_unsafe_media() {
        let mut info = MediaInfo {
            width: 1920,
            height: 1080,
            fps: 60.0,
            duration: 5.0,
            image: false,
            hdr: false,
            audio: false,
        };
        assert_eq!(
            conversion_plan(&info, 3840, 2160, 30).unwrap(),
            (1920, 1080, 30.0)
        );
        assert_eq!(
            conversion_plan(&info, 1280, 720, 45).unwrap(),
            (1280, 720, 45.0)
        );
        info.fps = 23.976;
        assert_eq!(conversion_plan(&info, 1280, 720, 30).unwrap().2, 23.976);
        assert!(conversion_plan(&info, 1280, 720, 25).is_err());
        assert!(conversion_plan(&info, 1, 720, 30).is_err());
        info.hdr = true;
        assert!(
            conversion_plan(&info, 1280, 720, 30)
                .unwrap_err()
                .contains("HDR")
        );
        info.hdr = false;
        info.image = true;
        assert!(conversion_plan(&info, 1280, 720, 30).is_err());
        info.image = false;
        info.duration = f64::NAN;
        assert!(conversion_plan(&info, 1280, 720, 30).is_err());
    }
    #[test]
    fn display_dimensions_include_rotation_and_sample_aspect() {
        assert_eq!(display_dimensions(1920, 1080, 90).unwrap(), (1080, 1920));
        assert_eq!(display_dimensions(1024, 576, 180).unwrap(), (1024, 576));
        assert!(display_dimensions(1920, 1080, 45).is_err());
        assert!(display_dimensions(0, 1080, 0).is_err());
    }
    #[test]
    fn cancelled_conversion_does_not_open_decoder_or_output() {
        let cancelled = AtomicBool::new(true);
        let error = encode(
            Path::new(r"C:\missing\libmpv-2.dll"),
            Path::new(r"C:\missing\input.mp4"),
            Path::new(r"C:\missing\output.mp4"),
            320,
            180,
            30,
            &cancelled,
        )
        .unwrap_err();
        assert!(error.contains("cancelled"));
    }
    #[test]
    #[ignore = "Requires pinned runtime DLL and owned scratch/media/sample.mp4; creates only a unique scratch cache copy"]
    fn native_probe_encode_and_finalize() -> Result<(), String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let dll = root.join("runtime/libmpv-2.dll");
        let source = std::env::var_os("WALLPAPERLIVE_ENCODE_TEST_MEDIA")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("scratch/media/sample.mp4"));
        let original = std::fs::read(&source).map_err(|e| e.to_string())?;
        let info = probe_media(&dll, &source)?;
        assert!(info.width > 0 && info.height > 0 && info.fps > 0.0 && info.duration > 0.0);
        assert!(!info.image && !info.hdr);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let output = root.join(format!(
            "scratch/native-cache-{}-{stamp}.mp4",
            std::process::id()
        ));
        encode(
            &dll,
            &source,
            &output,
            320,
            180,
            30,
            &AtomicBool::new(false),
        )?;
        let cache = probe_media(&dll, &output)?;
        assert!(cache.width <= 320 && cache.height <= 180);
        assert!((cache.fps - info.fps.min(30.0)).abs() <= 0.2f64.max(2.0 / cache.duration));
        assert!((cache.duration - info.duration).abs() < 0.5);
        assert_eq!(std::fs::read(&source).map_err(|e| e.to_string())?, original);
        assert!(
            encode(
                &dll,
                &source,
                &output,
                320,
                180,
                30,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        println!(
            "Native cache validated: {cache:?}; output={}",
            output.display()
        );
        Ok(())
    }
    #[test]
    fn missing_decoder_has_actionable_error() {
        let error = Mpv::open_at(
            Path::new(r"C:\missing-decoder-931251\libmpv-2.dll"),
            HWND::default(),
            HWND::default(),
            Fit::Fill,
        )
        .err()
        .unwrap();
        assert!(error.to_ascii_lowercase().contains("decoder") && !error.is_empty());
    }
}
