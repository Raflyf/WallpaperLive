use crate::visuals::{
    self, DECODER_DISMISS, DECODER_NOW, UPDATE_DISABLE, UPDATE_DISMISS, UPDATE_NOW,
    UPDATE_WHATS_NEW, UpdatePopupHit, UpdatePopupLayout,
};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    mem::size_of,
    rc::Rc,
};
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
                GetCapture, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE,
                TRACKMOUSEEVENT, TrackMouseEvent, VK_ESCAPE, VK_RETURN, VK_SHIFT, VK_SPACE, VK_TAB,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, w},
};

const POPUP_CLASS: PCWSTR = w!("WallpaperLive.UpdatePopup");
const WM_MOUSELEAVE_MSG: u32 = 0x02A3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PopupKind {
    AppUpdate,
    DecoderSetup,
}

struct PopupState {
    kind: PopupKind,
    owner: HWND,
    action_msg: u32,
    tag: String,
    title: String,
    subtitle: String,
    update_label: String,
    secondary_label: String,
    downloading: bool,
    hover: Option<UpdatePopupHit>,
    pressed: Option<UpdatePopupHit>,
    focused: Option<UpdatePopupHit>,
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
            kind: PopupKind::AppUpdate,
            owner,
            action_msg,
            tag: tag.to_owned(),
            title: format!("WallpaperLive {ver} is out"),
            subtitle: format!("You are on {current}. Read what changed, or update now."),
            update_label: "Update".into(),
            secondary_label: "What's new".into(),
            downloading: false,
            hover: None,
            pressed: None,
            focused: None,
            tracking_leave: false,
            dpi,
            glass: false,
            layout: UpdatePopupLayout::measure_with_footer(dpi, false, true),
        }
    }

    fn new_decoder(owner: HWND, action_msg: u32, dpi: i32) -> Self {
        Self {
            kind: PopupKind::DecoderSetup,
            owner,
            action_msg,
            tag: String::new(),
            title: "Native decoder needed".into(),
            subtitle: "Download libmpv-2.dll (~31.8 MB, SHA-256 verified) to play video?".into(),
            update_label: "Download".into(),
            secondary_label: "Not now".into(),
            downloading: false,
            hover: None,
            pressed: None,
            focused: Some(UpdatePopupHit::Update),
            tracking_leave: false,
            dpi,
            glass: false,
            layout: UpdatePopupLayout::measure_with_footer(dpi, false, false),
        }
    }

    fn measure(&self) -> UpdatePopupLayout {
        UpdatePopupLayout::measure_with_footer(
            self.dpi,
            self.downloading,
            self.kind == PopupKind::AppUpdate,
        )
    }

    fn focus_order(&self) -> Vec<UpdatePopupHit> {
        let mut order = Vec::with_capacity(4);
        if !self.downloading {
            order.push(UpdatePopupHit::Update);
        }
        order.push(UpdatePopupHit::WhatsNew);
        if self.layout.footer_visible {
            order.push(UpdatePopupHit::Disable);
        }
        order.push(UpdatePopupHit::Close);
        order
    }

    fn step_focus(&mut self, reverse: bool) {
        let order = self.focus_order();
        if order.is_empty() {
            self.focused = None;
            return;
        }
        let next = match self
            .focused
            .and_then(|f| order.iter().position(|&x| x == f))
        {
            Some(idx) => {
                if reverse {
                    if idx == 0 { order.len() - 1 } else { idx - 1 }
                } else {
                    (idx + 1) % order.len()
                }
            }
            None => {
                if reverse {
                    order.len() - 1
                } else {
                    0
                }
            }
        };
        self.focused = Some(order[next]);
    }

    fn action_for_hit(&self, target: UpdatePopupHit) -> Option<usize> {
        if self.downloading && target == UpdatePopupHit::Update {
            return None;
        }
        match (self.kind, target) {
            (PopupKind::AppUpdate, UpdatePopupHit::Close) => Some(UPDATE_DISMISS),
            (PopupKind::AppUpdate, UpdatePopupHit::Update) => Some(UPDATE_NOW),
            (PopupKind::AppUpdate, UpdatePopupHit::WhatsNew) => Some(UPDATE_WHATS_NEW),
            (PopupKind::AppUpdate, UpdatePopupHit::Disable) => Some(UPDATE_DISABLE),
            (PopupKind::DecoderSetup, UpdatePopupHit::Close)
            | (PopupKind::DecoderSetup, UpdatePopupHit::WhatsNew) => Some(DECODER_DISMISS),
            (PopupKind::DecoderSetup, UpdatePopupHit::Update) => Some(DECODER_NOW),
            (PopupKind::DecoderSetup, UpdatePopupHit::Disable) => None,
        }
    }

    fn set_ready(&mut self) {
        match self.kind {
            PopupKind::AppUpdate => {
                let current = env!("CARGO_PKG_VERSION");
                self.subtitle = format!("You are on {current}. Read what changed, or update now.");
                self.update_label = "Update".into();
            }
            PopupKind::DecoderSetup => {
                self.subtitle =
                    "Download libmpv-2.dll (~31.8 MB, SHA-256 verified) to play video?".into();
                self.update_label = "Download".into();
            }
        }
        self.downloading = false;
        self.layout = self.measure();
    }

    fn set_downloading(&mut self, percent: Option<u8>) {
        self.downloading = true;
        self.update_label = "Downloading...".into();
        if self.focused == Some(UpdatePopupHit::Update) {
            self.focused = Some(UpdatePopupHit::WhatsNew);
        }
        self.subtitle = match (self.kind, percent) {
            (PopupKind::AppUpdate, Some(pct)) => {
                format!("Downloading update installer ({pct}%)...")
            }
            (PopupKind::AppUpdate, None) => "Downloading update installer...".into(),
            (PopupKind::DecoderSetup, Some(pct)) => {
                format!("Downloading native decoder ({pct}%)...")
            }
            (PopupKind::DecoderSetup, None) => "Downloading native decoder...".into(),
        };
        self.layout = self.measure();
    }

    fn set_verifying(&mut self) {
        self.downloading = true;
        if self.focused == Some(UpdatePopupHit::Update) {
            self.focused = Some(UpdatePopupHit::WhatsNew);
        }
        match self.kind {
            PopupKind::AppUpdate => {
                self.update_label = "Verifying...".into();
                self.subtitle = "Verifying installer SHA-256 checksum...".into();
            }
            PopupKind::DecoderSetup => {
                self.update_label = "Installing...".into();
                self.subtitle = "Verifying SHA-256 and extracting decoder...".into();
            }
        }
        self.layout = self.measure();
    }

    fn set_error(&mut self, error: &str) {
        self.downloading = false;
        self.update_label = "Retry".into();
        self.subtitle = error.to_owned();
        self.layout = self.measure();
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

fn clone_state(hwnd: HWND) -> Option<Rc<RefCell<PopupState>>> {
    // SAFETY: GWLP_USERDATA stores an Rc::into_raw pointer from open_popup until WM_NCDESTROY zeroes it.
    unsafe {
        let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<PopupState>;
        if raw.is_null() {
            return None;
        }
        Rc::increment_strong_count(raw);
        Some(Rc::from_raw(raw))
    }
}

fn post_owner_action(owner: HWND, action_msg: u32, action_id: usize) {
    // SAFETY: posts validated internal command with process-local token to owned panel HWND.
    unsafe {
        let _ = PostMessageW(
            owner,
            action_msg,
            WPARAM(action_id),
            LPARAM(crate::app::internal_action_token()),
        );
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
    let mut origin = POINT::default();
    let ok = unsafe { ClientToScreen(owner, &mut origin).as_bool() };
    if !ok {
        return (100, 100);
    }
    let mut x = origin.x + s(24);
    let mut y = origin.y + s(84);

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
        if let Some(rc) = clone_state(hwnd) {
            let (w, h) = {
                let mut state = rc.borrow_mut();
                let same_tag =
                    state.kind == PopupKind::AppUpdate && state.tag == tag && state.owner == owner;
                if !same_tag {
                    *state = PopupState::new(owner, action_msg, tag, dpi);
                } else if !state.downloading {
                    state.set_ready();
                }
                state.dpi = dpi;
                state.layout = state.measure();
                (state.layout.width, state.layout.height)
            };
            let glass = unsafe { configure_popup_frame(hwnd, w, h, dpi) };
            rc.borrow_mut().glass = glass;
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

    open_popup(owner, PopupState::new(owner, action_msg, tag, dpi), false);
}

pub fn show_decoder(owner: HWND, action_msg: u32) {
    if owner.0.is_null() || unsafe { !IsWindow(owner).as_bool() } {
        return;
    }
    let dpi = unsafe { GetDpiForWindow(owner) }.max(96) as i32;
    if let Some(hwnd) = current_popup() {
        if let Some(rc) = clone_state(hwnd) {
            let (w, h) = {
                let mut state = rc.borrow_mut();
                if state.kind != PopupKind::DecoderSetup || state.owner != owner {
                    *state = PopupState::new_decoder(owner, action_msg, dpi);
                } else if !state.downloading {
                    state.set_ready();
                }
                state.dpi = dpi;
                state.layout = state.measure();
                (state.layout.width, state.layout.height)
            };
            let glass = unsafe { configure_popup_frame(hwnd, w, h, dpi) };
            rc.borrow_mut().glass = glass;
        }
        sync(owner);
        if unsafe { IsWindowVisible(owner).as_bool() && !IsIconic(owner).as_bool() } {
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetFocus(hwnd);
                let _ = RedrawWindow(hwnd, None, None, RDW_INVALIDATE | RDW_UPDATENOW);
            }
        }
        return;
    }

    open_popup(owner, PopupState::new_decoder(owner, action_msg, dpi), true);
}

fn open_popup(owner: HWND, mut state: PopupState, activate_for_keyboard: bool) {
    if ensure_class().is_err() {
        return;
    }
    let Ok(instance) = (unsafe { GetModuleHandleW(None) }) else {
        return;
    };
    let (x, y) = anchor_rect(owner, state.layout.width, state.layout.height, state.dpi);
    let Ok(hwnd) = (unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            POPUP_CLASS,
            w!("WallpaperLive Update"),
            WS_POPUP | WS_TABSTOP,
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
        unsafe { configure_popup_frame(hwnd, state.layout.width, state.layout.height, state.dpi) };
    let rc = Rc::new(RefCell::new(state));
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::into_raw(rc) as isize);
    }
    POPUP_HWND.with(|slot| slot.set(hwnd.0 as isize));
    if unsafe { IsWindowVisible(owner).as_bool() && !IsIconic(owner).as_bool() } {
        unsafe {
            let _ = ShowWindow(
                hwnd,
                if activate_for_keyboard {
                    SW_SHOW
                } else {
                    SW_SHOWNOACTIVATE
                },
            );
            if activate_for_keyboard {
                let _ = SetFocus(hwnd);
            }
            let _ = RedrawWindow(hwnd, None, None, RDW_INVALIDATE | RDW_UPDATENOW);
        }
    }
}

pub fn focus_popup(owner: HWND) -> bool {
    let Some(hwnd) = current_popup() else {
        return false;
    };
    let matches = clone_state(hwnd)
        .is_some_and(|rc| rc.borrow().owner == owner && unsafe { IsWindowVisible(hwnd).as_bool() });
    if !matches {
        return false;
    }
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetFocus(hwnd);
        let _ = InvalidateRect(hwnd, None, false);
    }
    true
}

