use windows::{
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, POINT, SIZE, WPARAM},
        Graphics::Gdi::*,
        System::Registry::*,
        UI::{
            Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW, MSAA_MENU_SIG, MSAAMENUINFO},
            Controls::{
                DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_GRAYED,
                ODS_SELECTED, ODT_MENU,
            },
            HiDpi::{
                GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI, SystemParametersInfoForDpi,
            },
            Shell::{NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect},
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, PWSTR, w},
};

#[derive(Clone, Copy)]
struct Palette {
    background: COLORREF,
    text: COLORREF,
    selected: COLORREF,
    disabled: COLORREF,
}
impl Palette {
    fn new(dark: bool) -> Self {
        if dark {
            Self {
                background: COLORREF(0x202020),
                text: COLORREF(0xf3f3f3),
                selected: COLORREF(0x3b3b3b),
                disabled: COLORREF(0xa6a6a6),
            }
        } else {
            Self {
                background: COLORREF(0xffffff),
                text: COLORREF(0x1f1f1f),
                selected: COLORREF(0xe9e9e9),
                disabled: COLORREF(0x666666),
            }
        }
    }
    fn app_surface() -> Self {
        Self {
            // Match WallpaperLive's dark surface (#232221 / #F3EEE3 / #353331) using COLORREF BGR.
            background: COLORREF(0x212223),
            text: COLORREF(0xe3eef3),
            selected: COLORREF(0x313335),
            disabled: COLORREF(0x888f93),
        }
    }
}
fn system_palette(force_app_dark: bool) -> Option<Palette> {
    let mut contrast = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    // SAFETY: read-only system preferences; high contrast retains native menu rendering.
    unsafe {
        if SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            Some((&mut contrast as *mut HIGHCONTRASTW).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_err()
            || contrast.dwFlags.contains(HCF_HIGHCONTRASTON)
        {
            return None;
        }
        if force_app_dark {
            return Some(Palette::app_surface());
        }
        let mut light = 1u32;
        let mut bytes = std::mem::size_of_val(&light) as u32;
        let result = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut light as *mut u32).cast()),
            Some(&mut bytes),
        );
        Some(Palette::new(result.is_ok() && bytes == 4 && light == 0))
    }
}

#[repr(C)]
struct Entry {
    // Windows accessibility reads this first member for owner-drawn menu names.
    accessible: MSAAMENUINFO,
    text: Vec<u16>,
    font: HFONT,
    colors: Palette,
    width: u32,
    height: u32,
    padding: i32,
    checked: bool,
}

#[derive(Clone)]
struct ActiveMenuEntry {
    text: Vec<u16>,
    font: HFONT,
    colors: Palette,
    width: u32,
    height: u32,
    padding: i32,
    checked: bool,
}

struct ActiveMenuState {
    menu_open: bool,
    entries: Vec<ActiveMenuEntry>,
}

thread_local! {
    static ACTIVE_MENU: std::cell::RefCell<Option<ActiveMenuState>> =
        const { std::cell::RefCell::new(None) };
}

struct Menu {
    handle: HMENU,
    #[allow(clippy::vec_box)]
    entries: Vec<Box<Entry>>,
    brush: HBRUSH,
    font: HFONT,
    registered_active: bool,
}
impl Drop for Menu {
    fn drop(&mut self) {
        if self.registered_active {
            ACTIVE_MENU.with(|slot| *slot.borrow_mut() = None);
        }
        // SAFETY: menu is destroyed before its borrowed names, font and background brush.
        unsafe {
            let _ = DestroyMenu(self.handle);
            if !self.brush.0.is_null() {
                let _ = DeleteObject(self.brush);
            }
            if !self.font.0.is_null() {
                let _ = DeleteObject(self.font);
            }
        }
    }
}
impl Menu {
    fn new(
        owner: HWND,
        items: &[(usize, &str)],
        selected_id: Option<usize>,
        colors: Option<Palette>,
        dpi: u32,
    ) -> Result<Self, String> {
        // SAFETY: resources have one owner; RAII also covers partially built menus.
        unsafe {
            let mut menu = Self {
                handle: CreatePopupMenu().map_err(|e| e.to_string())?,
                entries: Vec::with_capacity(items.len()),
                brush: HBRUSH::default(),
                font: HFONT::default(),
                registered_active: false,
            };
            if let Some(colors) = colors {
                let mut metrics = NONCLIENTMETRICSW {
                    cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
                    ..Default::default()
                };
                SystemParametersInfoForDpi(
                    SPI_GETNONCLIENTMETRICS.0,
                    metrics.cbSize,
                    Some((&mut metrics as *mut NONCLIENTMETRICSW).cast()),
                    0,
                    dpi,
                )
                .map_err(|e| e.to_string())?;
                menu.font = CreateFontIndirectW(&metrics.lfMenuFont);
                menu.brush = CreateSolidBrush(colors.background);
                if menu.font.0.is_null() || menu.brush.0.is_null() {
                    return Err("Could not create system menu resources".into());
                }
                SetMenuInfo(
                    menu.handle,
                    &MENUINFO {
                        cbSize: std::mem::size_of::<MENUINFO>() as u32,
                        fMask: MIM_BACKGROUND | MIM_STYLE,
                        dwStyle: MNS_NOCHECK,
                        hbrBack: menu.brush,
                        ..Default::default()
                    },
                )
                .map_err(|e| e.to_string())?;
                let check_gutter = selected_id.is_some();
                let dc = GetDC(owner);
                if dc.0.is_null() {
                    return Err("Could not measure tray menu text".into());
                }
                let old = SelectObject(dc, menu.font);
                let scale = |n| n * dpi as i32 / 96;
                let pad_left = scale(if check_gutter { 26 } else { 16 });
                let pad_right = scale(16);
                let mut active_entries = Vec::with_capacity(items.len());
                let measure_result = (|| -> Result<(), String> {
                    for (idx, (id, label)) in items.iter().enumerate() {
                        let mut text: Vec<_> = label.encode_utf16().chain(Some(0)).collect();
                        let mut size = SIZE::default();
                        if !GetTextExtentPoint32W(dc, &text[..text.len() - 1], &mut size).as_bool()
                        {
                            return Err("Could not measure tray menu text".into());
                        }
                        let width = (size.cx + pad_left + pad_right) as u32;
                        let height = (size.cy.max(scale(16)) + scale(12)) as u32;
                        let checked = selected_id == Some(*id);
                        active_entries.push(ActiveMenuEntry {
                            text: text.clone(),
                            font: menu.font,
                            colors,
                            width,
                            height,
                            padding: pad_left,
                            checked,
                        });
                        let entry = Box::new(Entry {
                            accessible: MSAAMENUINFO {
                                dwMSAASignature: MSAA_MENU_SIG as u32,
                                cchWText: (text.len() - 1) as u32,
                                pszWText: PWSTR(text.as_mut_ptr()),
                            },
                            text,
                            font: menu.font,
                            colors,
                            width,
                            height,
                            padding: pad_left,
                            checked,
                        });
                        let info = MENUITEMINFOW {
                            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                            fMask: MIIM_ID | MIIM_FTYPE | MIIM_DATA | MIIM_STRING,
                            fType: MFT_OWNERDRAW,
                            wID: *id as u32,
                            // Store 1-based validated index, never a raw pointer dereferenced in WndProc.
                            dwItemData: idx + 1,
                            dwTypeData: entry.accessible.pszWText,
                            cch: entry.accessible.cchWText,
                            ..Default::default()
                        };
                        InsertMenuItemW(menu.handle, *id as u32, false, &info)
                            .map_err(|e| e.to_string())?;
                        menu.entries.push(entry);
                    }
                    Ok(())
                })();
                SelectObject(dc, old);
                ReleaseDC(owner, dc);
                measure_result?;
                ACTIVE_MENU.with(|slot| {
                    *slot.borrow_mut() = Some(ActiveMenuState {
                        menu_open: true,
                        entries: active_entries,
                    });
                });
                menu.registered_active = true;
            } else {
                for (id, label) in items {
                    let text: Vec<_> = label.encode_utf16().chain(Some(0)).collect();
                    let flags = if selected_id == Some(*id) {
                        MF_STRING | MF_CHECKED
                    } else {
                        MF_STRING
                    };
                    AppendMenuW(menu.handle, flags, *id, PCWSTR(text.as_ptr()))
                        .map_err(|e| e.to_string())?;
                }
            }
            Ok(menu)
        }
    }
}

