use crate::{
    tray,
    visuals::{
        CLOCK_CTX_24H, CLOCK_CTX_BEHIND_ICONS, CLOCK_CTX_LOCK, CLOCK_CTX_OPEN_SETTINGS,
        CLOCK_CTX_POS_SAVED, CLOCK_CTX_REMOVE, CLOCK_CTX_SIZE_DELTA, CLOCK_REPOSITION,
    },
};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    mem::size_of,
    sync::OnceLock,
};
use windows::{
    Win32::{
        Foundation::{
            BOOL, COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, SYSTEMTIME, WPARAM,
        },
        Graphics::Gdi::{
            AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, AddFontMemResourceEx, BI_RGB,
            BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CLIP_DEFAULT_PRECIS, CreateCompatibleDC,
            CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS,
            DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW,
            FF_DONTCARE, FW_BOLD, GdiFlush, HDC, HGDIOBJ, OUT_DEFAULT_PRECIS, SelectObject,
            SetBkMode, SetTextCharacterExtra, SetTextColor, TRANSPARENT,
        },
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
            MonitorFromWindow,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
            HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI},
            Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture, VK_ESCAPE},
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, w},
};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLocalTime(lpSystemTime: *mut SYSTEMTIME);
}

unsafe fn local_time() -> SYSTEMTIME {
    let mut st = SYSTEMTIME::default();
    // SAFETY: valid writable stack SYSTEMTIME for kernel32 GetLocalTime.
    unsafe {
        GetLocalTime(&mut st);
    }
    st
}

const CLOCK_CLASS: PCWSTR = w!("WallpaperLive.DayClock");
const CLOCK_TIMER_ID: usize = 91;
const CLOCK_ZORDER_SETTLE_TIMER_ID: usize = 92;
const CTX_LOCAL_DONE_REPOS: usize = 9001;

const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
const WINEVENT_SKIPOWNPROCESS: u32 = 0x0002;

static QUICKSAND_OTF: &[u8] = include_bytes!("../assets/Quicksand-Bold.otf");
static FONT_REGISTERED: OnceLock<bool> = OnceLock::new();

fn ensure_quicksand_loaded() {
    let _ = FONT_REGISTERED.get_or_init(|| {
        let count = 0u32;
        // SAFETY: QUICKSAND_OTF is static process-lifetime bytes; AddFontMemResourceEx registers it cleanly.
        let handle = unsafe {
            AddFontMemResourceEx(
                QUICKSAND_OTF.as_ptr() as *const c_void,
                QUICKSAND_OTF.len() as u32,
                None,
                &count,
            )
        };
        !handle.0.is_null()
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockConfig {
    pub enabled: bool,
    pub locked: bool,
    pub behind_icons: bool,
    pub use_24h: bool,
    pub size_pct: u32,
    pub opacity_pct: u32,
    pub color: u32,
    pub pos: Option<(i32, i32)>,
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            locked: false,
            behind_icons: true,
            use_24h: true,
            size_pct: 100,
            opacity_pct: 100,
            color: 0x00FF_FFFF,
            pos: None,
        }
    }
}

#[derive(Default)]
struct ClockRuntime {
    hwnd: isize,
    owner: isize,
    action_msg: u32,
    cfg: ClockConfig,
    repositioning: bool,
    dragging: bool,
    drag_moved: bool,
    drag_start_x: i32,
    drag_start_y: i32,
    drag_offset_x: i32,
    drag_offset_y: i32,
    last_stamp: (u16, u16, u16, u16, u16, u16),
    last_bounds: Option<(i32, i32, i32, i32, bool)>,
    pending_wheel_delta: i32,
    hooks: [isize; 2],
}

#[derive(Default)]
struct ClockTextScratch {
    dc: isize,
    bmp: isize,
    bits: usize,
    w: i32,
    h: i32,
}

impl ClockTextScratch {
    fn free(&mut self) {
        unsafe {
            if self.bmp != 0 {
                let _ = DeleteObject(HGDIOBJ(self.bmp as *mut c_void));
            }
            if self.dc != 0 {
                let _ = DeleteDC(HDC(self.dc as *mut c_void));
            }
        }
        *self = Self::default();
    }

    fn ensure(&mut self, w: i32, h: i32) -> Option<(HDC, *mut u32)> {
        if w <= 0 || h <= 0 {
            return None;
        }
        if self.w != w || self.h != h || self.dc == 0 || self.bmp == 0 || self.bits == 0 {
            self.free();
            // SAFETY: owns memory DC and 32bpp DIB section strictly inside UI thread-local scratch.
            unsafe {
                let dc = CreateCompatibleDC(None);
                if dc.0.is_null() {
                    return None;
                }
                let mut bits = std::ptr::null_mut();
                let Ok(bmp) = CreateDIBSection(
                    dc,
                    &BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: w,
                            biHeight: -h,
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
                ) else {
                    let _ = DeleteDC(dc);
                    return None;
                };
                if bits.is_null() {
                    let _ = DeleteObject(bmp);
                    let _ = DeleteDC(dc);
                    return None;
                }
                let _ = SelectObject(dc, HGDIOBJ(bmp.0));
                self.dc = dc.0 as isize;
                self.bmp = bmp.0 as isize;
                self.bits = bits as usize;
                self.w = w;
                self.h = h;
            }
        }
        Some((HDC(self.dc as *mut c_void), self.bits as *mut u32))
    }
}

thread_local! {
    static CLOCK_RUNTIME: RefCell<Option<ClockRuntime>> = const { RefCell::new(None) };
    static CLOCK_SAVED_POS: RefCell<Option<(i32, i32)>> = const { RefCell::new(None) };
    static CLOCK_WHEEL_STEPS: RefCell<i32> = const { RefCell::new(0) };
    static CLOCK_TEXT_SCRATCH: RefCell<ClockTextScratch> = const { RefCell::new(ClockTextScratch {
        dc: 0,
        bmp: 0,
        bits: 0,
        w: 0,
        h: 0,
    }) };
    static ALLOW_INTERNAL_HIDE: Cell<bool> = const { Cell::new(false) };
    static IN_ZORDER_SYNC: Cell<bool> = const { Cell::new(false) };
}

fn minute_timer_delay_ms(second: u16) -> u32 {
    // Wait until right after the next minute boundary so idle CPU stays near zero.
    (60u32.saturating_sub(second.min(59) as u32)).clamp(1, 60) * 1000 + 50
}

pub fn take_saved_position() -> Option<(i32, i32)> {
    CLOCK_SAVED_POS.with(|slot| slot.borrow_mut().take())
}

pub fn take_wheel_steps() -> i32 {
    CLOCK_WHEEL_STEPS.with(|slot| std::mem::take(&mut *slot.borrow_mut()))
}

pub fn is_repositioning() -> bool {
    CLOCK_RUNTIME.with(|slot| slot.borrow().as_ref().is_some_and(|rt| rt.repositioning))
}

fn unhook_events(hooks: [isize; 2]) {
    for raw in hooks {
        if raw != 0 {
            // SAFETY: unhooks HWINEVENTHOOK created on this UI thread.
            unsafe {
                let _ = UnhookWinEvent(HWINEVENTHOOK(raw as *mut c_void));
            }
        }
    }
}

fn install_events() -> [isize; 2] {
    // SAFETY: out-of-context UI-thread WinEvent hooks skip own process so idle CPU stays zero.
    unsafe {
        let h1 = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        let h2 = SetWinEventHook(
            EVENT_SYSTEM_MINIMIZESTART,
            EVENT_SYSTEM_MINIMIZEEND,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        [h1.0 as isize, h2.0 as isize]
    }
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    event_hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    let clock_raw = CLOCK_RUNTIME.with(|s| s.borrow().as_ref().map_or(0, |r| r.hwnd));
    if clock_raw == 0 || event_hwnd.0 as isize == clock_raw {
        return;
    }
    ensure_desktop_visible();
    let clock_hwnd = HWND(clock_raw as *mut c_void);
    if unsafe { IsWindow(clock_hwnd).as_bool() } {
        // Re-check once after Explorer finishes Win+D transition animation.
        unsafe {
            let _ = SetTimer(clock_hwnd, CLOCK_ZORDER_SETTLE_TIMER_ID, 70, None);
        }
    }
}

#[derive(Default)]
struct DesktopTopSearch {
    progman: HWND,
    top_shell: HWND,
}

unsafe extern "system" fn find_top_shell_cb(window: HWND, data: LPARAM) -> BOOL {
    // SAFETY: read-only EnumWindows top-to-bottom scan matching Explorer's desktop host windows.
    unsafe {
        if !IsWindowVisible(window).as_bool() {
            return BOOL(1);
        }
        let out = &mut *(data.0 as *mut DesktopTopSearch);
        if window == out.progman {
            out.top_shell = window;
            return BOOL(0);
        }
        let mut cls = [0u16; 32];
        let len = GetClassNameW(window, &mut cls);
        if len == 7
            && cls[..7]
                == [
                    'W' as u16, 'o' as u16, 'r' as u16, 'k' as u16, 'e' as u16, 'r' as u16,
                    'W' as u16,
                ]
            && FindWindowExW(window, None, w!("SHELLDLL_DefView"), PCWSTR::null()).is_ok()
        {
            out.top_shell = window;
            return BOOL(0);
        }
    }
    BOOL(1)
}

fn desktop_insert_after(clock_hwnd: HWND, repositioning: bool) -> HWND {
    if repositioning {
        return HWND_TOP;
    }
    // SAFETY: walks global Win32 non-topmost Z-order right above the topmost desktop host (Progman/WorkerW).
    unsafe {
        let progman = FindWindowW(w!("Progman"), PCWSTR::null()).unwrap_or_default();
        let mut search = DesktopTopSearch {
            progman,
            top_shell: HWND::default(),
        };
        let _ = EnumWindows(
            Some(find_top_shell_cb),
            LPARAM((&mut search as *mut DesktopTopSearch) as isize),
        );
        let shell_top = if !search.top_shell.0.is_null() {
            search.top_shell
        } else {
            progman
        };
        if shell_top.0.is_null() {
            return HWND_BOTTOM;
        }
        let mut prev = GetWindow(shell_top, GW_HWNDPREV).unwrap_or_default();
        let mut hops = 0usize;
        while !prev.0.is_null() && hops < 256 {
            hops += 1;
            if prev == clock_hwnd {
                prev = GetWindow(prev, GW_HWNDPREV).unwrap_or_default();
                continue;
            }
            let ex = GetWindowLongPtrW(prev, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOPMOST.0 != 0 {
                // Reached the bottom of the topmost band; placing at HWND_TOP keeps clock above desktop.
                return HWND_TOP;
            }
            return prev;
        }
        HWND_TOP
    }
}

fn place_on_desktop(
    hwnd: HWND,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    resize_move: bool,
    repositioning: bool,
) {
    let after = desktop_insert_after(hwnd, repositioning);
    let mut flags = SWP_NOACTIVATE | SWP_SHOWWINDOW;
    if !resize_move {
        flags |= SWP_NOMOVE | SWP_NOSIZE;
    }
    IN_ZORDER_SYNC.with(|g| g.set(true));
    // SAFETY: owns clock HWND; IN_ZORDER_SYNC prevents WM_WINDOWPOSCHANGING recursion.
    unsafe {
        let _ = SetWindowPos(hwnd, after, x, y, w, h, flags);
    }
    IN_ZORDER_SYNC.with(|g| g.set(false));
}

pub fn ensure_desktop_visible() {
    let state = CLOCK_RUNTIME.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|rt| (rt.hwnd != 0 && rt.cfg.enabled).then_some((rt.hwnd, rt.repositioning)))
    });
    let Some((raw, repositioning)) = state else {
        return;
    };
    let hwnd = HWND(raw as *mut c_void);
    if unsafe { !IsWindow(hwnd).as_bool() } {
        return;
    }
    if unsafe { IsIconic(hwnd).as_bool() || !IsWindowVisible(hwnd).as_bool() } {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }
    place_on_desktop(hwnd, 0, 0, 0, 0, false, repositioning);
}