pub fn set_progress(owner: HWND, percent: Option<u8>) {
    if let Some(hwnd) = current_popup()
        && let Some(rc) = clone_state(hwnd)
    {
        let updated = {
            let mut state = rc.borrow_mut();
            if state.owner == owner && state.kind == PopupKind::AppUpdate {
                state.set_downloading(percent);
                true
            } else {
                false
            }
        };
        if updated {
            unsafe {
                let _ = InvalidateRect(hwnd, None, false);
            }
        }
    }
}

pub fn set_verifying(owner: HWND) {
    if let Some(hwnd) = current_popup()
        && let Some(rc) = clone_state(hwnd)
    {
        let updated = {
            let mut state = rc.borrow_mut();
            if state.owner == owner && state.kind == PopupKind::AppUpdate {
                state.set_verifying();
                true
            } else {
                false
            }
        };
        if updated {
            unsafe {
                let _ = InvalidateRect(hwnd, None, false);
            }
        }
    }
}

pub fn set_error(owner: HWND, error: &str) {
    if let Some(hwnd) = current_popup()
        && let Some(rc) = clone_state(hwnd)
    {
        let updated = {
            let mut state = rc.borrow_mut();
            if state.owner == owner && state.kind == PopupKind::AppUpdate {
                state.set_error(error);
                true
            } else {
                false
            }
        };
        if updated {
            unsafe {
                let _ = InvalidateRect(hwnd, None, false);
            }
        }
    }
}

