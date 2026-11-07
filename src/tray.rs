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
}
fn system_palette() -> Option<Palette> {
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
}
struct Menu {
    handle: HMENU,
    #[allow(clippy::vec_box)]
    // Native menu stores stable itemData pointers while this Vec can grow.
    entries: Vec<Box<Entry>>,
    brush: HBRUSH,
    font: HFONT,
}
impl Drop for Menu {
    fn drop(&mut self) {
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
                for (id, label) in items {
                    let mut text: Vec<_> = label.encode_utf16().chain(Some(0)).collect();
                    let dc = GetDC(owner);
                    if dc.0.is_null() {
                        return Err("Could not measure tray menu text".into());
                    }
                    let old = SelectObject(dc, menu.font);
                    let mut size = SIZE::default();
                    let measured =
                        GetTextExtentPoint32W(dc, &text[..text.len() - 1], &mut size).as_bool();
                    SelectObject(dc, old);
                    ReleaseDC(owner, dc);
                    if !measured {
                        return Err("Could not measure tray menu text".into());
                    }
                    let scale = |n| n * dpi as i32 / 96;
                    let entry = Box::new(Entry {
                        accessible: MSAAMENUINFO {
                            dwMSAASignature: MSAA_MENU_SIG as u32,
                            cchWText: (text.len() - 1) as u32,
                            pszWText: PWSTR(text.as_mut_ptr()),
                        },
                        text,
                        font: menu.font,
                        colors,
                        width: (size.cx + scale(32)) as u32,
                        height: (size.cy.max(scale(16)) + scale(12)) as u32,
                        padding: scale(16),
                    });
                    let info = MENUITEMINFOW {
                        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                        fMask: MIIM_ID | MIIM_FTYPE | MIIM_DATA | MIIM_STRING,
                        fType: MFT_OWNERDRAW,
                        wID: *id as u32,
                        dwItemData: (&*entry as *const Entry) as usize,
                        dwTypeData: entry.accessible.pszWText,
                        cch: entry.accessible.cchWText,
                        ..Default::default()
                    };
                    InsertMenuItemW(menu.handle, *id as u32, false, &info)
                        .map_err(|e| e.to_string())?;
                    menu.entries.push(entry);
                }
            } else {
                for (id, label) in items {
                    let text: Vec<_> = label.encode_utf16().chain(Some(0)).collect();
                    AppendMenuW(menu.handle, MF_STRING, *id, PCWSTR(text.as_ptr()))
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
    let menu = Menu::new(owner, items, system_palette(), dpi.max(96))?;
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
pub unsafe fn measure(item: &mut MEASUREITEMSTRUCT) -> bool {
    if item.CtlType != ODT_MENU || item.itemData == 0 {
        return false;
    }
    // SAFETY: caller dispatches native menu notifications; itemData belongs to the live Menu.
    let entry = unsafe { &*(item.itemData as *const Entry) };
    item.itemWidth = entry.width;
    item.itemHeight = entry.height;
    true
}
pub unsafe fn draw(item: &DRAWITEMSTRUCT) -> bool {
    if item.CtlType != ODT_MENU || item.itemData == 0 {
        return false;
    }
    // SAFETY: itemData, HDC and rectangle are supplied by native menu dispatch and outlive this call.
    unsafe {
        let entry = &*(item.itemData as *const Entry);
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
        rect.right -= entry.padding;
        let mut text = entry.text[..entry.text.len() - 1].to_vec();
        DrawTextW(
            item.hDC,
            &mut text,
            &mut rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );
        if item.itemState.0 & ODS_FOCUS.0 != 0 {
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
            (126, "Pembaruan tersedia (v0.1.2)"),
            (110, "Exit"),
        ];
        for dpi in [96, 192] {
            for dark in [false, true] {
                let colors = Palette::new(dark);
                assert!(contrast(colors.text, colors.background) >= 4.5);
                assert!(contrast(colors.text, colors.selected) >= 4.5);
                assert!(contrast(colors.disabled, colors.background) >= 4.5);
                let menu = Menu::new(HWND::default(), &labels, Some(colors), dpi)?;
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
                        let data = (&**entry as *const Entry) as usize;
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
            }
        }
        Ok(())
    }
}
