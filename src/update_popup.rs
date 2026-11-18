use crate::visuals::{
    self, UPDATE_DISABLE, UPDATE_DISMISS, UPDATE_NOW, UPDATE_WHATS_NEW, UpdatePopupHit,
    UpdatePopupLayout,
};
use std::{cell::Cell, ffi::c_void, mem::size_of};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::{
            Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute},
            Gdi::{
                BeginPaint, ClientToScreen, CreateRoundRectRgn, DeleteObject, EndPaint,
                GetMonitorInfoW, HBRUSH, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
                MonitorFromWindow, PAINTSTRUCT, RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow,
                SetWindowRgn,
            },
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{
                GetCapture, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT,
                TrackMouseEvent, VK_ESCAPE,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, w},
};

const POPUP_CLASS: PCWSTR = w!("WallpaperLive.UpdatePopup");
const WM_MOUSELEAVE_MSG: u32 = 0x02A3;

struct PopupState {
    owner: HWND,
    action_msg: u32,
    tag: String,
    title: String,
    subtitle: String,
    update_label: String,
    downloading: bool,
    hover: Option<UpdatePopupHit>,
    pressed: Option<UpdatePopupHit>,
    tracking_leave: bool,
    dpi: i32,
    glass: bool,
    layout: UpdatePopupLayout,
}

impl PopupState {
    fn new(owner: HWND, action_msg: u32, tag: &str, dpi: i32) -> Self {
        let ver = tag.strip_prefix('v').unwrap_or(tag);
        let current = env!("CARGO_PKG_VERSION");
        Self {
            owner,
            action_msg,
            tag: tag.to_owned(),
            title: format!("WallpaperLive {ver} is out"),
            subtitle: format!("You are on {current}. Read what changed, or update now."),
            update_label: "Update".into(),
            downloading: false,
            hover: None,
            pressed: None,
            tracking_leave: false,
            dpi,
            glass: false,
            layout: UpdatePopupLayout::measure(dpi, false),
        }
    }

    fn set_ready(&mut self) {
        let current = env!("CARGO_PKG_VERSION");
        self.subtitle = format!("You are on {current}. Read what changed, or update now.");
        self.update_label = "Update".into();
        self.downloading = false;
        self.layout = UpdatePopupLayout::measure(self.dpi, false);
    }

    fn set_downloading(&mut self, percent: Option<u8>) {
        self.downloading = true;
        self.update_label = "Downloading...".into();
        self.subtitle = match percent {
            Some(pct) => format!("Downloading update installer ({pct}%)..."),
            None => "Downloading update installer...".into(),
        };
        self.layout = UpdatePopupLayout::measure(self.dpi, true);
    }

    fn set_verifying(&mut self) {
        self.downloading = true;
        self.update_label = "Verifying...".into();
        self.subtitle = "Verifying installer SHA-256 checksum...".into();
        self.layout = UpdatePopupLayout::measure(self.dpi, true);
    }

    fn set_error(&mut self, error: &str) {
        self.downloading = false;
        self.update_label = "Retry".into();
        self.subtitle = error.to_owned();
        self.layout = UpdatePopupLayout::measure(self.dpi, false);
    }
}

thread_local! {
    static POPUP_HWND: Cell<isize> = const { Cell::new(0) };
}

fn current_popup() -> Option<HWND> {
    let raw = POPUP_HWND.with(Cell::get);
    if raw == 0 {
        return None;
    }
    let hwnd = HWND(raw as *mut c_void);
    if unsafe { IsWindow(hwnd).as_bool() } {
        Some(hwnd)
    } else {
        POPUP_HWND.with(|slot| slot.set(0));
        None
    }
}

unsafe fn state_mut<'a>(hwnd: HWND) -> Option<&'a mut PopupState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut PopupState;
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &mut *ptr })
    }
}

fn ensure_class() -> Result<(), String> {
    let instance = unsafe { GetModuleHandleW(None) }.map_err(|e| e.to_string())?;
    let mut existing = WNDCLASSEXW::default();
    if unsafe { GetClassInfoExW(instance, POPUP_CLASS, &mut existing) }.is_ok() {
        return Ok(());
    }
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(popup_proc),
        hInstance: instance.into(),
        lpszClassName: POPUP_CLASS,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.map_err(|e| e.to_string())?,
        hbrBackground: HBRUSH::default(),
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&wc) } == 0 {
        let mut check = WNDCLASSEXW::default();
        if unsafe { GetClassInfoExW(instance, POPUP_CLASS, &mut check) }.is_err() {
            return Err("Could not register the update popup class".into());
        }
    }
    Ok(())
}