fn anchor(payload: WPARAM) -> POINT {
    POINT {
        x: payload.0 as u16 as i16 as i32,
        y: (payload.0 >> 16) as u16 as i16 as i32,
    }
}
fn track_menu(
    owner: HWND,
    point: POINT,
    items: &[(usize, &str)],
    selected_id: Option<usize>,
    force_app_dark: bool,
) -> Result<usize, String> {
    let mut dpi = unsafe { GetDpiForWindow(owner) }.max(96);
    let mut dpi_y = dpi;
    let _ = unsafe {
        GetDpiForMonitor(
            MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST),
            MDT_EFFECTIVE_DPI,
            &mut dpi,
            &mut dpi_y,
        )
    };
    let menu = Menu::new(
        owner,
        items,
        selected_id,
        system_palette(force_app_dark),
        dpi.max(96),
    )?;
    // SAFETY: native menu retains keyboard navigation; entry data lives throughout modal tracking.
    unsafe {
        let _ = SetForegroundWindow(owner);
        let id = TrackPopupMenu(
            menu.handle,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            point.x,
            point.y,
            0,
            owner,
            None,
        )
        .0 as usize;
        let _ = PostMessageW(owner, WM_NULL, WPARAM(0), LPARAM(0));
        Ok(id)
    }
}
pub fn popup(owner: HWND, items_anchor: WPARAM, items: &[(usize, &str)]) -> Result<usize, String> {
    // Read preferences on every open: no cached theme, registry writes or polling.
    // SAFETY: only this app's tray icon and monitor metadata are queried.
    let mut point = anchor(items_anchor);
    if point.x == -1 && point.y == -1 {
        let rect = unsafe {
            Shell_NotifyIconGetRect(&NOTIFYICONIDENTIFIER {
                cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
                hWnd: owner,
                uID: 1,
                ..Default::default()
            })
        }
        .map_err(|e| format!("Could not locate the tray icon: {e}"))?;
        point = POINT {
            x: (rect.left + rect.right) / 2,
            y: (rect.top + rect.bottom) / 2,
        };
    }
    track_menu(owner, point, items, None, false)
}
pub fn popup_at(
    owner: HWND,
    point: POINT,
    items: &[(usize, &str)],
    selected_id: Option<usize>,
) -> Result<usize, String> {
    dropdown_at(
        owner,
        windows::Win32::Foundation::RECT {
            left: point.x,
            top: point.y,
            right: point.x,
            bottom: point.y,
        },
        items,
        selected_id,
    )
}

/// Native menu entry for anchored rounded Acrylic dropdowns.
#[derive(Clone, Copy)]
pub struct MenuEntry<'a> {
    pub id: usize,
    pub label: &'a str,
    pub checked: bool,
    pub separator_before: bool,
}