pub fn sync_zorder() {
    ensure_desktop_visible();
}

fn ensure_class() -> Result<(), String> {
    let instance = unsafe { GetModuleHandleW(None) }.map_err(|e| e.to_string())?;
    let mut existing = WNDCLASSEXW::default();
    if unsafe { GetClassInfoExW(instance, CLOCK_CLASS, &mut existing) }.is_ok() {
        return Ok(());
    }
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(clock_proc),
        hInstance: instance.into(),
        lpszClassName: CLOCK_CLASS,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.map_err(|e| e.to_string())?,
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&wc) } == 0 {
        let mut check = WNDCLASSEXW::default();
        if unsafe { GetClassInfoExW(instance, CLOCK_CLASS, &mut check) }.is_err() {
            return Err("Could not register the desktop clock window class".into());
        }
    }
    Ok(())
}

fn destroy_clock_hwnd(raw: isize, hooks: [isize; 2]) {
    unhook_events(hooks);
    if raw == 0 {
        return;
    }
    let hwnd = HWND(raw as *mut c_void);
    ALLOW_INTERNAL_HIDE.with(|g| g.set(true));
    // SAFETY: owns this single unowned popup HWND on the UI thread.
    unsafe {
        if IsWindow(hwnd).as_bool() {
            let _ = KillTimer(hwnd, CLOCK_TIMER_ID);
            let _ = KillTimer(hwnd, CLOCK_ZORDER_SETTLE_TIMER_ID);
            let _ = DestroyWindow(hwnd);
        }
    }
    ALLOW_INTERNAL_HIDE.with(|g| g.set(false));
}

pub fn hide_all() {
    let old = CLOCK_RUNTIME.with(|slot| slot.borrow_mut().take().map(|r| (r.hwnd, r.hooks)));
    if let Some((raw, hooks)) = old {
        destroy_clock_hwnd(raw, hooks);
    }
    CLOCK_TEXT_SCRATCH.with(|s| s.borrow_mut().free());
}

pub fn begin_reposition(owner: HWND, action_msg: u32, cfg: ClockConfig) {
    if !cfg.enabled {
        return;
    }
    CLOCK_RUNTIME.with(|slot| {
        if let Some(rt) = slot.borrow_mut().as_mut() {
            rt.repositioning = true;
        }
    });
    sync_inner(owner, action_msg, cfg, true);
}

pub fn end_reposition(owner: HWND, action_msg: u32, cfg: ClockConfig) {
    CLOCK_RUNTIME.with(|slot| {
        if let Some(rt) = slot.borrow_mut().as_mut() {
            rt.repositioning = false;
            rt.dragging = false;
        }
    });
    if cfg.enabled {
        sync_inner(owner, action_msg, cfg, false);
    }
}

pub fn sync(owner: HWND, action_msg: u32, cfg: ClockConfig) {
    let rep = is_repositioning();
    sync_inner(owner, action_msg, cfg, rep);
}

fn is_click_through(cfg: ClockConfig, repositioning: bool) -> bool {
    !repositioning && (cfg.locked || cfg.behind_icons)
}

fn sync_inner(owner: HWND, action_msg: u32, cfg: ClockConfig, repositioning: bool) {
    if !cfg.enabled {
        hide_all();
        return;
    }
    if ensure_class().is_err() {
        return;
    }
    ensure_quicksand_loaded();

    let (need_create, existing_raw) = CLOCK_RUNTIME.with(|slot| {
        let borrow = slot.borrow();
        let Some(rt) = borrow.as_ref() else {
            return (true, 0);
        };
        let valid = rt.hwnd != 0 && unsafe { IsWindow(HWND(rt.hwnd as *mut c_void)).as_bool() };
        (!valid, rt.hwnd)
    });

    let click_through = is_click_through(cfg, repositioning);
    let mut ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    if click_through {
        ex |= WS_EX_TRANSPARENT;
    }

    if need_create {
        if existing_raw != 0 {
            destroy_clock_hwnd(existing_raw, [0, 0]);
        }
        let Ok(instance) = (unsafe { GetModuleHandleW(None) }) else {
            return;
        };
        // Rainmeter model: ALWAYS unowned top-level WS_POPUP so minimizing/hiding App or Win+D never hides clock.
        let Ok(hwnd) = (unsafe {
            CreateWindowExW(
                ex,
                CLOCK_CLASS,
                w!("WallpaperLive Day Clock"),
                WS_POPUP,
                0,
                0,
                100,
                100,
                HWND::default(),
                None,
                instance,
                None,
            )
        }) else {
            return;
        };
        let hooks = install_events();
        let now = unsafe { local_time() };
        unsafe {
            let _ = SetTimer(
                hwnd,
                CLOCK_TIMER_ID,
                minute_timer_delay_ms(now.wSecond),
                None,
            );
        }
        CLOCK_RUNTIME.with(|slot| {
            *slot.borrow_mut() = Some(ClockRuntime {
                hwnd: hwnd.0 as isize,
                owner: owner.0 as isize,
                action_msg,
                cfg,
                repositioning,
                dragging: false,
                drag_moved: false,
                drag_start_x: 0,
                drag_start_y: 0,
                drag_offset_x: 0,
                drag_offset_y: 0,
                last_stamp: (0, 0, 0, 0, 0, 0),
                last_bounds: None,
                pending_wheel_delta: 0,
                hooks,
            });
        });
        render_and_place(hwnd);
    } else {
        let hwnd = HWND(existing_raw as *mut c_void);
        CLOCK_RUNTIME.with(|slot| {
            if let Some(rt) = slot.borrow_mut().as_mut() {
                rt.owner = owner.0 as isize;
                rt.action_msg = action_msg;
                rt.cfg = cfg;
                rt.repositioning = repositioning;
            }
        });
        unsafe {
            let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let next = if click_through {
                (cur & !WS_EX_TOPMOST.0) | WS_EX_TRANSPARENT.0
            } else {
                cur & !(WS_EX_TRANSPARENT.0 | WS_EX_TOPMOST.0)
            };
            if next != cur {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next as isize);
            }
        }
        render_and_place(hwnd);
    }
}

