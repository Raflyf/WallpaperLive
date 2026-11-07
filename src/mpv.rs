use crate::core::Fit;
use std::{
    ffi::{CStr, CString, c_char, c_int, c_ulong, c_void},
    path::Path,
    ptr,
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

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Loaded,
    Ready,
    Failed(String),
    Pause(bool),
}
impl Event {
    fn video(id: c_int) -> Option<Self> {
        match id {
            8 => Some(Self::Loaded),
            17 => Some(Self::Ready),
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

pub struct Mpv {
    handle: *mut c_void,
    api: Api,
}
impl Mpv {
    pub fn open_at(dll: &Path, host: HWND, notify: HWND, fit: Fit) -> Result<Self, String> {
        let api = Api::load(dll)?;
        // SAFETY: no concurrent access during initialization; libmpv handle owned by this object.
        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err("Could not create decoder".into());
        }
        let player = Self { handle, api };
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
    }
    pub fn load(&self, path: &Path) -> Result<(), String> {
        let filename = CString::new(path.to_str().ok_or("Filename is not valid Unicode")?)
            .map_err(|_| "Invalid filename")?;
        let args = [
            c"loadfile".as_ptr(),
            filename.as_ptr(),
            c"replace".as_ptr(),
            ptr::null(),
        ];
        // SAFETY: libmpv copies command args before returning, each string remains valid during call.
        self.api
            .check(unsafe { (self.api.command)(self.handle, 1, args.as_ptr()) })
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
                        output.push(Event::Failed(
                            self.api
                                .check(end.error)
                                .err()
                                .unwrap_or_else(|| "Could not play file".into()),
                        ));
                    } else if end.reason == 5 {
                        output.push(Event::Failed(
                            "Playlists and external references are not allowed".into(),
                        ));
                    }
                }
                id @ (8 | 17) => {
                    // VIDEO_RECONFIG may repeat; consumers must verify current video output before showing.
                    output.extend(Event::video(id));
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
    fn video_events_preserve_order_and_reconfiguration() {
        let classify = |ids: &[c_int]| {
            ids.iter()
                .filter_map(|id| Event::video(*id))
                .collect::<Vec<_>>()
        };
        assert_eq!(classify(&[6, 8, 17, 21]), [Event::Loaded, Event::Ready]);
        assert_eq!(
            classify(&[17, 8, 17]),
            [Event::Ready, Event::Loaded, Event::Ready]
        );
        assert_eq!(
            classify(&[7, 6, 8, 17, 17]),
            [Event::Loaded, Event::Ready, Event::Ready]
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
        let player = Mpv { handle, api };
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
        let player = Mpv { handle, api };
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
        let player = Mpv { handle, api };
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
        player.load(&path)?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let event = unsafe { &*(player.api.wait)(handle, 0.1) };
            if event.id == 8 {
                break;
            }
            if event.id == 5 && event.request == 1 {
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