pub fn set_decoder_progress(owner: HWND, percent: Option<u8>) {
    if let Some(hwnd) = current_popup()
        && let Some(rc) = clone_state(hwnd)
    {
        let updated = {
            let mut state = rc.borrow_mut();
            if state.owner == owner && state.kind == PopupKind::DecoderSetup {
                state.set_downloading(percent);
                true
            } else {
                false
            }
        };
        if updated {
            unsafe {
                let _ = InvalidateRect(hwnd, None, false);
            }
        }
    }
}

pub fn set_decoder_verifying(owner: HWND) {
    if let Some(hwnd) = current_popup()
        && let Some(rc) = clone_state(hwnd)
    {
        let updated = {
            let mut state = rc.borrow_mut();
            if state.owner == owner && state.kind == PopupKind::DecoderSetup {
                state.set_verifying();
                true
            } else {
                false
            }
        };
        if updated {
            unsafe {
                let _ = InvalidateRect(hwnd, None, false);
            }
        }
    }
}

pub fn set_decoder_error(owner: HWND, error: &str) {
    if let Some(hwnd) = current_popup()
        && let Some(rc) = clone_state(hwnd)
    {
        let updated = {
            let mut state = rc.borrow_mut();
            if state.owner == owner && state.kind == PopupKind::DecoderSetup {
                state.set_error(error);
                true
            } else {
                false
            }
        };
        if updated {
            unsafe {
                let _ = InvalidateRect(hwnd, None, false);
            }
        }
    }
}