fn anchor_rect(owner: HWND, width: i32, height: i32, dpi: i32) -> (i32, i32) {
    let s = |n: i32| n * dpi.max(96) / 96;
    let mut client = RECT::default();
    let mut origin = POINT::default();
    let ok = unsafe {
        GetClientRect(owner, &mut client).is_ok() && ClientToScreen(owner, &mut origin).as_bool()
    };
    if !ok {
        return (100, 100);
    }
    let mut x = origin.x + s(20);
    let mut y = origin.y + (client.bottom - s(76) - height - s(14)).max(s(8));

    let mon = unsafe { MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST) };
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(mon, &mut mi) }.as_bool() {
        let max_x = (mi.rcWork.right - width - s(8)).max(mi.rcWork.left);
        let max_y = (mi.rcWork.bottom - height - s(8)).max(mi.rcWork.top);
        x = x.clamp(mi.rcWork.left, max_x);
        y = y.clamp(mi.rcWork.top, max_y);
    }
    (x, y)
}

unsafe fn configure_popup_frame(hwnd: HWND, width: i32, height: i32, dpi: i32) -> bool {
    let glass = visuals::enable_backdrop(hwnd);
    let corner = DWMWCP_ROUND.0;
    if unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&corner as *const i32).cast(),
            size_of::<i32>() as u32,
        )
    }
    .is_err()
    {
        let d = 12 * dpi.max(96) / 96;
        let rgn = unsafe { CreateRoundRectRgn(0, 0, width + 1, height + 1, d, d) };
        if !rgn.0.is_null() && unsafe { SetWindowRgn(hwnd, rgn, true) } == 0 {
            let _ = unsafe { DeleteObject(rgn) };
        }
    }
    glass
}

pub fn show(owner: HWND, action_msg: u32, tag: &str) {
    if owner.0.is_null() || unsafe { !IsWindow(owner).as_bool() } {
        return;
    }
    let dpi = unsafe { GetDpiForWindow(owner) }.max(96) as i32;
    if let Some(hwnd) = current_popup() {
        // SAFETY: popup HWND was created on this UI thread with a boxed PopupState.
        if let Some(state) = unsafe { state_mut(hwnd) } {
            let same_tag = state.tag == tag && state.owner == owner;
            if !same_tag {
                *state = PopupState::new(owner, action_msg, tag, dpi);
            } else if !state.downloading {
                state.set_ready();
            }
            state.dpi = dpi;
            state.layout = UpdatePopupLayout::measure(dpi, state.downloading);
            state.glass = unsafe {
                configure_popup_frame(hwnd, state.layout.width, state.layout.height, dpi)
            };
        }
        sync(owner);
        if unsafe { IsWindowVisible(owner).as_bool() && !IsIconic(owner).as_bool() } {
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                let _ = RedrawWindow(hwnd, None, None, RDW_INVALIDATE | RDW_UPDATENOW);
            }
        }
        return;
    }

    if ensure_class().is_err() {
        return;
    }
    let Ok(instance) = (unsafe { GetModuleHandleW(None) }) else {
        return;
    };
    let mut state = Box::new(PopupState::new(owner, action_msg, tag, dpi));
    let (x, y) = anchor_rect(owner, state.layout.width, state.layout.height, dpi);
    let Ok(hwnd) = (unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            POPUP_CLASS,
            w!("WallpaperLive Update"),
            WS_POPUP,
            x,
            y,
            state.layout.width,
            state.layout.height,
            owner,
            None,
            instance,
            None,
        )
    }) else {
        return;
    };
    state.glass =
        unsafe { configure_popup_frame(hwnd, state.layout.width, state.layout.height, dpi) };
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
    }
    POPUP_HWND.with(|slot| slot.set(hwnd.0 as isize));
    if unsafe { IsWindowVisible(owner).as_bool() && !IsIconic(owner).as_bool() } {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = RedrawWindow(hwnd, None, None, RDW_INVALIDATE | RDW_UPDATENOW);
        }
    }
}

pub fn set_progress(owner: HWND, percent: Option<u8>) {
    if let Some(hwnd) = current_popup()
        && let Some(state) = unsafe { state_mut(hwnd) }
        && state.owner == owner
    {
        state.set_downloading(percent);
        unsafe {
            let _ = InvalidateRect(hwnd, None, false);
        }
    }
}

pub fn set_verifying(owner: HWND) {
    if let Some(hwnd) = current_popup()
        && let Some(state) = unsafe { state_mut(hwnd) }
        && state.owner == owner
    {
        state.set_verifying();
        unsafe {
            let _ = InvalidateRect(hwnd, None, false);
        }
    }
}

pub fn set_error(owner: HWND, error: &str) {
    if let Some(hwnd) = current_popup()
        && let Some(state) = unsafe { state_mut(hwnd) }
        && state.owner == owner
    {
        state.set_error(error);
        unsafe {
            let _ = InvalidateRect(hwnd, None, false);
        }
    }
}