fn month_name(m: u16) -> &'static str {
    match m {
        1 => "JANUARY",
        2 => "FEBRUARY",
        3 => "MARCH",
        4 => "APRIL",
        5 => "MAY",
        6 => "JUNE",
        7 => "JULY",
        8 => "AUGUST",
        9 => "SEPTEMBER",
        10 => "OCTOBER",
        11 => "NOVEMBER",
        12 => "DECEMBER",
        _ => "",
    }
}

fn weekday_name(dow: u16) -> &'static str {
    match dow {
        0 => "SUNDAY",
        1 => "MONDAY",
        2 => "TUESDAY",
        3 => "WEDNESDAY",
        4 => "THURSDAY",
        5 => "FRIDAY",
        6 => "SATURDAY",
        _ => "DAY",
    }
}

pub fn format_clock_lines(
    year: u16,
    month: u16,
    dow: u16,
    day: u16,
    hour: u16,
    minute: u16,
    use_24h: bool,
) -> (String, String, String) {
    let day_str = weekday_name(dow).to_owned();
    let date_str = format!("{day:02}  {},  {year}.", month_name(month));
    let time_str = if use_24h {
        format!("- {hour:02}:{minute:02} -")
    } else {
        let suffix = if hour >= 12 { "PM" } else { "AM" };
        let h12 = match hour % 12 {
            0 => 12,
            h => h,
        };
        format!("- {h12}:{minute:02} {suffix} -")
    };
    (day_str, date_str, time_str)
}

