use crate::{
    tray,
    visuals::{
        CLOCK_CTX_24H, CLOCK_CTX_BEHIND_ICONS, CLOCK_CTX_LOCK, CLOCK_CTX_OPEN_SETTINGS,
        CLOCK_CTX_POS_SAVED, CLOCK_CTX_REMOVE, CLOCK_CTX_SIZE_DELTA,
    },
};
use std::{cell::RefCell, f32::consts::PI, ffi::c_void, mem::size_of};
use windows::{
    Win32::{
        Foundation::{
            BOOL, COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, SYSTEMTIME, WPARAM,
        },
        Graphics::Gdi::{
            AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
            CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW,
            DEFAULT_CHARSET, DEFAULT_PITCH, DEFAULT_QUALITY, DIB_RGB_COLORS, DT_CENTER,
            DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, FF_DONTCARE,
            FW_SEMIBOLD, GdiFlush, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO,
            MapWindowPoints, MonitorFromPoint, MonitorFromWindow, OUT_DEFAULT_PRECIS, SelectObject,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{
                CloseThemeData, DTT_COMPOSITED, DTT_TEXTCOLOR, DTTOPTS, DrawThemeTextEx, HTHEME,
                OpenThemeData,
            },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DesktopLayer {
    parent: HWND,
    icons: HWND,
    raised: bool,
}

#[derive(Default)]
struct DesktopSearch {
    worker: HWND,
    icons: HWND,
}

unsafe extern "system" fn search_desktop_cb(window: HWND, data: LPARAM) -> BOOL {
    // SAFETY: synchronous EnumWindows callback reading live Explorer hierarchy.
    unsafe {
        if let Ok(icons) = FindWindowExW(window, None, w!("SHELLDLL_DefView"), PCWSTR::null()) {
            let out = &mut *(data.0 as *mut DesktopSearch);
            out.icons = icons;
            if let Ok(worker) = FindWindowExW(None, window, w!("WorkerW"), PCWSTR::null()) {
                out.worker = worker;
            }
        }
    }
    BOOL(1)
}

fn discover_desktop_layer() -> Option<DesktopLayer> {
    // SAFETY: Win32 read-only discovery on Explorer's Progman/WorkerW hierarchy.
    unsafe {
        let progman = FindWindowW(w!("Progman"), PCWSTR::null()).ok()?;
        let raised =
            GetWindowLongPtrW(progman, GWL_EXSTYLE) as u32 & WS_EX_NOREDIRECTIONBITMAP.0 != 0;
        let mut msg_res = 0;
        let _ = SendMessageTimeoutW(
            progman,
            0x052c,
            WPARAM(0xD),
            LPARAM(1),
            SMTO_ABORTIFHUNG,
            1000,
            Some(&mut msg_res),
        );
        let mut search = DesktopSearch::default();
        let _ = EnumWindows(
            Some(search_desktop_cb),
            LPARAM((&mut search as *mut DesktopSearch) as isize),
        );
        let (parent, icons) = if raised {
            let icons =
                FindWindowExW(progman, None, w!("SHELLDLL_DefView"), PCWSTR::null()).ok()?;
            (progman, icons)
        } else {
            (search.worker, search.icons)
        };
        if parent.0.is_null() || icons.0.is_null() || !IsWindow(parent).as_bool() {
            return None;
        }
        Some(DesktopLayer {
            parent,
            icons,
            raised,
        })
    }
}

#[derive(Clone, Default)]
struct ClockRuntime {
    hwnd: isize,
    owner: isize,
    action_msg: u32,
    cfg: ClockConfig,
    behind_attached: bool,
    repositioning: bool,
    dragging: bool,
    drag_offset_x: i32,
    drag_offset_y: i32,
    last_stamp: (u16, u16, u16, u16, u16, u16), // (year, month, dow, day, hour, minute)
    last_bounds: Option<(i32, i32, i32, i32, bool)>,
    cached_layer: Option<DesktopLayer>,
    theme: isize,
    pending_wheel_delta: i32,
}

fn minute_timer_delay_ms(second: u16) -> u32 {
    // Wait until right after the next minute boundary so idle CPU stays near zero.
    (60u32.saturating_sub(second.min(59) as u32)).clamp(1, 60) * 1000 + 50
}

fn layer_still_valid(layer: DesktopLayer) -> bool {
    !layer.parent.0.is_null()
        && !layer.icons.0.is_null()
        && unsafe { IsWindow(layer.parent).as_bool() && IsWindow(layer.icons).as_bool() }
}

thread_local! {
    static CLOCK_RUNTIME: RefCell<Option<ClockRuntime>> = const { RefCell::new(None) };
    static CLOCK_SAVED_POS: RefCell<Option<(i32, i32)>> = const { RefCell::new(None) };
    static CLOCK_WHEEL_STEPS: RefCell<i32> = const { RefCell::new(0) };
}

pub fn take_saved_position() -> Option<(i32, i32)> {
    CLOCK_SAVED_POS.with(|slot| slot.borrow_mut().take())
}

pub fn take_wheel_steps() -> i32 {
    CLOCK_WHEEL_STEPS.with(|slot| std::mem::take(&mut *slot.borrow_mut()))
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

fn destroy_clock_hwnd(raw: isize, theme: isize) {
    if theme != 0 {
        // SAFETY: theme opened via OpenThemeData for this clock HWND.
        unsafe {
            let _ = CloseThemeData(HTHEME(theme));
        }
    }
    if raw == 0 {
        return;
    }
    let hwnd = HWND(raw as *mut c_void);
    // SAFETY: owns this single HWND on UI thread.
    unsafe {
        if IsWindow(hwnd).as_bool() {
            let _ = KillTimer(hwnd, CLOCK_TIMER_ID);
            let _ = DestroyWindow(hwnd);
        }
    }
}

pub fn hide_all() {
    let old = CLOCK_RUNTIME.with(|slot| slot.borrow_mut().take().map(|r| (r.hwnd, r.theme)));
    if let Some((raw, theme)) = old {
        destroy_clock_hwnd(raw, theme);
    }
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

pub fn sync(owner: HWND, action_msg: u32, cfg: ClockConfig) {
    let rep = CLOCK_RUNTIME.with(|slot| slot.borrow().as_ref().is_some_and(|r| r.repositioning));
    sync_inner(owner, action_msg, cfg, rep);
}

fn sync_inner(owner: HWND, action_msg: u32, cfg: ClockConfig, repositioning: bool) {
    if !cfg.enabled {
        hide_all();
        return;
    }
    if ensure_class().is_err() {
        return;
    }
    let want_behind = cfg.behind_icons && !repositioning;
    let cached_layer = CLOCK_RUNTIME.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|rt| rt.cached_layer.filter(|&l| layer_still_valid(l)))
    });
    let layer = if want_behind {
        cached_layer.or_else(discover_desktop_layer)
    } else {
        None
    };
    let actual_behind = want_behind && layer.is_some();

    let (need_recreate, old_hwnd, old_theme) = CLOCK_RUNTIME.with(|slot| {
        let borrow = slot.borrow();
        let Some(rt) = borrow.as_ref() else {
            return (true, 0, 0);
        };
        let valid = rt.hwnd != 0 && unsafe { IsWindow(HWND(rt.hwnd as *mut c_void)).as_bool() };
        if !valid || rt.behind_attached != actual_behind {
            (true, rt.hwnd, rt.theme)
        } else {
            (false, rt.hwnd, rt.theme)
        }
    });

    if need_recreate {
        destroy_clock_hwnd(old_hwnd, old_theme);
        let Ok(instance) = (unsafe { GetModuleHandleW(None) }) else {
            return;
        };
        let click_through = actual_behind || (cfg.locked && !repositioning);
        let mut ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        if click_through {
            ex |= WS_EX_TRANSPARENT;
        }
        let (style, parent) = if let Some(l) = layer {
            (WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS, l.parent)
        } else {
            (WS_POPUP, HWND::default())
        };
        let Ok(hwnd) = (unsafe {
            CreateWindowExW(
                ex,
                CLOCK_CLASS,
                w!("WallpaperLive Day Clock"),
                style,
                0,
                0,
                100,
                100,
                parent,
                None,
                instance,
                None,
            )
        }) else {
            return;
        };
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
                behind_attached: actual_behind,
                repositioning,
                dragging: false,
                drag_offset_x: 0,
                drag_offset_y: 0,
                last_stamp: (0, 0, 0, 0, 0, 0),
                last_bounds: None,
                cached_layer: layer,
                theme: 0,
                pending_wheel_delta: 0,
            });
        });
        render_and_place(hwnd, layer);
    } else {
        let hwnd = HWND(old_hwnd as *mut c_void);
        CLOCK_RUNTIME.with(|slot| {
            if let Some(rt) = slot.borrow_mut().as_mut() {
                rt.owner = owner.0 as isize;
                rt.action_msg = action_msg;
                rt.cfg = cfg;
                rt.repositioning = repositioning;
                if layer.is_some() {
                    rt.cached_layer = layer;
                }
            }
        });
        // Update click-through exstyle when lock state toggles in interactive mode.
        let click_through = actual_behind || (cfg.locked && !repositioning);
        unsafe {
            let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let next = if click_through {
                cur | WS_EX_TRANSPARENT.0
            } else {
                cur & !WS_EX_TRANSPARENT.0
            };
            if next != cur {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next as isize);
            }
        }
        render_and_place(hwnd, layer);
    }
}