pub fn dismiss_decoder(owner: HWND) {
    let Some(hwnd) = current_popup() else {
        return;
    };
    let matches = clone_state(hwnd).is_some_and(|rc| {
        let s = rc.borrow();
        s.kind == PopupKind::DecoderSetup && (owner.0.is_null() || s.owner == owner)
    });
    if matches {
        POPUP_HWND.with(|slot| slot.set(0));
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}

pub fn sync(owner: HWND) {
    let Some(hwnd) = current_popup() else {
        return;
    };
    let Some(rc) = clone_state(hwnd) else {
        return;
    };
    if rc.borrow().owner != owner {
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
    let (reconfigure, w, h, eff_dpi) = {
        let mut state = rc.borrow_mut();
        if dpi != state.dpi {
            state.dpi = dpi;
            state.layout = state.measure();
            (true, state.layout.width, state.layout.height, dpi)
        } else {
            (false, state.layout.width, state.layout.height, state.dpi)
        }
    };
    if reconfigure {
        let glass = unsafe { configure_popup_frame(hwnd, w, h, eff_dpi) };
        rc.borrow_mut().glass = glass;
    }
    let (x, y) = anchor_rect(owner, w, h, eff_dpi);
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            HWND::default(),
            x,
            y,
            w,
            h,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

pub fn refresh_theme(owner: HWND) {
    let Some(hwnd) = current_popup() else {
        return;
    };
    let Some(rc) = clone_state(hwnd) else {
        return;
    };
    let dims = {
        let state = rc.borrow();
        (state.owner == owner).then_some((state.layout.width, state.layout.height, state.dpi))
    };
    if let Some((w, h, dpi)) = dims {
        let glass = unsafe { configure_popup_frame(hwnd, w, h, dpi) };
        rc.borrow_mut().glass = glass;
        unsafe {
            let _ = InvalidateRect(hwnd, None, false);
        }
    }
}

pub fn dismiss(owner: HWND) {
    let Some(hwnd) = current_popup() else {
        return;
    };
    let matches_owner = clone_state(hwnd).is_some_and(|rc| {
        let s = rc.borrow();
        owner.0.is_null() || s.owner == owner
    });
    if matches_owner {
        POPUP_HWND.with(|slot| slot.set(0));
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}

pub fn is_open(owner: HWND) -> bool {
    current_popup().is_some_and(|hwnd| {
        clone_state(hwnd).is_some_and(|rc| {
            let s = rc.borrow();
            owner.0.is_null() || s.owner == owner
        })
    })
}

pub fn dismiss_action_for(owner: HWND) -> Option<(u32, usize)> {
    let hwnd = current_popup()?;
    let rc = clone_state(hwnd)?;
    let s = rc.borrow();
    if !owner.0.is_null() && s.owner != owner {
        return None;
    }
    let action_id = match s.kind {
        PopupKind::AppUpdate => UPDATE_DISMISS,
        PopupKind::DecoderSetup => DECODER_DISMISS,
    };
    Some((s.action_msg, action_id))
}

unsafe extern "system" fn popup_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Rc<RefCell<PopupState>> keeps state alive for the full frame; borrows are never held across Win32 calls.
    unsafe {
        if msg == WM_NCDESTROY {
            if POPUP_HWND.with(Cell::get) == hwnd.0 as isize {
                POPUP_HWND.with(|slot| slot.set(0));
            }
            let raw = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if raw != 0 {
                drop(Rc::from_raw(raw as *const RefCell<PopupState>));
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let Some(rc) = clone_state(hwnd) else {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEACTIVATE => LRESULT(MA_ACTIVATE as isize),
            WM_SETFOCUS => {
                if let Ok(mut state) = rc.try_borrow_mut()
                    && state.focused.is_none()
                {
                    state.focused = state.focus_order().first().copied();
                }
                let _ = InvalidateRect(hwnd, None, false);
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                if let Ok(mut state) = rc.try_borrow_mut() {
                    state.focused = None;
                }
                let _ = InvalidateRect(hwnd, None, false);
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                if !dc.0.is_null() {
                    let mut area = RECT::default();
                    let _ = GetClientRect(hwnd, &mut area);
                    let snap = rc.try_borrow().ok().map(|s| {
                        (
                            s.layout,
                            s.title.clone(),
                            s.subtitle.clone(),
                            s.update_label.clone(),
                            s.secondary_label.clone(),
                            s.kind == PopupKind::AppUpdate,
                            s.downloading,
                            s.hover,
                            s.pressed,
                            s.focused,
                            s.dpi,
                            s.glass,
                        )
                    });
                    if let Some((
                        layout,
                        title,
                        subtitle,
                        update_label,
                        secondary_label,
                        is_app_update,
                        downloading,
                        hover,
                        pressed,
                        focused,
                        dpi,
                        glass,
                    )) = snap
                    {
                        let paint = visuals::UpdatePopupPaint {
                            layout: &layout,
                            title: &title,
                            subtitle: &subtitle,
                            update_label: &update_label,
                            secondary_label: &secondary_label,
                            secondary_external: is_app_update,
                            footer_label: is_app_update.then_some("Don't check for updates"),
                            downloading,
                            hover,
                            pressed,
                            focused,
                            dpi,
                            glass,
                        };
                        if !visuals::paint_update_popup_into(dc, area, &paint) {
                            visuals::fill(dc, area, visuals::CARD);
                        }
                    }
                }
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let is_hover = rc.try_borrow().ok().is_some_and(|s| s.hover.is_some());
                let cursor_id = if is_hover { IDC_HAND } else { IDC_ARROW };
                if let Ok(cursor) = LoadCursorW(None, cursor_id) {
                    SetCursor(cursor);
                }
                LRESULT(1)
            }
            WM_MOUSEMOVE => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                let need_track = rc.try_borrow().ok().is_some_and(|s| !s.tracking_leave);
                if need_track {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    if TrackMouseEvent(&mut tme).is_ok()
                        && let Ok(mut state) = rc.try_borrow_mut()
                    {
                        state.tracking_leave = true;
                    }
                }
                let changed = rc.try_borrow_mut().ok().is_some_and(|mut state| {
                    let next = state.layout.hit_test(x, y);
                    if next != state.hover {
                        state.hover = next;
                        true
                    } else {
                        false
                    }
                });
                if changed {
                    let _ = InvalidateRect(hwnd, None, false);
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE_MSG => {
                let redraw = rc.try_borrow_mut().ok().is_some_and(|mut state| {
                    state.tracking_leave = false;
                    if state.hover.is_some() || state.pressed.is_some() {
                        state.hover = None;
                        state.pressed = None;
                        true
                    } else {
                        false
                    }
                });
                if redraw {
                    let _ = InvalidateRect(hwnd, None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                let hit = rc.try_borrow_mut().ok().and_then(|mut state| {
                    let hit = state.layout.hit_test(x, y);
                    if hit.is_some() {
                        state.pressed = hit;
                        state.focused = hit;
                    }
                    hit
                });
                let _ = SetFocus(hwnd);
                if hit.is_some() {
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
                let dispatch = rc.try_borrow_mut().ok().and_then(|mut state| {
                    let pressed = state.pressed.take();
                    let hit = state.layout.hit_test(x, y);
                    pressed
                        .filter(|&target| hit == Some(target))
                        .and_then(|target| state.action_for_hit(target))
                        .map(|action_id| (state.owner, state.action_msg, action_id))
                });
                if GetCapture() == hwnd {
                    let _ = ReleaseCapture();
                }
                let _ = InvalidateRect(hwnd, None, false);
                if let Some((owner, action_msg, action_id)) = dispatch {
                    post_owner_action(owner, action_msg, action_id);
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let vk = wparam.0 as u16;
                if vk == VK_ESCAPE.0 {
                    let dispatch = rc.try_borrow().ok().map(|state| {
                        let action_id = match state.kind {
                            PopupKind::AppUpdate => UPDATE_DISMISS,
                            PopupKind::DecoderSetup => DECODER_DISMISS,
                        };
                        (state.owner, state.action_msg, action_id)
                    });
                    if let Some((owner, action_msg, action_id)) = dispatch {
                        post_owner_action(owner, action_msg, action_id);
                    }
                    return LRESULT(0);
                }
                if vk == VK_TAB.0 {
                    let reverse = GetKeyState(VK_SHIFT.0 as i32) < 0;
                    if let Ok(mut state) = rc.try_borrow_mut() {
                        state.step_focus(reverse);
                    }
                    let _ = InvalidateRect(hwnd, None, false);
                    return LRESULT(0);
                }
                if vk == VK_RETURN.0 || vk == VK_SPACE.0 {
                    let dispatch = rc.try_borrow().ok().and_then(|state| {
                        let target = state
                            .focused
                            .or_else(|| state.focus_order().first().copied())?;
                        state
                            .action_for_hit(target)
                            .map(|action_id| (state.owner, state.action_msg, action_id))
                    });
                    if let Some((owner, action_msg, action_id)) = dispatch {
                        post_owner_action(owner, action_msg, action_id);
                    }
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