#[derive(Clone, Copy)]
enum AnuratiSeg {
    Move(f32, f32),
    Line(f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

#[derive(Clone, Copy)]
struct AnuratiGlyph {
    advance: f32,
    visual_width: f32,
    ops: &'static [AnuratiSeg],
}

use AnuratiSeg::{Close as Z, Cubic as C, Line as L, Move as M};

// Exact Mond Anurati glyph contours at em=100.0 (cap top=-80.0, baseline=0.0).
const GLYPH_S: AnuratiGlyph = AnuratiGlyph {
    advance: 80.3,
    visual_width: 66.6,
    ops: &[
        M(43.2, -33.3),
        C(46.0, -33.3, 48.3, -32.3, 50.3, -30.3),
        C(52.2, -28.3, 53.3, -26.0, 53.3, -23.3),
        C(53.3, -21.9, 53.0, -20.6, 52.5, -19.4),
        C(52.0, -18.1, 51.3, -17.1, 50.4, -16.2),
        C(49.5, -15.3, 48.4, -14.6, 47.1, -14.1),
        C(45.9, -13.6, 44.6, -13.4, 43.2, -13.4),
        L(43.2, 0.0),
        C(49.7, 0.0, 55.2, -2.3, 59.7, -6.8),
        C(64.3, -11.4, 66.6, -16.9, 66.6, -23.3),
        C(66.6, -29.7, 64.3, -35.2, 59.7, -39.8),
        C(55.2, -44.3, 49.7, -46.6, 43.2, -46.6),
        L(23.3, -46.6),
        C(20.4, -46.6, 18.1, -47.6, 16.1, -49.6),
        C(14.2, -51.6, 13.3, -53.9, 13.3, -56.7),
        C(13.3, -58.0, 13.5, -59.3, 14.0, -60.5),
        C(14.5, -61.7, 15.2, -62.8, 16.1, -63.7),
        C(17.1, -64.7, 18.1, -65.4, 19.4, -65.9),
        C(20.6, -66.4, 21.9, -66.6, 23.3, -66.6),
        L(23.3, -80.0),
        C(16.8, -80.0, 11.3, -77.7, 6.8, -73.1),
        C(2.3, -68.5, 0.0, -63.1, 0.0, -56.7),
        C(0.0, -50.2, 2.3, -44.7, 6.8, -40.1),
        C(11.3, -35.6, 16.8, -33.3, 23.3, -33.3),
        Z,
    ],
};

const GLYPH_U: AnuratiGlyph = AnuratiGlyph {
    advance: 78.2,
    visual_width: 66.6,
    ops: &[
        M(53.3, -33.3),
        C(53.3, -27.9, 51.2, -23.2, 47.3, -19.3),
        C(43.4, -15.4, 38.7, -13.4, 33.2, -13.4),
        C(27.7, -13.4, 22.9, -15.3, 19.0, -19.3),
        C(15.2, -23.1, 13.3, -27.8, 13.3, -33.3),
        L(13.3, -80.0),
        L(0.0, -80.0),
        L(0.0, -33.3),
        C(0.0, -24.2, 3.2, -16.2, 9.7, -9.7),
        C(16.2, -3.2, 24.1, 0.0, 33.2, 0.0),
        C(42.3, 0.0, 50.3, -3.2, 56.8, -9.7),
        C(63.4, -16.2, 66.6, -24.1, 66.6, -33.3),
        Z,
    ],
};

const GLYPH_N: AnuratiGlyph = AnuratiGlyph {
    advance: 81.8,
    visual_width: 66.5,
    ops: &[
        M(53.2, -80.0),
        L(53.2, -32.1),
        L(0.0, -80.0),
        L(0.0, -59.8),
        L(66.5, 0.0),
        L(66.5, -80.0),
        Z,
    ],
};

const GLYPH_D: AnuratiGlyph = AnuratiGlyph {
    advance: 83.0,
    visual_width: 66.6,
    ops: &[
        M(63.5, -55.4),
        C(61.3, -60.3, 58.5, -64.5, 54.8, -68.2),
        C(51.2, -71.8, 46.9, -74.7, 42.1, -76.8),
        C(37.2, -78.9, 32.1, -80.0, 26.6, -80.0),
        L(0.0, -80.0),
        L(13.3, -66.6),
        L(26.6, -66.6),
        C(30.2, -66.6, 33.7, -65.9, 36.9, -64.6),
        C(40.1, -63.2, 43.0, -61.2, 45.4, -58.8),
        C(47.9, -56.3, 49.8, -53.5, 51.2, -50.3),
        C(52.6, -47.0, 53.3, -43.6, 53.3, -40.0),
        C(53.3, -36.4, 52.6, -33.0, 51.2, -29.7),
        C(49.8, -26.5, 47.9, -23.7, 45.5, -21.2),
        C(43.1, -18.7, 40.2, -16.8, 37.0, -15.4),
        C(33.8, -14.1, 30.3, -13.4, 26.6, -13.4),
        L(13.3, -13.4),
        L(0.0, 0.0),
        L(26.6, 0.0),
        C(32.1, 0.0, 37.3, -1.1, 42.1, -3.2),
        C(47.0, -5.3, 51.3, -8.2, 54.9, -11.8),
        C(58.5, -15.5, 61.3, -19.7, 63.5, -24.6),
        C(65.6, -29.4, 66.6, -34.5, 66.6, -40.0),
        C(66.6, -45.5, 65.6, -50.6, 63.5, -55.4),
        Z,
    ],
};

const GLYPH_A: AnuratiGlyph = AnuratiGlyph {
    advance: 89.9,
    visual_width: 85.0,
    ops: &[
        M(33.2, -26.6),
        L(45.0, -50.1),
        L(56.7, -26.6),
        L(71.7, -26.6),
        L(45.0, -80.0),
        L(5.0, 0.0),
        L(19.8, 0.0),
        L(26.6, -13.4),
        Z,
        M(78.3, -13.3),
        L(63.5, -13.3),
        L(70.1, 0.0),
        L(85.0, 0.0),
        Z,
    ],
};

const GLYPH_Y: AnuratiGlyph = AnuratiGlyph {
    advance: 88.8,
    visual_width: 79.5,
    ops: &[
        M(48.3, -40.0),
        L(79.5, -80.0),
        L(62.6, -80.0),
        L(39.8, -50.8),
        L(17.1, -80.0),
        L(0.0, -80.0),
        L(31.4, -40.0),
        L(0.0, 0.0),
        L(17.1, 0.0),
        L(39.8, -29.2),
        Z,
    ],
};

const GLYPH_M: AnuratiGlyph = AnuratiGlyph {
    advance: 95.4,
    visual_width: 80.1,
    ops: &[
        M(0.0, -61.4),
        L(40.1, -21.1),
        L(66.9, -47.8),
        L(66.9, 0.0),
        L(80.1, 0.0),
        L(80.1, -80.0),
        L(40.1, -40.0),
        L(0.0, -80.0),
        Z,
    ],
};

const GLYPH_O: AnuratiGlyph = AnuratiGlyph {
    advance: 96.5,
    visual_width: 80.1,
    ops: &[
        M(40.0, -80.0),
        C(28.9, -80.0, 19.5, -76.0, 11.8, -68.2),
        C(4.0, -60.4, 0.0, -51.0, 0.0, -40.0),
        C(0.0, -28.9, 4.0, -19.6, 11.8, -11.8),
        C(19.6, -4.0, 28.9, 0.0, 40.0, 0.0),
        C(51.1, 0.0, 60.4, -4.0, 68.2, -11.8),
        C(76.1, -19.5, 80.1, -28.9, 80.1, -40.0),
        C(80.1, -51.0, 76.1, -60.4, 68.2, -68.2),
        C(60.4, -76.0, 51.1, -80.0, 40.0, -80.0),
        Z,
        M(40.0, -66.6),
        C(47.3, -66.6, 53.6, -64.0, 58.8, -58.8),
        C(64.0, -53.6, 66.6, -47.3, 66.6, -40.0),
        C(66.6, -32.7, 64.1, -26.4, 58.8, -21.1),
        C(53.6, -15.9, 47.3, -13.4, 40.0, -13.4),
        C(32.7, -13.4, 26.3, -15.9, 21.1, -21.1),
        C(15.9, -26.3, 13.4, -32.7, 13.4, -40.0),
        C(13.4, -47.3, 15.9, -53.6, 21.1, -58.8),
        C(26.4, -64.1, 32.7, -66.6, 40.0, -66.6),
        Z,
    ],
};

const GLYPH_T: AnuratiGlyph = AnuratiGlyph {
    advance: 51.0,
    visual_width: 40.0,
    ops: &[
        M(0.0, -80.0),
        L(0.0, -66.6),
        L(0.0, 0.0),
        L(13.4, 0.0),
        L(13.4, -66.6),
        L(40.0, -66.6),
        L(40.0, -80.0),
        Z,
    ],
};

const GLYPH_H: AnuratiGlyph = AnuratiGlyph {
    advance: 81.9,
    visual_width: 66.6,
    ops: &[
        M(53.3, -80.0),
        L(53.3, -46.7),
        L(13.3, -46.7),
        L(0.0, -33.3),
        L(0.0, 0.0),
        L(13.3, 0.0),
        L(13.3, -33.3),
        L(53.3, -33.3),
        L(53.3, 0.0),
        L(66.6, 0.0),
        L(66.6, -80.0),
        Z,
    ],
};

const GLYPH_E: AnuratiGlyph = AnuratiGlyph {
    advance: 63.6,
    visual_width: 53.3,
    ops: &[
        M(0.0, -1.1),
        L(0.0, 0.0),
        L(53.3, 0.0),
        L(53.3, -13.4),
        L(13.3, -13.4),
        Z,
        M(0.0, -80.0),
        L(13.3, -66.6),
        L(53.3, -66.6),
        L(53.3, -80.0),
        Z,
        M(0.0, -46.6),
        L(0.0, -33.3),
        L(53.3, -33.3),
        L(53.3, -46.6),
        Z,
    ],
};

const GLYPH_R: AnuratiGlyph = AnuratiGlyph {
    advance: 76.1,
    visual_width: 61.7,
    ops: &[
        M(57.0, -38.3),
        C(60.2, -42.8, 61.7, -47.8, 61.7, -53.4),
        C(61.7, -60.7, 59.1, -66.9, 53.9, -72.2),
        C(48.7, -77.4, 42.4, -80.0, 35.1, -80.0),
        L(0.0, -80.0),
        L(6.9, -68.1),
        L(8.4, -66.6),
        L(35.1, -66.6),
        C(38.7, -66.6, 41.8, -65.3, 44.4, -62.7),
        C(47.1, -60.1, 48.4, -57.0, 48.4, -53.4),
        C(48.4, -49.7, 47.1, -46.5, 44.4, -43.9),
        C(41.8, -41.3, 38.7, -40.0, 35.1, -40.0),
        L(23.2, -40.1),
        L(46.4, 0.0),
        L(61.7, 0.0),
        L(45.3, -28.7),
        C(50.0, -30.6, 53.9, -33.8, 57.0, -38.3),
        Z,
    ],
};

const GLYPH_F: AnuratiGlyph = AnuratiGlyph {
    advance: 63.7,
    visual_width: 53.3,
    ops: &[
        M(0.0, -40.1),
        L(0.0, 0.0),
        L(13.3, 0.0),
        L(13.3, -26.6),
        L(53.3, -26.6),
        L(53.3, -40.1),
        L(13.3, -40.1),
        Z,
        M(0.0, -80.0),
        L(13.3, -66.6),
        L(53.3, -66.6),
        L(53.3, -80.0),
        Z,
    ],
};

const GLYPH_W: AnuratiGlyph = AnuratiGlyph {
    advance: 141.7,
    visual_width: 130.3,
    ops: &[
        M(14.9, -80.0),
        L(0.0, -80.0),
        L(32.6, -14.9),
        L(40.1, -29.9),
        Z,
        M(115.4, -80.0),
        L(90.3, -29.9),
        L(72.6, -65.1),
        L(65.1, -50.1),
        L(90.3, 0.0),
        L(130.3, -80.0),
        Z,
    ],
};

const GLYPH_I: AnuratiGlyph = AnuratiGlyph {
    advance: 14.0,
    visual_width: 13.3,
    ops: &[
        M(13.3, -66.8),
        L(0.0, -80.0),
        L(0.0, 0.0),
        L(13.3, -13.4),
        Z,
    ],
};

fn anurati_glyph(ch: char) -> Option<&'static AnuratiGlyph> {
    match ch {
        'A' => Some(&GLYPH_A),
        'D' => Some(&GLYPH_D),
        'E' => Some(&GLYPH_E),
        'F' => Some(&GLYPH_F),
        'H' => Some(&GLYPH_H),
        'I' => Some(&GLYPH_I),
        'M' => Some(&GLYPH_M),
        'N' => Some(&GLYPH_N),
        'O' => Some(&GLYPH_O),
        'R' => Some(&GLYPH_R),
        'S' => Some(&GLYPH_S),
        'T' => Some(&GLYPH_T),
        'U' => Some(&GLYPH_U),
        'W' => Some(&GLYPH_W),
        'Y' => Some(&GLYPH_Y),
        _ => None,
    }
}

const ANURATI_TRACKING_UNITS: f32 = 18.0;
const ANURATI_CAP_HEIGHT_UNITS: f32 = 80.0;

fn measure_anurati_word_units(word: &str) -> f32 {
    let glyphs: Vec<&AnuratiGlyph> = word.chars().filter_map(anurati_glyph).collect();
    if glyphs.is_empty() {
        return 0.0;
    }
    let mut width = 0.0f32;
    for (i, g) in glyphs.iter().enumerate() {
        if i + 1 < glyphs.len() {
            width += g.advance + ANURATI_TRACKING_UNITS;
        } else {
            width += g.visual_width;
        }
    }
    width
}

pub fn widget_pixel_size(day_str: &str, size_pct: u32, dpi: i32) -> (i32, i32) {
    let scale = (size_pct.clamp(50, 200) as f32 / 100.0) * (dpi.max(96) as f32 / 96.0);
    let cap_h = 50.0 * scale;
    let em_scale = cap_h / ANURATI_CAP_HEIGHT_UNITS;
    let day_px = measure_anurati_word_units(day_str) * em_scale;
    let width = (day_px + 56.0 * scale).ceil().max(360.0 * scale) as i32;
    let height = (176.0 * scale).ceil() as i32;
    (width.clamp(240, 2400), height.clamp(120, 900))
}

fn target_monitor_info(hwnd: HWND, owner: HWND, pos: Option<(i32, i32)>) -> (RECT, i32) {
    let mon = if let Some((x, y)) = pos {
        unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) }
    } else if !owner.0.is_null() && unsafe { IsWindow(owner).as_bool() } {
        unsafe { MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST) }
    } else {
        unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }
    };
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let work = if unsafe { GetMonitorInfoW(mon, &mut mi) }.as_bool() {
        mi.rcWork
    } else {
        RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }
    };
    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    let dpi = if unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }.is_ok()
    {
        dpi_x as i32
    } else if !owner.0.is_null() {
        unsafe { GetDpiForWindow(owner) }.max(96) as i32
    } else {
        96
    };
    (work, dpi.max(96))
}