/// Shows an anchored, rounded Acrylic glassmorphic dropdown below (or above) `anchor`.
/// Falls back to the system menu when Windows High Contrast mode is active.
pub fn dropdown_at(
    owner: HWND,
    anchor: windows::Win32::Foundation::RECT,
    items: &[(usize, &str)],
    selected_id: Option<usize>,
) -> Result<usize, String> {
    if items.is_empty() {
        return Ok(0);
    }
    if system_palette(true).is_none() {
        return track_menu(
            owner,
            POINT {
                x: anchor.left,
                y: anchor.bottom,
            },
            items,
            selected_id,
            true,
        );
    }
    let entries: Vec<MenuEntry<'_>> = items
        .iter()
        .map(|&(id, label)| MenuEntry {
            id,
            label,
            checked: selected_id == Some(id),
            separator_before: false,
        })
        .collect();
    dropdown::show_entries(owner, anchor, &entries, selected_id.is_some(), true)
}

/// Shows an anchored Acrylic toggle/action menu with independent checkmarks and optional dividers.
pub fn menu_at(
    owner: HWND,
    anchor: windows::Win32::Foundation::RECT,
    entries: &[MenuEntry<'_>],
) -> Result<usize, String> {
    if entries.is_empty() {
        return Ok(0);
    }
    if system_palette(true).is_none() {
        let pairs: Vec<(usize, &str)> = entries.iter().map(|e| (e.id, e.label)).collect();
        return track_menu(
            owner,
            POINT {
                x: anchor.left,
                y: anchor.bottom,
            },
            &pairs,
            None,
            true,
        );
    }
    dropdown::show_entries(owner, anchor, entries, true, false)
}

mod dropdown {
    use std::cell::RefCell;
    use std::mem::size_of;
    use std::sync::Once;
    use std::time::Instant;

    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Dwm::{
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CLIP_DEFAULT_PRECIS, CreateFontW, CreateRoundRectRgn, DEFAULT_CHARSET,
        DEFAULT_PITCH, DEFAULT_QUALITY, DeleteObject, EndPaint, FF_DONTCARE, FW_NORMAL, GetDC,
        GetMonitorInfoW, GetTextExtentPoint32W, InvalidateRect, MONITOR_DEFAULTTONEAREST,
        MONITORINFO, MonitorFromPoint, OUT_DEFAULT_PRECIS, PAINTSTRUCT, ReleaseDC, SelectObject,
        SetWindowRgn,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetCapture, ReleaseCapture, SetCapture, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_RETURN,
        VK_SPACE, VK_TAB, VK_UP,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
        GetClientRect, GetMessageW, GetWindowLongPtrW, IDC_ARROW, KillTimer, LoadCursorW,
        MA_NOACTIVATE, MSG, RegisterClassW, RemovePropW, SW_SHOWNOACTIVATE, SetTimer,
        SetWindowLongPtrW, ShowWindow, TranslateMessage, WA_INACTIVE, WM_ACTIVATE, WM_CANCELMODE,
        WM_CAPTURECHANGED, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN,
        WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_MOUSEWHEEL,
        WM_NCLBUTTONDOWN, WM_NCRBUTTONDOWN, WM_PAINT, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_TIMER,
        WNDCLASS_STYLES, WNDCLASSW, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };
    use windows::core::{PCWSTR, w};

    const REVEAL_TIMER_ID: usize = 1;
    const REVEAL_MS: u32 = 120;
    const MAX_VISIBLE_ROWS: usize = 8;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) struct Geometry {
        pub rect: RECT,
        pub item_h: i32,
        pub pad_y: i32,
        pub visible_rows: usize,
    }

    #[inline]
    fn scale_px(px: i32, dpi: i32) -> i32 {
        px * dpi.max(96) / 96
    }

    pub(crate) fn compute_geometry(
        anchor: RECT,
        work: RECT,
        item_count: usize,
        measured_w: i32,
        dpi: i32,
    ) -> Geometry {
        let count = item_count.max(1);
        let visible_rows = count.min(MAX_VISIBLE_ROWS);
        let item_h = scale_px(34, dpi).max(24);
        let pad_y = scale_px(6, dpi).max(4);
        let gap = scale_px(4, dpi).max(2);
        let work_w = (work.right - work.left).max(160);
        let work_h = (work.bottom - work.top).max(item_h + pad_y * 2);

        let anchor_w = (anchor.right - anchor.left).max(0);
        let min_w = anchor_w.max(scale_px(184, dpi));
        let max_w = (work_w - scale_px(16, dpi)).max(min_w.min(work_w));
        let width = measured_w.max(min_w).min(max_w);

        let desired_h = visible_rows as i32 * item_h + pad_y * 2;
        let height = desired_h.min(work_h);

        let x_right_clamped = anchor.left.min(work.right - width);
        let left = x_right_clamped.max(work.left);

        let top = if anchor.bottom + gap + height <= work.bottom {
            anchor.bottom + gap
        } else if anchor.top - gap - height >= work.top {
            anchor.top - gap - height
        } else {
            (work.bottom - height).max(work.top)
        };

        Geometry {
            rect: RECT {
                left,
                top,
                right: left + width,
                bottom: top + height,
            },
            item_h,
            pad_y,
            visible_rows,
        }
    }

    struct State<'a> {
        items: &'a [&'a str],
        checked: &'a [bool],
        separator_before: &'a [bool],
        has_check_gutter: bool,
        highlight_checked: bool,
        checked_index: Option<usize>,
        hover_index: Option<usize>,
        pressed_inside: bool,
        scroll_top: usize,
        item_h: i32,
        pad_y: i32,
        visible_rows: usize,
        dpi: i32,
        glass: bool,
        animate: bool,
        opened_at: Instant,
        reveal_t: f32,
        chosen_index: Option<usize>,
        open: bool,
    }

    impl<'a> State<'a> {
        fn row_at(&self, width: i32, x: i32, y: i32) -> Option<usize> {
            if x < 0 || x >= width || y < self.pad_y {
                return None;
            }
            let rel = y - self.pad_y;
            let row = (rel / self.item_h) as usize;
            if row >= self.visible_rows {
                return None;
            }
            let idx = self.scroll_top + row;
            (idx < self.items.len()).then_some(idx)
        }

        fn ensure_visible(&mut self, index: usize) {
            if index < self.scroll_top {
                self.scroll_top = index;
            } else if index >= self.scroll_top + self.visible_rows {
                self.scroll_top = index + 1 - self.visible_rows;
            }
        }

        fn step_selection(&mut self, delta: isize) {
            if self.items.is_empty() {
                return;
            }
            let len = self.items.len() as isize;
            let base = self
                .hover_index
                .or(self.checked_index)
                .map(|i| i as isize)
                .unwrap_or(if delta >= 0 { -1 } else { 0 });
            let next = (base + delta).rem_euclid(len) as usize;
            self.hover_index = Some(next);
            self.ensure_visible(next);
        }

        fn finish(&mut self, chosen: Option<usize>) {
            if !self.open {
                return;
            }
            self.chosen_index = chosen;
            self.open = false;
        }
    }

    unsafe fn state_cell<'a>(hwnd: HWND) -> Option<&'a RefCell<State<'a>>> {
        // SAFETY: GWLP_USERDATA holds a shared RefCell<State> pointer until WM_DESTROY/exit.
        let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<State<'a>> };
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { &*ptr })
        }
    }

    unsafe fn measure_required_width(owner: HWND, labels: &[&str], dpi: i32) -> i32 {
        let mut max_w = 0i32;
        // SAFETY: temporary screen/owner DC and GDI font released immediately below.
        unsafe {
            let dc = GetDC(owner);
            if !dc.0.is_null() {
                let font_h = -scale_px(13, dpi).max(11);
                let font = CreateFontW(
                    font_h,
                    0,
                    0,
                    0,
                    FW_NORMAL.0 as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET.0 as u32,
                    OUT_DEFAULT_PRECIS.0 as u32,
                    CLIP_DEFAULT_PRECIS.0 as u32,
                    DEFAULT_QUALITY.0 as u32,
                    (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                    w!("Segoe UI Variable Text"),
                );
                let prev = if !font.0.is_null() {
                    Some(SelectObject(dc, font))
                } else {
                    None
                };
                for label in labels {
                    let wide: Vec<u16> = label.encode_utf16().collect();
                    if !wide.is_empty() {
                        let mut sz = std::mem::zeroed();
                        if GetTextExtentPoint32W(dc, &wide, &mut sz).as_bool() {
                            max_w = max_w.max(sz.cx);
                        }
                    }
                }
                if let Some(old) = prev {
                    let _ = SelectObject(dc, old);
                }
                if !font.0.is_null() {
                    let _ = DeleteObject(font);
                }
                let _ = ReleaseDC(owner, dc);
            }
        }
        max_w + scale_px(76, dpi)
    }

    unsafe fn configure_backdrop(hwnd: HWND, w: i32, h: i32, dpi: i32) -> bool {
        // SAFETY: delegates DWM Acrylic setup to visuals::enable_backdrop and rounds Win10 regions if needed.
        unsafe {
            let glass = crate::visuals::enable_backdrop(hwnd);
            let corner = DWMWCP_ROUND.0;
            if DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&corner as *const i32).cast(),
                size_of::<i32>() as u32,
            )
            .is_err()
            {
                let d = scale_px(12, dpi);
                let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, d, d);
                if !rgn.0.is_null() && SetWindowRgn(hwnd, rgn, true) == 0 {
                    let _ = DeleteObject(rgn);
                }
            }
            glass
        }
    }

    struct PaintSnapshot<'a> {
        items: &'a [&'a str],
        checked: &'a [bool],
        separator_before: &'a [bool],
        has_check_gutter: bool,
        highlight_checked: bool,
        hover_index: Option<usize>,
        item_h: i32,
        scroll_top: usize,
        reveal_t: f32,
        dpi: i32,
        glass: bool,
    }

    unsafe fn paint_popup(hwnd: HWND, snap: &PaintSnapshot<'_>) {
        // SAFETY: BeginPaint/EndPaint wraps buffered Win32 painting inside visuals::paint_dropdown_into.
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if hdc.0.is_null() {
                return;
            }
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            if !crate::visuals::paint_dropdown_into(
                hdc,
                client,
                snap.items,
                snap.checked,
                snap.separator_before,
                snap.has_check_gutter,
                snap.highlight_checked,
                snap.hover_index,
                snap.item_h,
                snap.scroll_top,
                snap.reveal_t,
                snap.dpi,
                snap.glass,
            ) {
                crate::visuals::fill(hdc, client, crate::visuals::CARD);
            }
            let _ = EndPaint(hwnd, &ps);
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // SAFETY: accesses live RefCell<State> with short non-overlapping borrows, never held across Win32 calls.
        unsafe {
            if msg == WM_DESTROY {
                let _ = KillTimer(hwnd, REVEAL_TIMER_ID);
                let _ = RemovePropW(hwnd, w!("WallpaperLive.Glass"));
                let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                return LRESULT(0);
            }
            let Some(cell) = state_cell(hwnd) else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let width = client.right - client.left;

            match msg {
                WM_ERASEBKGND => LRESULT(1),
                WM_PAINT => {
                    let snap = cell.try_borrow().ok().map(|s| PaintSnapshot {
                        items: s.items,
                        checked: s.checked,
                        separator_before: s.separator_before,
                        has_check_gutter: s.has_check_gutter,
                        highlight_checked: s.highlight_checked,
                        hover_index: s.hover_index,
                        item_h: s.item_h,
                        scroll_top: s.scroll_top,
                        reveal_t: s.reveal_t,
                        dpi: s.dpi,
                        glass: s.glass,
                    });
                    if let Some(snap) = snap {
                        paint_popup(hwnd, &snap);
                    }
                    LRESULT(0)
                }
                WM_TIMER if wparam.0 == REVEAL_TIMER_ID => {
                    let stop_timer = if let Ok(mut state) = cell.try_borrow_mut() {
                        let elapsed = state.opened_at.elapsed().as_secs_f32() * 1000.0;
                        state.reveal_t = (elapsed / REVEAL_MS as f32).clamp(0.0, 1.0);
                        if state.reveal_t >= 1.0 {
                            state.animate = false;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if stop_timer {
                        let _ = KillTimer(hwnd, REVEAL_TIMER_ID);
                    }
                    let _ = InvalidateRect(hwnd, None, false);
                    LRESULT(0)
                }
                WM_MOUSEMOVE => {
                    let x = (lparam.0 & 0xffff) as i16 as i32;
                    let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                    let changed = if let Ok(mut state) = cell.try_borrow_mut() {
                        let next = state.row_at(width, x, y);
                        if next != state.hover_index {
                            state.hover_index = next;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if changed {
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                    LRESULT(0)
                }
                WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
                WM_LBUTTONDOWN => {
                    let x = (lparam.0 & 0xffff) as i16 as i32;
                    let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                    let (want_capture, want_redraw) = if let Ok(mut state) = cell.try_borrow_mut() {
                        if let Some(idx) = state.row_at(width, x, y) {
                            state.pressed_inside = true;
                            let redraw = state.hover_index != Some(idx);
                            if redraw {
                                state.hover_index = Some(idx);
                            }
                            (true, redraw)
                        } else {
                            if x < 0 || x >= width || y < 0 || y >= client.bottom {
                                state.finish(None);
                            }
                            (false, false)
                        }
                    } else {
                        (false, false)
                    };
                    if want_capture && GetCapture() != hwnd {
                        let _ = SetCapture(hwnd);
                    }
                    if want_redraw {
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                    LRESULT(0)
                }
                WM_LBUTTONUP => {
                    let x = (lparam.0 & 0xffff) as i16 as i32;
                    let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                    if let Ok(mut state) = cell.try_borrow_mut() {
                        let row = state.row_at(width, x, y);
                        if state.pressed_inside {
                            state.finish(row);
                        } else if let Some(idx) = row {
                            state.finish(Some(idx));
                        }
                    }
                    LRESULT(0)
                }
                WM_RBUTTONDOWN | WM_NCLBUTTONDOWN | WM_NCRBUTTONDOWN | WM_CANCELMODE => {
                    if let Ok(mut state) = cell.try_borrow_mut() {
                        state.finish(None);
                    }
                    LRESULT(0)
                }
                WM_CAPTURECHANGED => {
                    if HWND(lparam.0 as *mut std::ffi::c_void) != hwnd
                        && let Ok(mut state) = cell.try_borrow_mut()
                        && state.open
                    {
                        state.finish(None);
                    }
                    LRESULT(0)
                }
                WM_MOUSEWHEEL => {
                    let delta = ((wparam.0 >> 16) & 0xffff) as i16 as i32;
                    let redraw = if let Ok(mut state) = cell.try_borrow_mut() {
                        let max_scroll = state.items.len().saturating_sub(state.visible_rows);
                        if max_scroll > 0 {
                            let step = if delta > 0 { -1isize } else { 1isize };
                            let next_top = (state.scroll_top as isize + step)
                                .clamp(0, max_scroll as isize)
                                as usize;
                            if next_top != state.scroll_top {
                                state.scroll_top = next_top;
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if redraw {
                        let _ = InvalidateRect(hwnd, None, false);
                    }
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
    }

    fn ensure_class(instance: HINSTANCE) -> PCWSTR {
        static INIT: Once = Once::new();
        let name = w!("WallpaperLive.Dropdown");
        INIT.call_once(|| {
            // SAFETY: registers the lightweight dropdown window class once per process.
            unsafe {
                let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
                let wc = WNDCLASSW {
                    style: WNDCLASS_STYLES(0),
                    lpfnWndProc: Some(wnd_proc),
                    hInstance: instance,
                    hCursor: cursor,
                    lpszClassName: name,
                    ..Default::default()
                };
                let _ = RegisterClassW(&wc);
            }
        });
        name
    }

    pub(crate) fn show_entries(
        owner: HWND,
        anchor: RECT,
        entries: &[super::MenuEntry<'_>],
        has_check_gutter: bool,
        highlight_checked: bool,
    ) -> Result<usize, String> {
        let labels: Vec<&str> = entries.iter().map(|entry| entry.label).collect();
        let checked: Vec<bool> = entries.iter().map(|entry| entry.checked).collect();
        let separator_before: Vec<bool> =
            entries.iter().map(|entry| entry.separator_before).collect();
        let checked_index = if highlight_checked {
            checked.iter().position(|&c| c)
        } else {
            None
        };

        // SAFETY: Win32 popup lifecycle bounded to this function; State outlives popup window.
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None)
                .map_err(|err| format!("failed to get module handle: {err}"))?
                .into();
            let class_name = ensure_class(instance);
            let dpi = GetDpiForWindow(owner) as i32;
            let dpi = if dpi > 0 { dpi } else { 96 };

            let pt = POINT {
                x: anchor.left,
                y: anchor.bottom,
            };
            let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let work = if GetMonitorInfoW(monitor, &mut mi).as_bool() {
                mi.rcWork
            } else {
                RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                }
            };

            let measured_w = measure_required_width(owner, &labels, dpi);
            let geom = compute_geometry(anchor, work, labels.len(), measured_w, dpi);
            let w = geom.rect.right - geom.rect.left;
            let h = geom.rect.bottom - geom.rect.top;

            let scroll_top = checked_index
                .map(|idx| idx.saturating_sub(geom.visible_rows / 2))
                .unwrap_or(0)
                .min(labels.len().saturating_sub(geom.visible_rows));

            let animate = crate::visuals::animations_enabled(owner);
            let state = RefCell::new(State {
                items: &labels,
                checked: &checked,
                separator_before: &separator_before,
                has_check_gutter,
                highlight_checked,
                checked_index,
                hover_index: checked_index,
                pressed_inside: false,
                scroll_top,
                item_h: geom.item_h,
                pad_y: geom.pad_y,
                visible_rows: geom.visible_rows,
                dpi,
                glass: false,
                animate,
                opened_at: Instant::now(),
                reveal_t: if animate { 0.0 } else { 1.0 },
                chosen_index: None,
                open: true,
            });

            let popup = CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class_name,
                w!(""),
                WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                geom.rect.left,
                geom.rect.top,
                w,
                h,
                owner,
                None,
                instance,
                None,
            )
            .map_err(|err| format!("failed to create dropdown window: {err}"))?;

            let _ = SetWindowLongPtrW(
                popup,
                GWLP_USERDATA,
                (&state as *const RefCell<State<'_>>) as isize,
            );
            let glass = configure_backdrop(popup, w, h, dpi);
            state.borrow_mut().glass = glass;
            let _ = ShowWindow(popup, SW_SHOWNOACTIVATE);
            if animate {
                let _ = SetTimer(popup, REVEAL_TIMER_ID, 16, None);
            }
            let _ = SetCapture(popup);

            let mut msg = MSG::default();
            while state.borrow().open && GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                if msg.hwnd == owner
                    && msg.message == WM_ACTIVATE
                    && (msg.wParam.0 & 0xffff) as u32 == WA_INACTIVE
                {
                    state.borrow_mut().finish(None);
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageW(&msg);
                    break;
                }
                if matches!(msg.message, WM_KEYDOWN | WM_SYSKEYDOWN) {
                    let vk = msg.wParam.0 as u16;
                    match vk {
                        x if x == VK_ESCAPE.0 || x == VK_TAB.0 => {
                            state.borrow_mut().finish(None);
                            continue;
                        }
                        x if x == VK_RETURN.0 || x == VK_SPACE.0 => {
                            let mut s = state.borrow_mut();
                            let chosen = s.hover_index.or(s.checked_index);
                            s.finish(chosen);
                            continue;
                        }
                        x if x == VK_UP.0 => {
                            state.borrow_mut().step_selection(-1);
                            let _ = InvalidateRect(popup, None, false);
                            continue;
                        }
                        x if x == VK_DOWN.0 => {
                            state.borrow_mut().step_selection(1);
                            let _ = InvalidateRect(popup, None, false);
                            continue;
                        }
                        x if x == VK_HOME.0 => {
                            let redraw = {
                                let mut s = state.borrow_mut();
                                if !s.items.is_empty() {
                                    s.hover_index = Some(0);
                                    s.ensure_visible(0);
                                    true
                                } else {
                                    false
                                }
                            };
                            if redraw {
                                let _ = InvalidateRect(popup, None, false);
                            }
                            continue;
                        }
                        x if x == VK_END.0 => {
                            let redraw = {
                                let mut s = state.borrow_mut();
                                if !s.items.is_empty() {
                                    let last = s.items.len() - 1;
                                    s.hover_index = Some(last);
                                    s.ensure_visible(last);
                                    true
                                } else {
                                    false
                                }
                            };
                            if redraw {
                                let _ = InvalidateRect(popup, None, false);
                            }
                            continue;
                        }
                        _ => {}
                    }
                }
                if msg.message == WM_MOUSEWHEEL && msg.hwnd != popup {
                    let delta = ((msg.wParam.0 >> 16) & 0xffff) as i16 as i32;
                    let redraw = {
                        let mut s = state.borrow_mut();
                        let max_scroll = s.items.len().saturating_sub(s.visible_rows);
                        if max_scroll > 0 {
                            let step = if delta > 0 { -1isize } else { 1isize };
                            let next_top = (s.scroll_top as isize + step)
                                .clamp(0, max_scroll as isize)
                                as usize;
                            if next_top != s.scroll_top {
                                s.scroll_top = next_top;
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    };
                    if redraw {
                        let _ = InvalidateRect(popup, None, false);
                    }
                    continue;
                }
                if msg.hwnd != popup
                    && matches!(
                        msg.message,
                        WM_LBUTTONDOWN
                            | WM_RBUTTONDOWN
                            | WM_MBUTTONDOWN
                            | WM_NCLBUTTONDOWN
                            | WM_NCRBUTTONDOWN
                    )
                {
                    state.borrow_mut().finish(None);
                    continue;
                }
                // Drop forged posted system pointer messages before dispatching.
                if crate::app::is_disallowed_queued_pointer_msg(msg.message) {
                    continue;
                }
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
                if msg.hwnd == owner && matches!(msg.message, WM_CLOSE | WM_DESTROY) {
                    state.borrow_mut().finish(None);
                }
            }

            let _ = SetWindowLongPtrW(popup, GWLP_USERDATA, 0);
            if GetCapture() == popup {
                let _ = ReleaseCapture();
            }
            let _ = DestroyWindow(popup);
            let chosen_index = state.borrow().chosen_index;
            Ok(chosen_index
                .and_then(|idx| entries.get(idx))
                .map(|entry| entry.id)
                .unwrap_or(0))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn dropdown_matches_button_width_and_flips_above_work_area() {
            let work = RECT {
                left: 0,
                top: 0,
                right: 1280,
                bottom: 720,
            };
            let wide_anchor = RECT {
                left: 100,
                top: 160,
                right: 520,
                bottom: 194,
            };
            let g_below = compute_geometry(wide_anchor, work, 4, 240, 96);
            assert_eq!(g_below.rect.right - g_below.rect.left, 420);
            assert_eq!(g_below.rect.left, 100);
            assert!(g_below.rect.top >= wide_anchor.bottom);

            let bottom_anchor = RECT {
                left: 1120,
                top: 660,
                right: 1260,
                bottom: 694,
            };
            let g_above = compute_geometry(bottom_anchor, work, 5, 220, 96);
            assert!(g_above.rect.bottom <= bottom_anchor.top);
            assert!(g_above.rect.right <= work.right);
            assert!(g_above.rect.top >= work.top);
        }

        #[test]
        fn dropdown_click_selection_survives_capture_release_and_never_activates()
        -> Result<(), String> {
            use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

            let labels = ["All folders", r"D:\wallpapers\live"];
            let checked = [false, true];
            let separators = [false, false];
            let state = RefCell::new(State {
                items: &labels,
                checked: &checked,
                separator_before: &separators,
                has_check_gutter: true,
                highlight_checked: true,
                checked_index: Some(1),
                hover_index: Some(1),
                pressed_inside: false,
                scroll_top: 0,
                item_h: 34,
                pad_y: 6,
                visible_rows: 2,
                dpi: 96,
                glass: false,
                animate: false,
                opened_at: Instant::now(),
                reveal_t: 1.0,
                chosen_index: None,
                open: true,
            });
            state.borrow_mut().finish(Some(1));
            state.borrow_mut().finish(None);
            assert_eq!(state.borrow().chosen_index, Some(1));
            assert!(!state.borrow().open);

            {
                let mut s = state.borrow_mut();
                s.chosen_index = None;
                s.open = true;
            }

            // SAFETY: offscreen no-activate popups created and destroyed within this test only.
            unsafe {
                let instance: HINSTANCE = GetModuleHandleW(None).map_err(|e| e.to_string())?.into();
                let class_name = ensure_class(instance);
                let owner = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                    class_name,
                    w!(""),
                    WS_POPUP,
                    -32000,
                    -32000,
                    220,
                    120,
                    HWND::default(),
                    None,
                    instance,
                    None,
                )
                .map_err(|e| e.to_string())?;
                let popup = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                    class_name,
                    w!(""),
                    WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                    -32000,
                    -32000,
                    220,
                    80,
                    owner,
                    None,
                    instance,
                    None,
                )
                .map_err(|e| e.to_string())?;
                let _ = SetWindowLongPtrW(
                    popup,
                    GWLP_USERDATA,
                    (&state as *const RefCell<State<'_>>) as isize,
                );
                let _ = ShowWindow(popup, SW_SHOWNOACTIVATE);
                let _ = SetCapture(popup);

                let ma = SendMessageW(
                    popup,
                    WM_MOUSEACTIVATE,
                    WPARAM(owner.0 as usize),
                    LPARAM(0x0201_0001),
                );
                assert_eq!(ma.0, MA_NOACTIVATE as isize);

                let (pad_y, item_h) = {
                    let s = state.borrow();
                    (s.pad_y, s.item_h)
                };
                let click_lp = LPARAM((((pad_y + item_h / 2) << 16) | 40) as isize);
                let _ = SendMessageW(popup, WM_LBUTTONDOWN, WPARAM(1), click_lp);
                assert!(state.borrow().pressed_inside);
                assert!(state.borrow().open);

                let _ = SendMessageW(popup, WM_LBUTTONUP, WPARAM(0), click_lp);
                assert_eq!(state.borrow().chosen_index, Some(0));
                assert!(!state.borrow().open);

                let _ = SendMessageW(popup, WM_CAPTURECHANGED, WPARAM(0), LPARAM(0));
                if GetCapture() == popup {
                    let _ = ReleaseCapture();
                }
                assert_eq!(state.borrow().chosen_index, Some(0));

                let _ = SetWindowLongPtrW(popup, GWLP_USERDATA, 0);
                let _ = DestroyWindow(popup);
                let _ = DestroyWindow(owner);
            }
            Ok(())
        }
    }
}
pub unsafe fn measure(item: &mut MEASUREITEMSTRUCT) -> bool {
    if item.CtlType != ODT_MENU || item.itemData == 0 {
        return false;
    }
    let idx = item.itemData - 1;
    let dims = ACTIVE_MENU.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|active| active.menu_open)
            .and_then(|active| active.entries.get(idx))
            .map(|entry| (entry.width, entry.height))
    });
    let Some((width, height)) = dims else {
        return false;
    };
    item.itemWidth = width;
    item.itemHeight = height;
    true
}
pub unsafe fn draw(item: &DRAWITEMSTRUCT) -> bool {
    if item.CtlType != ODT_MENU || item.itemData == 0 {
        return false;
    }
    let idx = item.itemData - 1;
    let entry = ACTIVE_MENU.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|active| active.menu_open)
            .and_then(|active| active.entries.get(idx))
            .cloned()
    });
    let Some(entry) = entry else {
        return false;
    };
    // SAFETY: HDC and rectangle are supplied by native menu dispatch; entry was validated via index bounds.
    unsafe {
        let selected = item.itemState.0 & ODS_SELECTED.0 != 0;
        let disabled = item.itemState.0 & (ODS_DISABLED.0 | ODS_GRAYED.0) != 0;
        let saved = SaveDC(item.hDC);
        let background = if selected {
            entry.colors.selected
        } else {
            entry.colors.background
        };
        let brush = CreateSolidBrush(background);
        FillRect(item.hDC, &item.rcItem, brush);
        let _ = DeleteObject(brush);
        SetBkMode(item.hDC, TRANSPARENT);
        SelectObject(item.hDC, entry.font);
        SetTextColor(
            item.hDC,
            if disabled {
                entry.colors.disabled
            } else {
                entry.colors.text
            },
        );
        let mut rect = item.rcItem;
        rect.left += entry.padding;
        rect.right -= entry.padding.min(16);
        if entry.checked {
            let cy = (item.rcItem.top + item.rcItem.bottom) / 2;
            let cx = item.rcItem.left + entry.padding / 2;
            let stroke = (entry.padding / 13).max(2);
            let u = (entry.padding / 6).max(3);
            let pen = CreatePen(PS_SOLID, stroke, COLORREF(0x1e54d9));
            let old_pen = SelectObject(item.hDC, pen);
            let _ = MoveToEx(item.hDC, cx - u, cy, None);
            let _ = LineTo(item.hDC, cx - u / 4, cy + u * 3 / 4);
            let _ = LineTo(item.hDC, cx + u * 5 / 4, cy - u * 3 / 4);
            SelectObject(item.hDC, old_pen);
            let _ = DeleteObject(pen);
        }
        let mut text = entry.text[..entry.text.len() - 1].to_vec();
        DrawTextW(
            item.hDC,
            &mut text,
            &mut rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );
        if item.itemState.0 & ODS_FOCUS.0 != 0 && item.itemState.0 & 0x0200 == 0 {
            let _ = DrawFocusRect(item.hDC, &item.rcItem);
        }
        let _ = RestoreDC(item.hDC, saved);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::{Foundation::RECT, UI::Controls::ODS_FLAGS};
    fn contrast(a: COLORREF, b: COLORREF) -> f64 {
        let luminance = |color: COLORREF| {
            let component = |shift| {
                let value = f64::from((color.0 >> shift) & 255u32) / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * component(0) + 0.7152 * component(8) + 0.0722 * component(16)
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    #[test]
    fn native_menu_names_geometry_and_palettes() -> Result<(), String> {
        let signed = WPARAM((-120i16 as u16 as usize) | ((48usize) << 16));
        assert_eq!(anchor(signed), POINT { x: -120, y: 48 });
        assert_eq!(anchor(WPARAM(0xffffffff)), POINT { x: -1, y: -1 });
        let labels = [
            (109, "Open controls"),
            (103, "Pause"),
            (120, "Turn sound on"),
            (104, "Stop wallpaper"),
            (126, "Check for updates"),
            (110, "Exit"),
        ];
        let app_colors = Palette::app_surface();
        assert!(contrast(app_colors.text, app_colors.background) >= 4.5);
        assert!(contrast(app_colors.text, app_colors.selected) >= 4.5);
        assert!(contrast(app_colors.disabled, app_colors.background) >= 4.5);
        for dpi in [96, 192] {
            for dark in [false, true] {
                let colors = Palette::new(dark);
                assert!(contrast(colors.text, colors.background) >= 4.5);
                assert!(contrast(colors.text, colors.selected) >= 4.5);
                assert!(contrast(colors.disabled, colors.background) >= 4.5);
                let menu = Menu::new(HWND::default(), &labels, Some(109), Some(colors), dpi)?;
                // SAFETY: memory DC/bitmap are owned by this test; no visible windows or registry writes.
                unsafe {
                    let width = menu.entries.iter().map(|e| e.width).max().unwrap() as i32;
                    let height = menu.entries.iter().map(|e| e.height).sum::<u32>() as i32;
                    let dc = CreateCompatibleDC(None);
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
                    .map_err(|e| e.to_string())?;
                    let previous = SelectObject(dc, bitmap);
                    let mut top = 0;
                    for (index, entry) in menu.entries.iter().enumerate() {
                        let data = index + 1;
                        let mut size = MEASUREITEMSTRUCT {
                            CtlType: ODT_MENU,
                            itemData: data,
                            ..Default::default()
                        };
                        assert!(measure(&mut size));
                        assert!(size.itemWidth > 0 && size.itemHeight >= 28 * dpi / 96);
                        assert_eq!(entry.accessible.dwMSAASignature, MSAA_MENU_SIG as u32);
                        assert_eq!(entry.accessible.pszWText.0, entry.text.as_ptr().cast_mut());
                        let mut info = MENUITEMINFOW {
                            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                            fMask: MIIM_STRING,
                            ..Default::default()
                        };
                        GetMenuItemInfoW(menu.handle, index as u32, true, &mut info)
                            .map_err(|e| e.to_string())?;
                        assert_eq!(info.cch, entry.accessible.cchWText);
                        let rect = RECT {
                            left: 0,
                            top,
                            right: width,
                            bottom: top + entry.height as i32,
                        };
                        assert!(draw(&DRAWITEMSTRUCT {
                            CtlType: ODT_MENU,
                            itemData: data,
                            hDC: dc,
                            rcItem: rect,
                            itemState: if index == 1 {
                                ODS_FLAGS(ODS_SELECTED.0 | ODS_FOCUS.0)
                            } else {
                                Default::default()
                            },
                            ..Default::default()
                        }));
                        assert_eq!(
                            GetPixel(dc, 3, top + 3),
                            if index == 1 {
                                colors.selected
                            } else {
                                colors.background
                            }
                        );
                        top = rect.bottom;
                    }
                    let _ = GdiFlush();
                    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("scratch/tray-theme-proof");
                    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
                    let pixels = std::slice::from_raw_parts(
                        bits as *const u8,
                        (width * height * 4) as usize,
                    );
                    let mut bmp = Vec::new();
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
                    let stamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos();
                    let path = directory.join(format!(
                        "{}-{dpi}-{stamp}.bmp",
                        if dark { "dark" } else { "light" }
                    ));
                    std::fs::write(&path, bmp).map_err(|e| e.to_string())?;
                    println!("Native menu drawing: {}", path.display());
                    SelectObject(dc, previous);
                    let _ = DeleteObject(bitmap);
                    let _ = DeleteDC(dc);
                }
                // Out-of-bounds itemData and closed menu must never dereference or succeed.
                unsafe {
                    let mut invalid = MEASUREITEMSTRUCT {
                        CtlType: ODT_MENU,
                        itemData: 0xDEAD_BEEF,
                        ..Default::default()
                    };
                    assert!(!measure(&mut invalid));
                    drop(menu);
                    let mut closed = MEASUREITEMSTRUCT {
                        CtlType: ODT_MENU,
                        itemData: 1,
                        ..Default::default()
                    };
                    assert!(!measure(&mut closed));
                }
            }
        }
        Ok(())
    }
}