pub fn sync(owner: HWND) {
    let Some(hwnd) = current_popup() else {
        return;
    };
    let Some(state) = (unsafe { state_mut(hwnd) }) else {
        return;
    };
    if state.owner != owner {
        return;
    }
    let owner_visible = unsafe { IsWindowVisible(owner).as_bool() && !IsIconic(owner).as_bool() };
    if !owner_visible {
        if unsafe { IsWindowVisible(hwnd).as_bool() } {
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
        return;
    }
    let dpi = unsafe { GetDpiForWindow(owner) }.max(96) as i32;
    if dpi != state.dpi {
        state.dpi = dpi;
        state.layout = UpdatePopupLayout::measure(dpi, state.downloading);
        state.glass =
            unsafe { configure_popup_frame(hwnd, state.layout.width, state.layout.height, dpi) };
    }
    let (x, y) = anchor_rect(owner, state.layout.width, state.layout.height, state.dpi);
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            HWND::default(),
            x,
            y,
            state.layout.width,
            state.layout.height,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

pub fn refresh_theme(owner: HWND) {
    let Some(hwnd) = current_popup() else {
        return;
    };
    let Some(state) = (unsafe { state_mut(hwnd) }) else {
        return;
    };
    if state.owner == owner {
        state.glass = unsafe {
            configure_popup_frame(hwnd, state.layout.width, state.layout.height, state.dpi)
        };
        unsafe {
            let _ = InvalidateRect(hwnd, None, false);
        }
    }
}

pub fn dismiss(owner: HWND) {
    let Some(hwnd) = current_popup() else {
        return;
    };
    let matches_owner =
        unsafe { state_mut(hwnd) }.is_some_and(|s| owner.0.is_null() || s.owner == owner);
    if matches_owner {
        POPUP_HWND.with(|slot| slot.set(0));
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}

#[allow(dead_code)]
pub fn is_open(owner: HWND) -> bool {
    current_popup().is_some_and(|hwnd| {
        unsafe { state_mut(hwnd) }.is_some_and(|s| owner.0.is_null() || s.owner == owner)
    })
}

unsafe extern "system" fn popup_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: state pointer is owned uniquely by GWLP_USERDATA from creation until WM_NCDESTROY.
    unsafe {
        let Some(state) = state_mut(hwnd) else {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                if !dc.0.is_null() {
                    let mut area = RECT::default();
                    let _ = GetClientRect(hwnd, &mut area);
                    let paint = visuals::UpdatePopupPaint {
                        layout: &state.layout,
                        title: &state.title,
                        subtitle: &state.subtitle,
                        update_label: &state.update_label,
                        downloading: state.downloading,
                        hover: state.hover,
                        pressed: state.pressed,
                        dpi: state.dpi,
                        glass: state.glass,
                    };
                    if !visuals::paint_update_popup_into(dc, area, &paint) {
                        visuals::fill(dc, area, visuals::CARD);
                    }
                }
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let cursor_id = if state.hover.is_some() {
                    IDC_HAND
                } else {
                    IDC_ARROW
                };
                if let Ok(cursor) = LoadCursorW(None, cursor_id) {
                    SetCursor(cursor);
                }
                LRESULT(1)
            }
            WM_MOUSEMOVE => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                if !state.tracking_leave {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    if TrackMouseEvent(&mut tme).is_ok() {
                        state.tracking_leave = true;
                    }
                }
                let next = state.layout.hit_test(x, y);
                if next != state.hover {
                    state.hover = next;
                    let _ = InvalidateRect(hwnd, None, false);
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE_MSG => {
                state.tracking_leave = false;
                if state.hover.is_some() || state.pressed.is_some() {
                    state.hover = None;
                    state.pressed = None;
                    let _ = InvalidateRect(hwnd, None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                let hit = state.layout.hit_test(x, y);
                if hit.is_some() {
                    state.pressed = hit;
                    if GetCapture() != hwnd {
                        let _ = SetCapture(hwnd);
                    }
                    let _ = InvalidateRect(hwnd, None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                let pressed = state.pressed.take();
                if GetCapture() == hwnd {
                    let _ = ReleaseCapture();
                }
                let hit = state.layout.hit_test(x, y);
                let _ = InvalidateRect(hwnd, None, false);
                if let Some(target) = pressed
                    && hit == Some(target)
                {
                    let action_id = match target {
                        UpdatePopupHit::Close => UPDATE_DISMISS,
                        UpdatePopupHit::Update => UPDATE_NOW,
                        UpdatePopupHit::WhatsNew => UPDATE_WHATS_NEW,
                        UpdatePopupHit::Disable => UPDATE_DISABLE,
                    };
                    if !state.downloading || target != UpdatePopupHit::Update {
                        let _ = PostMessageW(
                            state.owner,
                            state.action_msg,
                            WPARAM(action_id),
                            LPARAM(0),
                        );
                    }
                }
                LRESULT(0)
            }
            WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => {
                let _ = PostMessageW(
                    state.owner,
                    state.action_msg,
                    WPARAM(UPDATE_DISMISS),
                    LPARAM(0),
                );
                LRESULT(0)
            }
            WM_NCDESTROY => {
                if POPUP_HWND.with(Cell::get) == hwnd.0 as isize {
                    POPUP_HWND.with(|slot| slot.set(0));
                }
                let raw = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                if raw != 0 {
                    drop(Box::from_raw(raw as *mut PopupState));
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