fn resolve_screen_position(pos: Option<(i32, i32)>, work: RECT, w: i32, h: i32) -> (i32, i32) {
    let (raw_x, raw_y) = match pos {
        Some((x, y)) => (x, y),
        None => {
            let cx = work.left + (work.right - work.left - w) / 2;
            let cy = work.top + ((work.bottom - work.top) / 4 - h / 2).max(24);
            (cx, cy)
        }
    };
    let max_x = (work.right - w).max(work.left);
    let max_y = (work.bottom - h).max(work.top);
    (raw_x.clamp(work.left, max_x), raw_y.clamp(work.top, max_y))
}

fn premul_blend_pixel(dst: &mut u32, r: u32, g: u32, b: u32, a: u8) {
    if a == 0 {
        return;
    }
    let a_u = u32::from(a);
    let src_r = (r * a_u + 127) / 255;
    let src_g = (g * a_u + 127) / 255;
    let src_b = (b * a_u + 127) / 255;
    let inv = 255 - a_u;
    let cur_a = (*dst >> 24) & 255;
    let cur_r = (*dst >> 16) & 255;
    let cur_g = (*dst >> 8) & 255;
    let cur_b = *dst & 255;
    let out_a = (a_u + (cur_a * inv + 127) / 255).min(255);
    let out_r = (src_r + (cur_r * inv + 127) / 255).min(out_a);
    let out_g = (src_g + (cur_g * inv + 127) / 255).min(out_a);
    let out_b = (src_b + (cur_b * inv + 127) / 255).min(out_a);
    *dst = (out_a << 24) | (out_r << 16) | (out_g << 8) | out_b;
}

fn collect_glyph_edges(
    ops: &[AnuratiSeg],
    origin_x: f32,
    origin_y: f32,
    em_scale: f32,
    edges: &mut Vec<(f32, f32, f32, f32)>,
) {
    edges.clear();
    let map_pt = |gx: f32, gy: f32| -> (f32, f32) {
        (
            origin_x + gx * em_scale,
            origin_y + (gy + ANURATI_CAP_HEIGHT_UNITS) * em_scale,
        )
    };
    let push_edge = |edges: &mut Vec<(f32, f32, f32, f32)>, a: (f32, f32), b: (f32, f32)| {
        if (a.1 - b.1).abs() > 1e-5 {
            edges.push((a.0, a.1, b.0, b.1));
        }
    };
    let mut start = (0.0f32, 0.0f32);
    let mut cur = (0.0f32, 0.0f32);
    let mut has_open = false;

    for &op in ops {
        match op {
            M(x, y) => {
                if has_open {
                    push_edge(edges, cur, start);
                }
                start = map_pt(x, y);
                cur = start;
                has_open = true;
            }
            L(x, y) => {
                let next = map_pt(x, y);
                push_edge(edges, cur, next);
                cur = next;
            }
            C(cx1, cy1, cx2, cy2, x, y) => {
                let p0 = cur;
                let p1 = map_pt(cx1, cy1);
                let p2 = map_pt(cx2, cy2);
                let p3 = map_pt(x, y);
                let steps = 8;
                let mut prev = p0;
                for step in 1..=steps {
                    let t = step as f32 / steps as f32;
                    let mt = 1.0 - t;
                    let mt2 = mt * mt;
                    let t2 = t * t;
                    let nx = mt2 * mt * p0.0
                        + 3.0 * mt2 * t * p1.0
                        + 3.0 * mt * t2 * p2.0
                        + t2 * t * p3.0;
                    let ny = mt2 * mt * p0.1
                        + 3.0 * mt2 * t * p1.1
                        + 3.0 * mt * t2 * p2.1
                        + t2 * t * p3.1;
                    push_edge(edges, prev, (nx, ny));
                    prev = (nx, ny);
                }
                cur = p3;
            }
            Z => {
                if has_open {
                    push_edge(edges, cur, start);
                    cur = start;
                    has_open = false;
                }
            }
        }
    }
    if has_open {
        push_edge(edges, cur, start);
    }
}

pub struct GlyphCanvas<'a> {
    pub buf: &'a mut [u32],
    pub width: i32,
    pub height: i32,
}

pub fn render_anurati_word(
    canvas: &mut GlyphCanvas<'_>,
    text: &str,
    top_y: f32,
    cap_height_px: f32,
    center_x: f32,
    color: u32,
) {
    let stride_px = canvas.width.max(0) as usize;
    let buf_h = canvas.height;
    if stride_px == 0 || buf_h <= 0 || cap_height_px <= 0.0 {
        return;
    }
    let total_units = measure_anurati_word_units(text);
    if total_units <= 0.0 {
        return;
    }
    let em_scale = cap_height_px / ANURATI_CAP_HEIGHT_UNITS;
    let mut pen_x = center_x - total_units * em_scale * 0.5;
    let r = (color >> 16) & 255;
    let g = (color >> 8) & 255;
    let b = color & 255;
    let sh_dx = (1.5 * (cap_height_px / 50.0)).round().clamp(1.0, 3.0) as i32;
    let sh_dy = (2.0 * (cap_height_px / 50.0)).round().clamp(1.0, 4.0) as i32;

    let mut edges = Vec::with_capacity(64);
    let mut crossings = Vec::with_capacity(16);
    let mut cov_grid = Vec::new();
    let mut hits = Vec::new();

    for ch in text.chars() {
        let Some(glyph) = anurati_glyph(ch) else {
            continue;
        };
        collect_glyph_edges(glyph.ops, pen_x, top_y, em_scale, &mut edges);
        pen_x += (glyph.advance + ANURATI_TRACKING_UNITS) * em_scale;
        if edges.is_empty() {
            continue;
        }

        let mut min_x = f32::MAX;
        let mut max_x = f32::MIN;
        let mut min_y = f32::MAX;
        let mut max_y = f32::MIN;
        for &(ax, ay, bx, by) in &edges {
            min_x = min_x.min(ax).min(bx);
            max_x = max_x.max(ax).max(bx);
            min_y = min_y.min(ay).min(by);
            max_y = max_y.max(ay).max(by);
        }

        let x0 = (min_x.floor() as i32 - 1).max(0);
        let x1 = (max_x.ceil() as i32 + 1).min(stride_px as i32);
        let y0 = (min_y.floor() as i32 - 1).max(0);
        let y1 = (max_y.ceil() as i32 + 1).min(buf_h);
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        let bw = (x1 - x0) as usize;
        let bh = (y1 - y0) as usize;
        cov_grid.clear();
        cov_grid.resize(bw * bh, 0);
        hits.clear();
        hits.resize(bw, 0u8);

        for row_idx in 0..bh {
            let py = y0 + row_idx as i32;
            hits.fill(0);
            for sub_y in 0..4 {
                let sy = py as f32 + (sub_y as f32 + 0.5) * 0.25;
                crossings.clear();
                for &(ax, ay, bx, by) in &edges {
                    let (lo, hi) = if ay < by { (ay, by) } else { (by, ay) };
                    if lo <= sy && sy < hi {
                        let ix = ax + (sy - ay) * (bx - ax) / (by - ay);
                        crossings.push(ix);
                    }
                }
                crossings.sort_unstable_by(f32::total_cmp);
                for &[xa, xb] in crossings.as_chunks::<2>().0 {
                    let cx_start = ((xa - 0.125).floor() as i32).max(x0);
                    let cx_end = ((xb + 0.125).ceil() as i32).min(x1);
                    for px in cx_start..cx_end {
                        let col = (px - x0) as usize;
                        let mut h_count = 0u8;
                        for sub_x in 0..4 {
                            let sx = px as f32 + (sub_x as f32 + 0.5) * 0.25;
                            if sx >= xa && sx < xb {
                                h_count += 1;
                            }
                        }
                        hits[col] = hits[col].saturating_add(h_count);
                    }
                }
            }
            let out_row = &mut cov_grid[row_idx * bw..(row_idx + 1) * bw];
            for col in 0..bw {
                if hits[col] > 0 {
                    out_row[col] = ((u16::from(hits[col]) * 255 + 8) / 16).min(255) as u8;
                }
            }
        }

        // First composite soft drop shadow, then foreground so no glyph casts shadow onto itself.
        for row_idx in 0..bh {
            let py = y0 + row_idx as i32 + sh_dy;
            if py < 0 || py >= buf_h {
                continue;
            }
            let dst_row = py as usize * stride_px;
            let src_row = &cov_grid[row_idx * bw..(row_idx + 1) * bw];
            for (col, &cov) in src_row.iter().enumerate().take(bw) {
                if cov == 0 {
                    continue;
                }
                let px = x0 + col as i32 + sh_dx;
                if px >= 0 && (px as usize) < stride_px {
                    let sh_a = ((u16::from(cov) * 76) / 255) as u8;
                    premul_blend_pixel(&mut canvas.buf[dst_row + px as usize], 0, 0, 0, sh_a);
                }
            }
        }
        for row_idx in 0..bh {
            let py = y0 + row_idx as i32;
            let dst_row = py as usize * stride_px;
            let src_row = &cov_grid[row_idx * bw..(row_idx + 1) * bw];
            for (col, &cov) in src_row.iter().enumerate().take(bw) {
                if cov == 0 {
                    continue;
                }
                let px = x0 + col as i32;
                premul_blend_pixel(&mut canvas.buf[dst_row + px as usize], r, g, b, cov);
            }
        }
    }
}