pub fn sync_zorder() {
    let info = CLOCK_RUNTIME.with(|slot| {
        slot.borrow().as_ref().and_then(|rt| {
            (rt.hwnd != 0 && rt.behind_attached && !rt.repositioning).then_some(rt.hwnd)
        })
    });
    let Some(raw) = info else {
        return;
    };
    let hwnd = HWND(raw as *mut c_void);
    if unsafe { !IsWindow(hwnd).as_bool() } {
        return;
    }
    if let Some(layer) = discover_desktop_layer() {
        let after = if layer.raised { layer.icons } else { HWND_TOP };
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                after,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
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
    let date_str = format!("{day} {}, {year}.", month_name(month));
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

pub fn widget_pixel_size(day_len: usize, size_pct: u32, dpi: i32) -> (i32, i32) {
    let scale = (size_pct.clamp(50, 200) as f32 / 100.0) * (dpi.max(96) as f32 / 96.0);
    let glyph_w = 34.0 * scale;
    let gap = 22.0 * scale;
    let count = day_len.max(6) as f32;
    let day_total = count * glyph_w + (count - 1.0).max(0.0) * gap;
    let width = (day_total + 64.0 * scale).ceil().max(320.0 * scale) as i32;
    let height = (168.0 * scale).ceil() as i32;
    (width.clamp(220, 2400), height.clamp(110, 900))
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

fn dist_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let vx = bx - ax;
    let vy = by - ay;
    let wx = px - ax;
    let wy = py - ay;
    let c1 = vx * wx + vy * wy;
    if c1 <= 0.0 {
        return ((px - ax).powi(2) + (py - ay).powi(2)).sqrt();
    }
    let c2 = vx * vx + vy * vy;
    if c2 <= c1 || c2 <= f32::EPSILON {
        return ((px - bx).powi(2) + (py - by).powi(2)).sqrt();
    }
    let b = c1 / c2;
    let pb_x = ax + b * vx;
    let pb_y = ay + b * vy;
    ((px - pb_x).powi(2) + (py - pb_y).powi(2)).sqrt()
}

fn normalize_angle(a: f32) -> f32 {
    let mut r = a % (2.0 * PI);
    if r < 0.0 {
        r += 2.0 * PI;
    }
    r
}

fn dist_arc(px: f32, py: f32, cx: f32, cy: f32, radius: f32, a0_deg: f32, a1_deg: f32) -> f32 {
    // 0 deg = +X, 90 deg = +Y (screen space downward)
    let dx = px - cx;
    let dy = py - cy;
    let ang = normalize_angle(dy.atan2(dx));
    let a0 = normalize_angle(a0_deg.to_radians());
    let a1 = normalize_angle(a1_deg.to_radians());
    let inside = if a0 <= a1 {
        ang >= a0 && ang <= a1
    } else {
        ang >= a0 || ang <= a1
    };
    if inside {
        ((dx * dx + dy * dy).sqrt() - radius).abs()
    } else {
        let p0x = cx + radius * a0.cos();
        let p0y = cy + radius * a0.sin();
        let p1x = cx + radius * a1.cos();
        let p1y = cy + radius * a1.sin();
        let d0 = ((px - p0x).powi(2) + (py - p0y).powi(2)).sqrt();
        let d1 = ((px - p1x).powi(2) + (py - p1y).powi(2)).sqrt();
        d0.min(d1)
    }
}

#[derive(Clone, Copy)]
enum GlyphPrim {
    Seg(f32, f32, f32, f32),
    Arc(f32, f32, f32, f32, f32),
}

fn glyph_primitives(ch: char) -> &'static [GlyphPrim] {
    use GlyphPrim::{Arc, Seg};
    match ch {
        'A' => &[Seg(0.12, 0.94, 0.50, 0.06), Seg(0.50, 0.06, 0.88, 0.94)],
        'B' => &[
            Seg(0.16, 0.06, 0.16, 0.94),
            Seg(0.16, 0.06, 0.54, 0.06),
            Arc(0.54, 0.28, 0.22, -90.0, 90.0),
            Seg(0.28, 0.50, 0.56, 0.50),
            Arc(0.56, 0.72, 0.22, -90.0, 90.0),
            Seg(0.16, 0.94, 0.56, 0.94),
        ],
        'C' => &[Arc(0.56, 0.50, 0.44, 52.0, 308.0)],
        'D' => &[
            // Open geometric D curve matching Day Clock stencil (left stem omitted cleanly).
            Seg(0.20, 0.06, 0.46, 0.06),
            Arc(0.46, 0.50, 0.44, -90.0, 90.0),
            Seg(0.20, 0.94, 0.46, 0.94),
        ],
        'E' => &[
            // Three clean horizontal bars (minimalist stencil E).
            Seg(0.14, 0.06, 0.86, 0.06),
            Seg(0.14, 0.50, 0.78, 0.50),
            Seg(0.14, 0.94, 0.86, 0.94),
        ],
        'F' => &[
            Seg(0.14, 0.06, 0.86, 0.06),
            Seg(0.14, 0.50, 0.74, 0.50),
            Seg(0.14, 0.50, 0.14, 0.94),
        ],
        'G' => &[
            Arc(0.52, 0.50, 0.44, 48.0, 316.0),
            Seg(0.56, 0.54, 0.88, 0.54),
            Seg(0.88, 0.54, 0.88, 0.82),
        ],
        'H' => &[
            Seg(0.16, 0.06, 0.16, 0.94),
            Seg(0.84, 0.06, 0.84, 0.94),
            Seg(0.32, 0.50, 0.68, 0.50),
        ],
        'I' => &[Seg(0.50, 0.06, 0.50, 0.94)],
        'J' => &[
            Seg(0.76, 0.06, 0.76, 0.66),
            Arc(0.48, 0.66, 0.28, 0.0, 175.0),
        ],
        'K' => &[
            Seg(0.16, 0.06, 0.16, 0.94),
            Seg(0.84, 0.06, 0.34, 0.50),
            Seg(0.34, 0.50, 0.84, 0.94),
        ],
        'L' => &[Seg(0.18, 0.06, 0.18, 0.94), Seg(0.18, 0.94, 0.84, 0.94)],
        'M' => &[
            Seg(0.10, 0.94, 0.24, 0.06),
            Seg(0.24, 0.06, 0.50, 0.60),
            Seg(0.50, 0.60, 0.76, 0.06),
            Seg(0.76, 0.06, 0.90, 0.94),
        ],
        'N' => &[
            Seg(0.16, 0.94, 0.16, 0.06),
            Seg(0.16, 0.06, 0.84, 0.94),
            Seg(0.84, 0.06, 0.84, 0.54),
        ],
        'O' => &[Arc(0.50, 0.50, 0.44, 0.0, 359.9)],
        'P' => &[
            Seg(0.18, 0.06, 0.18, 0.94),
            Seg(0.18, 0.06, 0.56, 0.06),
            Arc(0.56, 0.29, 0.23, -90.0, 90.0),
            Seg(0.28, 0.52, 0.56, 0.52),
        ],
        'Q' => &[
            Arc(0.50, 0.50, 0.44, 0.0, 359.9),
            Seg(0.60, 0.64, 0.90, 0.94),
        ],
        'R' => &[
            Seg(0.18, 0.06, 0.18, 0.94),
            Seg(0.18, 0.06, 0.56, 0.06),
            Arc(0.56, 0.28, 0.22, -90.0, 90.0),
            Seg(0.28, 0.50, 0.56, 0.50),
            Seg(0.50, 0.50, 0.84, 0.94),
        ],
        'S' => &[
            Arc(0.52, 0.28, 0.22, 95.0, 335.0),
            Seg(0.36, 0.47, 0.64, 0.53),
            Arc(0.48, 0.72, 0.22, 275.0, 155.0),
        ],
        'T' => &[Seg(0.12, 0.06, 0.88, 0.06), Seg(0.50, 0.20, 0.50, 0.94)],
        'U' => &[
            Seg(0.16, 0.06, 0.16, 0.60),
            Arc(0.50, 0.60, 0.34, 0.0, 180.0),
            Seg(0.84, 0.06, 0.84, 0.60),
        ],
        'V' => &[Seg(0.12, 0.06, 0.50, 0.94), Seg(0.50, 0.94, 0.88, 0.06)],
        'W' => &[
            // Double open chevron matching WEDNESDAY in screenshot.
            Seg(0.06, 0.06, 0.30, 0.94),
            Seg(0.30, 0.94, 0.44, 0.48),
            Seg(0.56, 0.48, 0.70, 0.94),
            Seg(0.70, 0.94, 0.94, 0.06),
        ],
        'X' => &[Seg(0.14, 0.06, 0.86, 0.94), Seg(0.86, 0.06, 0.14, 0.94)],
        'Y' => &[
            Seg(0.14, 0.06, 0.50, 0.52),
            Seg(0.86, 0.06, 0.50, 0.52),
            Seg(0.50, 0.64, 0.50, 0.94),
        ],
        'Z' => &[
            Seg(0.14, 0.06, 0.86, 0.06),
            Seg(0.86, 0.06, 0.14, 0.94),
            Seg(0.14, 0.94, 0.86, 0.94),
        ],
        _ => &[],
    }
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

#[allow(clippy::too_many_arguments)]
pub fn render_stencil_word(
    buf: &mut [u32],
    stride_px: usize,
    buf_h: i32,
    text: &str,
    top_y: f32,
    glyph_w: f32,
    glyph_h: f32,
    letter_gap: f32,
    stroke_radius: f32,
    center_x: f32,
    color: u32,
) {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() || stride_px == 0 || buf_h <= 0 {
        return;
    }
    let total_w = chars.len() as f32 * glyph_w + (chars.len() as f32 - 1.0) * letter_gap;
    let start_x = center_x - total_w * 0.5;
    let r = (color >> 16) & 255;
    let g = (color >> 8) & 255;
    let b = color & 255;

    for (idx, ch) in chars.into_iter().enumerate() {
        let prims = glyph_primitives(ch);
        if prims.is_empty() {
            continue;
        }
        let gx = start_x + idx as f32 * (glyph_w + letter_gap);
        let gy = top_y;
        let pad = stroke_radius + 4.0;
        let x0 = (gx - pad).floor().max(0.0) as i32;
        let x1 = (gx + glyph_w + pad).ceil().min(stride_px as f32) as i32;
        let y0 = (gy - pad).floor().max(0.0) as i32;
        let y1 = (gy + glyph_h + pad).ceil().min(buf_h as f32) as i32;
        for py in y0..y1 {
            let row = py as usize * stride_px;
            for px in x0..x1 {
                let cx = px as f32 + 0.5;
                let cy = py as f32 + 0.5;
                // Evaluate subtle drop shadow first (2px down-right offset).
                let sx_u = (cx - 1.5 - gx) / glyph_w;
                let sy_u = (cy - 2.0 - gy) / glyph_h;
                let mut d_shadow = f32::MAX;
                let fx_u = (cx - gx) / glyph_w;
                let fy_u = (cy - gy) / glyph_h;
                let mut d_fg = f32::MAX;
                for &prim in prims {
                    match prim {
                        GlyphPrim::Seg(ax, ay, bx, by) => {
                            let scale = glyph_w.max(glyph_h);
                            d_shadow =
                                d_shadow.min(dist_segment(sx_u, sy_u, ax, ay, bx, by) * scale);
                            d_fg = d_fg.min(dist_segment(fx_u, fy_u, ax, ay, bx, by) * scale);
                        }
                        GlyphPrim::Arc(acx, acy, rad, a0, a1) => {
                            let scale = glyph_w.max(glyph_h);
                            d_shadow =
                                d_shadow.min(dist_arc(sx_u, sy_u, acx, acy, rad, a0, a1) * scale);
                            d_fg = d_fg.min(dist_arc(fx_u, fy_u, acx, acy, rad, a0, a1) * scale);
                        }
                    }
                }
                let sh_cov = (stroke_radius + 0.7 - d_shadow).clamp(0.0, 1.0);
                if sh_cov > 0.01 {
                    let sh_a = (sh_cov * 76.0).round().clamp(0.0, 255.0) as u8;
                    premul_blend_pixel(&mut buf[row + px as usize], 0, 0, 0, sh_a);
                }
                let fg_cov = (stroke_radius + 0.65 - d_fg).clamp(0.0, 1.0);
                if fg_cov > 0.01 {
                    let fg_a = (fg_cov * 255.0).round().clamp(0.0, 255.0) as u8;
                    premul_blend_pixel(&mut buf[row + px as usize], r, g, b, fg_a);
                }
            }
        }
    }
}

unsafe fn draw_composited_text_line(
    dc: windows::Win32::Graphics::Gdi::HDC,
    theme: HTHEME,
    text: &str,
    area: RECT,
    font_px: i32,
    color: u32,
    bold: bool,
) {
    let font = unsafe {
        CreateFontW(
            -font_px.max(10),
            0,
            0,
            0,
            if bold { FW_SEMIBOLD.0 } else { 400 } as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            DEFAULT_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            w!("Segoe UI Variable Display"),
        )
    };
    let old_font = if !font.0.is_null() {
        Some(unsafe { SelectObject(dc, font) })
    } else {
        None
    };
    let wide: Vec<u16> = text.encode_utf16().collect();
    if theme.0 != 0 && !wide.is_empty() {
        let shadow_rect = RECT {
            left: area.left + 1,
            top: area.top + 1,
            right: area.right + 1,
            bottom: area.bottom + 1,
        };
        let mut r_shadow = shadow_rect;
        let opts_shadow = DTTOPTS {
            dwSize: size_of::<DTTOPTS>() as u32,
            dwFlags: DTT_COMPOSITED | DTT_TEXTCOLOR,
            crText: COLORREF(0x0010_1010),
            ..Default::default()
        };
        let _ = unsafe {
            DrawThemeTextEx(
                theme,
                dc,
                0,
                0,
                &wide,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                &mut r_shadow,
                Some(&opts_shadow),
            )
        };
        let bgr = ((color & 255) << 16) | (color & 0x0000_FF00) | ((color >> 16) & 255);
        let mut r_main = area;
        let opts_main = DTTOPTS {
            dwSize: size_of::<DTTOPTS>() as u32,
            dwFlags: DTT_COMPOSITED | DTT_TEXTCOLOR,
            crText: COLORREF(bgr),
            ..Default::default()
        };
        let _ = unsafe {
            DrawThemeTextEx(
                theme,
                dc,
                0,
                0,
                &wide,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                &mut r_main,
                Some(&opts_main),
            )
        };
    }
    if let Some(prev) = old_font {
        unsafe {
            let _ = SelectObject(dc, prev);
        }
    }
    if !font.0.is_null() {
        unsafe {
            let _ = DeleteObject(font);
        }
    }
}

fn draw_reposition_guide(buf: &mut [u32], w: i32, h: i32) {
    if w <= 8 || h <= 8 {
        return;
    }
    let corner = 18.min(w / 4).min(h / 4);
    let col = 0xD0F3_EEE3u32; // semi-opaque cream corner brackets
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

fn render_and_place(hwnd: HWND, layer: Option<DesktopLayer>) {
    let snapshot = CLOCK_RUNTIME.with(|slot| slot.borrow().clone());
    let Some(rt) = snapshot else {
        return;
    };
    let owner = HWND(rt.owner as *mut c_void);
    let (work, dpi) = target_monitor_info(hwnd, owner, rt.cfg.pos);
    let now = unsafe { local_time() };
    let (day_str, date_str, time_str) = format_clock_lines(
        now.wYear,
        now.wMonth,
        now.wDayOfWeek,
        now.wDay,
        now.wHour,
        now.wMinute,
        rt.cfg.use_24h,
    );
    let (w, h) = widget_pixel_size(day_str.chars().count(), rt.cfg.size_pct, dpi);
    let (sx, sy) = resolve_screen_position(rt.cfg.pos, work, w, h);

    // SAFETY: owns offscreen memory DC, 32-bit DIBSection, and HTHEME strictly for this synchronous update.
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
            let prev_bmp = SelectObject(mem_dc, bmp);
            let total = (w as usize).saturating_mul(h as usize);
            std::slice::from_raw_parts_mut(bits as *mut u32, total).fill(0);

            let scale = (rt.cfg.size_pct.clamp(50, 200) as f32 / 100.0) * (dpi as f32 / 96.0);
            let mut theme = HTHEME(rt.theme);
            if theme.0 == 0 {
                theme = OpenThemeData(hwnd, w!("WINDOW"));
                if theme.0 != 0 {
                    CLOCK_RUNTIME.with(|slot| {
                        if let Some(state) = slot.borrow_mut().as_mut()
                            && state.hwnd == hwnd.0 as isize
                        {
                            state.theme = theme.0;
                        }
                    });
                }
            }
            draw_composited_text_line(
                mem_dc,
                theme,
                &date_str,
                RECT {
                    left: 8,
                    top: (76.0 * scale).round() as i32,
                    right: w - 8,
                    bottom: (112.0 * scale).round() as i32,
                },
                (17.0 * scale).round() as i32,
                rt.cfg.color,
                true,
            );
            draw_composited_text_line(
                mem_dc,
                theme,
                &time_str,
                RECT {
                    left: 8,
                    top: (116.0 * scale).round() as i32,
                    right: w - 8,
                    bottom: (152.0 * scale).round() as i32,
                },
                (16.0 * scale).round() as i32,
                rt.cfg.color,
                true,
            );
            let _ = GdiFlush();

            let buf = std::slice::from_raw_parts_mut(bits as *mut u32, total);
            render_stencil_word(
                buf,
                w as usize,
                h,
                &day_str,
                16.0 * scale,
                34.0 * scale,
                42.0 * scale,
                22.0 * scale,
                (2.2 * scale).max(1.2),
                w as f32 * 0.5,
                rt.cfg.color,
            );
            apply_opacity_in_place(buf, rt.cfg.opacity_pct);
            if rt.repositioning {
                draw_reposition_guide(buf, w, h);
            }

            let mut dst = POINT { x: sx, y: sy };
            if let Some(l) = layer {
                let mut pts = [dst];
                MapWindowPoints(None, l.parent, &mut pts);
                dst = pts[0];
            }
            let size = SIZE { cx: w, cy: h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let next_bounds = (dst.x, dst.y, w, h, rt.repositioning);
            if rt.last_bounds != Some(next_bounds) {
                let after = if let Some(l) = layer {
                    if l.raised { l.icons } else { HWND_TOP }
                } else if rt.repositioning {
                    HWND_TOP
                } else {
                    HWND_BOTTOM
                };
                let _ = SetWindowPos(
                    hwnd,
                    after,
                    dst.x,
                    dst.y,
                    w,
                    h,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
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
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem_dc);
    }
}

fn show_context_menu(hwnd: HWND, screen_pt: POINT) {
    let (owner, action_msg, cfg) = match CLOCK_RUNTIME.with(|s| {
        s.borrow()
            .as_ref()
            .map(|r| (HWND(r.owner as *mut c_void), r.action_msg, r.cfg))
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
        "Below shortcuts (On)"
    } else {
        "Below shortcuts (Off)"
    };
    let fmt_label = if cfg.use_24h {
        "24-hour clock (On)"
    } else {
        "12-hour clock (AM/PM)"
    };
    let items = [
        (CLOCK_CTX_SIZE_DELTA + 1, "Bigger\tScroll up"),
        (CLOCK_CTX_SIZE_DELTA + 2, "Smaller\tScroll down"),
        (CLOCK_CTX_24H, fmt_label),
        (CLOCK_CTX_LOCK, lock_label),
        (CLOCK_CTX_BEHIND_ICONS, icons_label),
        (CLOCK_CTX_OPEN_SETTINGS, "Widget settings"),
        (CLOCK_CTX_REMOVE, "Remove from desktop"),
    ];
    let target_owner = if !owner.0.is_null() && unsafe { IsWindow(owner).as_bool() } {
        owner
    } else {
        hwnd
    };
    if let Ok(choice) = tray::popup_at(target_owner, screen_pt, &items, None)
        && choice != 0
        && !owner.0.is_null()
    {
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
            WM_NCHITTEST => {
                let pass = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .is_none_or(|rt| rt.behind_attached || (rt.cfg.locked && !rt.repositioning))
                });
                if pass {
                    LRESULT(HTTRANSPARENT as isize)
                } else {
                    LRESULT(HTCLIENT as isize)
                }
            }
            WM_SETCURSOR => {
                let interactive = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow().as_ref().is_some_and(|rt| {
                        !rt.behind_attached && (!rt.cfg.locked || rt.repositioning)
                    })
                });
                let cursor = if interactive { IDC_SIZEALL } else { IDC_ARROW };
                if let Ok(h) = LoadCursorW(None, cursor) {
                    SetCursor(h);
                }
                LRESULT(1)
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
                    let cached = CLOCK_RUNTIME.with(|s| {
                        s.borrow().as_ref().and_then(|r| {
                            (r.behind_attached && !r.repositioning).then_some(
                                r.cached_layer
                                    .filter(|&l| layer_still_valid(l))
                                    .or_else(discover_desktop_layer),
                            )
                        })
                    });
                    let layer = cached.flatten();
                    render_and_place(hwnd, layer);
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
                let old_theme = CLOCK_RUNTIME.with(|slot| {
                    let mut borrow = slot.borrow_mut();
                    if borrow.as_ref().is_some_and(|rt| rt.hwnd == hwnd.0 as isize) {
                        borrow.take().map_or(0, |rt| rt.theme)
                    } else {
                        0
                    }
                });
                if old_theme != 0 {
                    let _ = CloseThemeData(HTHEME(old_theme));
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_LBUTTONDOWN => {
                let interactive = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow().as_ref().is_some_and(|rt| {
                        !rt.behind_attached && (!rt.cfg.locked || rt.repositioning)
                    })
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
                        rt.drag_offset_x = cur.x - wr.left;
                        rt.drag_offset_y = cur.y - wr.top;
                    }
                });
                let _ = SetCapture(hwnd);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let drag = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .filter(|rt| rt.dragging)
                        .map(|rt| (rt.drag_offset_x, rt.drag_offset_y))
                });
                if let Some((ox, oy)) = drag {
                    let mut cur = POINT::default();
                    let _ = GetCursorPos(&mut cur);
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
                let was_drag = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow_mut().as_mut().map_or((false, 0, 0), |rt| {
                        let d = rt.dragging;
                        rt.dragging = false;
                        rt.repositioning = false;
                        (d, rt.owner, rt.action_msg)
                    })
                });
                if GetCapture() == hwnd {
                    let _ = ReleaseCapture();
                }
                if was_drag.0 {
                    let mut wr = RECT::default();
                    if GetWindowRect(hwnd, &mut wr).is_ok() {
                        CLOCK_SAVED_POS.with(|s| *s.borrow_mut() = Some((wr.left, wr.top)));
                        if was_drag.1 != 0 {
                            let _ = PostMessageW(
                                HWND(was_drag.1 as *mut c_void),
                                was_drag.2,
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
                        let ok = !rt.behind_attached && (!rt.cfg.locked || rt.repositioning);
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
                    slot.borrow_mut().as_mut().and_then(|rt| {
                        if rt.repositioning {
                            rt.repositioning = false;
                            rt.dragging = false;
                            Some((rt.owner, rt.action_msg, rt.cfg))
                        } else {
                            None
                        }
                    })
                });
                if let Some((owner, action_msg, cfg)) = info {
                    sync_inner(HWND(owner as *mut c_void), action_msg, cfg, false);
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
    fn clock_formatting_and_stencil_render_cover_all_days() {
        let (day, date, t24) = format_clock_lines(2026, 11, 3, 18, 12, 2, true);
        assert_eq!(day, "WEDNESDAY");
        assert_eq!(date, "18 NOVEMBER, 2026.");
        assert_eq!(t24, "- 12:02 -");

        let (_, _, t12) = format_clock_lines(2026, 11, 3, 18, 0, 5, false);
        assert_eq!(t12, "- 12:05 AM -");
        let (_, _, t12_pm) = format_clock_lines(2026, 11, 3, 18, 15, 45, false);
        assert_eq!(t12_pm, "- 3:45 PM -");

        for dow in 0..=6 {
            let name = weekday_name(dow);
            assert!(name.chars().all(|c| !glyph_primitives(c).is_empty()));
        }

        let (w, h) = widget_pixel_size(9, 100, 96);
        let mut buf = vec![0u32; (w * h) as usize];
        render_stencil_word(
            &mut buf,
            w as usize,
            h,
            "WEDNESDAY",
            16.0,
            34.0,
            42.0,
            22.0,
            2.2,
            w as f32 * 0.5,
            0x00FF_FFFF,
        );
        assert!(buf.iter().any(|&px| (px >> 24) == 255));
        apply_opacity_in_place(&mut buf, 50);
        let max_alpha = buf.iter().map(|&px| px >> 24).max().unwrap_or(0);
        assert!((120..=135).contains(&max_alpha));

        assert_eq!(minute_timer_delay_ms(0), 60_050);
        assert_eq!(minute_timer_delay_ms(59), 1_050);
        assert_eq!(minute_timer_delay_ms(65), 1_050);
    }
}