fn draw_quicksand_line(
    canvas: &mut GlyphCanvas<'_>,
    text: &str,
    area: RECT,
    font_px: i32,
    char_extra: i32,
    color: u32,
) {
    let buf_w = canvas.width;
    let buf_h = canvas.height;
    let rw = (area.right - area.left).max(0);
    let rh = (area.bottom - area.top).max(0);
    if rw <= 0 || rh <= 0 || buf_w <= 0 || buf_h <= 0 || text.is_empty() {
        return;
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return;
    }

    CLOCK_TEXT_SCRATCH.with(|slot| {
        let mut scratch = slot.borrow_mut();
        let Some((dc, bits_ptr)) = scratch.ensure(rw, rh) else {
            return;
        };
        let total = rw as usize * rh as usize;
        // SAFETY: scratch DIB has rw * rh 32-bit pixels owned by this thread.
        unsafe {
            std::slice::from_raw_parts_mut(bits_ptr, total).fill(0);
            let font = CreateFontW(
                -font_px.max(10),
                0,
                0,
                0,
                FW_BOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                ANTIALIASED_QUALITY.0 as u32,
                (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                w!("Quicksand"),
            );
            let prev_font = (!font.0.is_null()).then(|| SelectObject(dc, HGDIOBJ(font.0)));
            let prev_extra = SetTextCharacterExtra(dc, char_extra.max(0));
            let _ = SetBkMode(dc, TRANSPARENT);
            let _ = SetTextColor(dc, COLORREF(0x00FF_FFFF));
            let mut local_rect = RECT {
                left: 0,
                top: 0,
                right: rw,
                bottom: rh,
            };
            let mut wide_mut = wide;
            let _ = DrawTextW(
                dc,
                &mut wide_mut,
                &mut local_rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            let _ = GdiFlush();
            SetTextCharacterExtra(dc, prev_extra);
            if let Some(prev) = prev_font {
                let _ = SelectObject(dc, prev);
            }
            if !font.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(font.0));
            }
        }

        // SAFETY: scratch DIB pixels remain valid after GdiFlush.
        let src = unsafe { std::slice::from_raw_parts(bits_ptr, total) };
        let r = (color >> 16) & 255;
        let g = (color >> 8) & 255;
        let b = color & 255;
        let stride = buf_w as usize;

        // Pass 1: subtle drop shadow (+1px, +2px).
        for sy in 0..rh {
            let dy = area.top + sy + 2;
            if dy < 0 || dy >= buf_h {
                continue;
            }
            let src_row = sy as usize * rw as usize;
            let dst_row = dy as usize * stride;
            for sx in 0..rw {
                let cov = (src[src_row + sx as usize] >> 16) & 255;
                if cov == 0 {
                    continue;
                }
                let dx = area.left + sx + 1;
                if dx >= 0 && dx < buf_w {
                    let sh_a = ((cov * 84) / 255) as u8;
                    premul_blend_pixel(&mut canvas.buf[dst_row + dx as usize], 0, 0, 0, sh_a);
                }
            }
        }
        // Pass 2: crisp antialiased Quicksand text.
        for sy in 0..rh {
            let dy = area.top + sy;
            if dy < 0 || dy >= buf_h {
                continue;
            }
            let src_row = sy as usize * rw as usize;
            let dst_row = dy as usize * stride;
            for sx in 0..rw {
                let cov = (src[src_row + sx as usize] >> 16) & 255;
                if cov == 0 {
                    continue;
                }
                let dx = area.left + sx;
                if dx >= 0 && dx < buf_w {
                    premul_blend_pixel(&mut canvas.buf[dst_row + dx as usize], r, g, b, cov as u8);
                }
            }
        }
    });
}

fn draw_reposition_guide(buf: &mut [u32], w: i32, h: i32) {
    if w <= 8 || h <= 8 {
        return;
    }
    let corner = 18.min(w / 4).min(h / 4);
    let col = 0xD8F3_EEE3u32;
    let wu = w as usize;
    for t in 0..2 {
        for x in 0..corner {
            buf[t * wu + x as usize] = col;
            buf[t * wu + (w - 1 - x) as usize] = col;
            buf[(h as usize - 1 - t) * wu + x as usize] = col;
            buf[(h as usize - 1 - t) * wu + (w - 1 - x) as usize] = col;
        }
        for y in 0..corner {
            buf[y as usize * wu + t] = col;
            buf[y as usize * wu + (w as usize - 1 - t)] = col;
            buf[(h - 1 - y) as usize * wu + t] = col;
            buf[(h - 1 - y) as usize * wu + (w as usize - 1 - t)] = col;
        }
    }
}

fn apply_opacity_in_place(buf: &mut [u32], opacity_pct: u32) {
    let pct = opacity_pct.clamp(20, 100);
    if pct >= 100 {
        return;
    }
    let scale = (pct * 255 + 50) / 100;
    for px in buf {
        let a = (*px >> 24) & 255;
        if a == 0 {
            continue;
        }
        let r = (*px >> 16) & 255;
        let g = (*px >> 8) & 255;
        let b = *px & 255;
        let na = (a * scale + 127) / 255;
        let nr = ((r * scale + 127) / 255).min(na);
        let ng = ((g * scale + 127) / 255).min(na);
        let nb = ((b * scale + 127) / 255).min(na);
        *px = (na << 24) | (nr << 16) | (ng << 8) | nb;
    }
}

pub fn apply_hit_alpha_in_place(buf: &mut [u32], interactive: bool) {
    if !interactive {
        return;
    }
    // Rainmeter SolidColor=0,0,0,1 equivalent: invisible alpha=1 lets users click/drag gaps between thin glyphs.
    for px in buf {
        if (*px >> 24) == 0 {
            *px = 0x0100_0000;
        }
    }
}

fn render_and_place(hwnd: HWND) {
    let snapshot = CLOCK_RUNTIME.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|r| (r.owner, r.cfg, r.repositioning, r.last_bounds))
    });
    let Some((owner_raw, cfg, repositioning, last_bounds)) = snapshot else {
        return;
    };
    let owner = HWND(owner_raw as *mut c_void);
    let (work, dpi) = target_monitor_info(hwnd, owner, cfg.pos);
    let now = unsafe { local_time() };
    let (day_str, date_str, time_str) = format_clock_lines(
        now.wYear,
        now.wMonth,
        now.wDayOfWeek,
        now.wDay,
        now.wHour,
        now.wMinute,
        cfg.use_24h,
    );
    let (w, h) = widget_pixel_size(&day_str, cfg.size_pct, dpi);
    let (sx, sy) = resolve_screen_position(cfg.pos, work, w, h);

    // SAFETY: owns offscreen memory DC and 32-bit DIBSection strictly for this synchronous UpdateLayeredWindow call.
    unsafe {
        let mem_dc = CreateCompatibleDC(None);
        if mem_dc.0.is_null() {
            return;
        }
        let mut bits = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(
            mem_dc,
            &BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
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
        ) else {
            let _ = DeleteDC(mem_dc);
            return;
        };
        if !bits.is_null() {
            let prev_bmp = SelectObject(mem_dc, HGDIOBJ(bmp.0));
            let total = (w as usize).saturating_mul(h as usize);
            let buf = std::slice::from_raw_parts_mut(bits as *mut u32, total);
            buf.fill(0);

            let scale = (cfg.size_pct.clamp(50, 200) as f32 / 100.0) * (dpi as f32 / 96.0);
            let mut canvas = GlyphCanvas {
                buf,
                width: w,
                height: h,
            };
            render_anurati_word(
                &mut canvas,
                &day_str,
                16.0 * scale,
                50.0 * scale,
                w as f32 * 0.5,
                cfg.color,
            );
            draw_quicksand_line(
                &mut canvas,
                &date_str,
                RECT {
                    left: 8,
                    top: (84.0 * scale).round() as i32,
                    right: w - 8,
                    bottom: (118.0 * scale).round() as i32,
                },
                (21.0 * scale).round() as i32,
                (1.5 * scale).round() as i32,
                cfg.color,
            );
            draw_quicksand_line(
                &mut canvas,
                &time_str,
                RECT {
                    left: 8,
                    top: (126.0 * scale).round() as i32,
                    right: w - 8,
                    bottom: (160.0 * scale).round() as i32,
                },
                (20.0 * scale).round() as i32,
                (1.2 * scale).round() as i32,
                cfg.color,
            );
            apply_opacity_in_place(buf, cfg.opacity_pct);
            apply_hit_alpha_in_place(buf, !is_click_through(cfg, repositioning));
            if repositioning {
                draw_reposition_guide(buf, w, h);
            }

            let dst = POINT { x: sx, y: sy };
            let size = SIZE { cx: w, cy: h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let next_bounds = (dst.x, dst.y, w, h, repositioning);
            if last_bounds != Some(next_bounds) || !IsWindowVisible(hwnd).as_bool() {
                place_on_desktop(hwnd, dst.x, dst.y, w, h, true, repositioning);
            }
            let _ = UpdateLayeredWindow(
                hwnd,
                None,
                Some(&dst),
                Some(&size),
                mem_dc,
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            SelectObject(mem_dc, prev_bmp);
            CLOCK_RUNTIME.with(|slot| {
                if let Some(state) = slot.borrow_mut().as_mut() {
                    state.last_bounds = Some(next_bounds);
                    state.last_stamp = (
                        now.wYear,
                        now.wMonth,
                        now.wDayOfWeek,
                        now.wDay,
                        now.wHour,
                        now.wMinute,
                    );
                }
            });
            let _ = SetTimer(
                hwnd,
                CLOCK_TIMER_ID,
                minute_timer_delay_ms(now.wSecond),
                None,
            );
        }
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem_dc);
    }
}

fn show_context_menu(hwnd: HWND, screen_pt: POINT) {
    let (owner, action_msg, cfg, repositioning) = match CLOCK_RUNTIME.with(|s| {
        s.borrow().as_ref().map(|r| {
            (
                HWND(r.owner as *mut c_void),
                r.action_msg,
                r.cfg,
                r.repositioning,
            )
        })
    }) {
        Some(v) => v,
        None => return,
    };
    let lock_label = if cfg.locked {
        "Lock position (On)"
    } else {
        "Lock position (Off)"
    };
    let icons_label = if cfg.behind_icons {
        "Pass clicks to shortcuts (On)"
    } else {
        "Pass clicks to shortcuts (Off)"
    };
    let fmt_label = if cfg.use_24h {
        "24-hour clock (On)"
    } else {
        "12-hour clock (AM/PM)"
    };
    let mut items: Vec<(usize, &str)> = vec![
        (CLOCK_CTX_SIZE_DELTA + 1, "Bigger\tScroll up"),
        (CLOCK_CTX_SIZE_DELTA + 2, "Smaller\tScroll down"),
        (CLOCK_CTX_24H, fmt_label),
        (CLOCK_CTX_LOCK, lock_label),
        (CLOCK_CTX_BEHIND_ICONS, icons_label),
    ];
    if repositioning {
        items.push((CTX_LOCAL_DONE_REPOS, "Done positioning"));
    }
    items.push((CLOCK_CTX_OPEN_SETTINGS, "Widget settings"));
    items.push((CLOCK_CTX_REMOVE, "Remove from desktop"));

    let target_owner = if !owner.0.is_null()
        && unsafe { IsWindow(owner).as_bool() && IsWindowVisible(owner).as_bool() }
    {
        owner
    } else {
        hwnd
    };
    if let Ok(choice) = tray::popup_at(target_owner, screen_pt, &items, None)
        && choice != 0
    {
        if choice == CTX_LOCAL_DONE_REPOS {
            if !owner.0.is_null() {
                unsafe {
                    let _ = PostMessageW(owner, action_msg, WPARAM(CLOCK_REPOSITION), LPARAM(0));
                }
            } else {
                end_reposition(owner, action_msg, cfg);
            }
            return;
        }
        if owner.0.is_null() {
            return;
        }
        if choice == CLOCK_CTX_SIZE_DELTA + 1 {
            CLOCK_WHEEL_STEPS.with(|s| *s.borrow_mut() += 2);
            unsafe {
                let _ = PostMessageW(owner, action_msg, WPARAM(CLOCK_CTX_SIZE_DELTA), LPARAM(0));
            }
        } else if choice == CLOCK_CTX_SIZE_DELTA + 2 {
            CLOCK_WHEEL_STEPS.with(|s| *s.borrow_mut() -= 2);
            unsafe {
                let _ = PostMessageW(owner, action_msg, WPARAM(CLOCK_CTX_SIZE_DELTA), LPARAM(0));
            }
        } else {
            unsafe {
                let _ = PostMessageW(owner, action_msg, WPARAM(choice), LPARAM(0));
            }
        }
    }
}

unsafe extern "system" fn clock_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: interacts strictly with thread-local CLOCK_RUNTIME on the UI thread.
    unsafe {
        match msg {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_SYSCOMMAND => {
                let cmd = (wparam.0 & 0xFFF0) as u32;
                if cmd == SC_MINIMIZE || cmd == SC_MAXIMIZE {
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_SIZE if wparam.0 == SIZE_MINIMIZED as usize => {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                ensure_desktop_visible();
                LRESULT(0)
            }
            WM_WINDOWPOSCHANGING => {
                if lparam.0 != 0 {
                    let pos = &mut *(lparam.0 as *mut WINDOWPOS);
                    if !ALLOW_INTERNAL_HIDE.with(Cell::get) {
                        pos.flags.0 &= !SWP_HIDEWINDOW.0;
                    }
                    if !IN_ZORDER_SYNC.with(Cell::get)
                        && (pos.flags.0 & SWP_NOZORDER.0 == 0)
                        && !is_repositioning()
                        && pos.hwndInsertAfter == HWND_BOTTOM
                    {
                        pos.hwndInsertAfter = desktop_insert_after(hwnd, false);
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_NCHITTEST => {
                let pass = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .is_none_or(|rt| is_click_through(rt.cfg, rt.repositioning))
                });
                if pass {
                    LRESULT(HTTRANSPARENT as isize)
                } else {
                    LRESULT(HTCLIENT as isize)
                }
            }
            WM_SETCURSOR => {
                let interactive = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .is_some_and(|rt| !is_click_through(rt.cfg, rt.repositioning))
                });
                let cursor = if interactive { IDC_SIZEALL } else { IDC_ARROW };
                if let Ok(h) = LoadCursorW(None, cursor) {
                    SetCursor(h);
                }
                LRESULT(1)
            }
            WM_TIMER if wparam.0 == CLOCK_ZORDER_SETTLE_TIMER_ID => {
                let _ = KillTimer(hwnd, CLOCK_ZORDER_SETTLE_TIMER_ID);
                ensure_desktop_visible();
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == CLOCK_TIMER_ID => {
                let now = local_time();
                let changed = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow().as_ref().is_some_and(|rt| {
                        rt.last_stamp
                            != (
                                now.wYear,
                                now.wMonth,
                                now.wDayOfWeek,
                                now.wDay,
                                now.wHour,
                                now.wMinute,
                            )
                    })
                });
                if changed {
                    render_and_place(hwnd);
                } else {
                    let _ = SetTimer(
                        hwnd,
                        CLOCK_TIMER_ID,
                        minute_timer_delay_ms(now.wSecond),
                        None,
                    );
                }
                LRESULT(0)
            }
            WM_NCDESTROY => {
                let hooks = CLOCK_RUNTIME.with(|slot| {
                    let mut borrow = slot.borrow_mut();
                    if borrow.as_ref().is_some_and(|rt| rt.hwnd == hwnd.0 as isize) {
                        borrow.take().map_or([0, 0], |rt| rt.hooks)
                    } else {
                        [0, 0]
                    }
                });
                unhook_events(hooks);
                CLOCK_TEXT_SCRATCH.with(|s| s.borrow_mut().free());
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_LBUTTONDOWN => {
                let interactive = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .is_some_and(|rt| !is_click_through(rt.cfg, rt.repositioning))
                });
                if !interactive {
                    return LRESULT(0);
                }
                let mut cur = POINT::default();
                let _ = GetCursorPos(&mut cur);
                let mut wr = RECT::default();
                let _ = GetWindowRect(hwnd, &mut wr);
                CLOCK_RUNTIME.with(|slot| {
                    if let Some(rt) = slot.borrow_mut().as_mut() {
                        rt.dragging = true;
                        rt.drag_moved = false;
                        rt.drag_start_x = cur.x;
                        rt.drag_start_y = cur.y;
                        rt.drag_offset_x = cur.x - wr.left;
                        rt.drag_offset_y = cur.y - wr.top;
                    }
                });
                let _ = SetCapture(hwnd);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let drag = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow().as_ref().filter(|rt| rt.dragging).map(|rt| {
                        (
                            rt.drag_start_x,
                            rt.drag_start_y,
                            rt.drag_offset_x,
                            rt.drag_offset_y,
                        )
                    })
                });
                if let Some((sx, sy, ox, oy)) = drag {
                    let mut cur = POINT::default();
                    let _ = GetCursorPos(&mut cur);
                    if (cur.x - sx).abs() > 2 || (cur.y - sy).abs() > 2 {
                        CLOCK_RUNTIME.with(|slot| {
                            if let Some(rt) = slot.borrow_mut().as_mut() {
                                rt.drag_moved = true;
                            }
                        });
                    }
                    let nx = cur.x - ox;
                    let ny = cur.y - oy;
                    let _ = SetWindowPos(
                        hwnd,
                        HWND::default(),
                        nx,
                        ny,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                // Clicks never collapse or hide the widget; only explicit Done/Esc exits repositioning.
                let (moved, owner, action_msg) = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow_mut().as_mut().map_or((false, 0, 0), |rt| {
                        let m = rt.dragging && rt.drag_moved;
                        rt.dragging = false;
                        rt.drag_moved = false;
                        (m, rt.owner, rt.action_msg)
                    })
                });
                if GetCapture() == hwnd {
                    let _ = ReleaseCapture();
                }
                if moved {
                    let mut wr = RECT::default();
                    if GetWindowRect(hwnd, &mut wr).is_ok() {
                        CLOCK_SAVED_POS.with(|s| *s.borrow_mut() = Some((wr.left, wr.top)));
                        if owner != 0 {
                            let _ = PostMessageW(
                                HWND(owner as *mut c_void),
                                action_msg,
                                WPARAM(CLOCK_CTX_POS_SAVED),
                                LPARAM(0),
                            );
                        }
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam.0 >> 16) as i16) as i32;
                let (allowed, owner, action_msg) = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow_mut().as_mut().map_or((false, 0, 0), |rt| {
                        let ok = !is_click_through(rt.cfg, rt.repositioning);
                        if ok {
                            rt.pending_wheel_delta += delta;
                            let steps = rt.pending_wheel_delta / 120;
                            rt.pending_wheel_delta %= 120;
                            if steps != 0 {
                                CLOCK_WHEEL_STEPS.with(|s| *s.borrow_mut() += steps);
                                return (true, rt.owner, rt.action_msg);
                            }
                        }
                        (false, 0, 0)
                    })
                });
                if allowed && owner != 0 {
                    let _ = PostMessageW(
                        HWND(owner as *mut c_void),
                        action_msg,
                        WPARAM(CLOCK_CTX_SIZE_DELTA),
                        LPARAM(0),
                    );
                }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                let mut cur = POINT::default();
                let _ = GetCursorPos(&mut cur);
                show_context_menu(hwnd, cur);
                LRESULT(0)
            }
            WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => {
                let info = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .filter(|rt| rt.repositioning)
                        .map(|rt| (rt.owner, rt.action_msg, rt.cfg))
                });
                if let Some((owner, action_msg, cfg)) = info {
                    if owner != 0 {
                        let _ = PostMessageW(
                            HWND(owner as *mut c_void),
                            action_msg,
                            WPARAM(CLOCK_REPOSITION),
                            LPARAM(0),
                        );
                    } else {
                        end_reposition(HWND::default(), action_msg, cfg);
                    }
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mond_clock_formatting_anurati_raster_and_hit_policy() {
        let (day, date, t24) = format_clock_lines(2026, 11, 3, 18, 13, 9, true);
        assert_eq!(day, "WEDNESDAY");
        assert_eq!(date, "18  NOVEMBER,  2026.");
        assert_eq!(t24, "- 13:09 -");

        let (_, date_pad, t12) = format_clock_lines(2026, 6, 0, 5, 0, 5, false);
        assert_eq!(date_pad, "05  JUNE,  2026.");
        assert_eq!(t12, "- 12:05 AM -");
        let (_, _, t12_pm) = format_clock_lines(2026, 6, 5, 5, 15, 45, false);
        assert_eq!(t12_pm, "- 3:45 PM -");

        // Every weekday letter has an exact Anurati vector contour and positive advance.
        for dow in 0..=6 {
            let name = weekday_name(dow);
            assert!(
                name.chars()
                    .all(|c| anurati_glyph(c).is_some_and(|g| !g.ops.is_empty() && g.advance > 0.0))
            );
        }

        // Exact Mond Anurati rasterization produces solid anti-aliased foreground + shadow.
        let (w, h) = widget_pixel_size("WEDNESDAY", 100, 96);
        let mut buf = vec![0u32; (w * h) as usize];
        {
            let mut canvas = GlyphCanvas {
                buf: &mut buf,
                width: w,
                height: h,
            };
            render_anurati_word(
                &mut canvas,
                "WEDNESDAY",
                16.0,
                50.0,
                w as f32 * 0.5,
                0x00FF_FFFF,
            );
        }
        assert!(buf.iter().any(|&px| (px >> 24) == 255));
        apply_opacity_in_place(&mut buf, 50);
        let max_alpha = buf.iter().map(|&px| px >> 24).max().unwrap_or(0);
        assert!((120..=135).contains(&max_alpha));

        // Rainmeter SolidColor=0,0,0,1 invariant: interactive surface has alpha>=1 everywhere, click-through stays 0.
        let mut hit_buf = vec![0u32; 64];
        apply_hit_alpha_in_place(&mut hit_buf, false);
        assert!(hit_buf.iter().all(|&px| px == 0));
        apply_hit_alpha_in_place(&mut hit_buf, true);
        assert!(hit_buf.iter().all(|&px| px == 0x0100_0000));

        assert!(is_click_through(
            ClockConfig {
                enabled: true,
                locked: false,
                behind_icons: true,
                ..Default::default()
            },
            false,
        ));
        assert!(!is_click_through(
            ClockConfig {
                enabled: true,
                locked: true,
                behind_icons: true,
                ..Default::default()
            },
            true,
        ));

        assert_eq!(minute_timer_delay_ms(0), 60_050);
        assert_eq!(minute_timer_delay_ms(59), 1_050);
        assert_eq!(minute_timer_delay_ms(65), 1_050);
    }
}
