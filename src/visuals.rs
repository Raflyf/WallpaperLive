use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::{COLORREF, HWND, LRESULT, RECT},
        Graphics::Gdi::*,
        UI::{
            Controls::{DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED},
            HiDpi::GetDpiForWindow,
            WindowsAndMessaging::*,
        },
    },
    core::w,
};

pub const TILE_BASE: usize = 400;
pub const PREVIEW: usize = 115;
pub const PREVIOUS: usize = 116;
pub const NEXT: usize = 117;
pub const CANVAS: u32 = 0x181b20;
pub const PANEL: u32 = 0x22272e;
pub const CARD: u32 = 0x262c34;
pub const TEXT: u32 = 0xf0f1f3;
pub const MUTED: u32 = 0xb5bcc6;
pub const ACCENT: u32 = 0xa6c7df;

pub fn rgb(hex: u32) -> COLORREF {
    COLORREF(((hex >> 16) & 255) | (hex & 0xff00) | ((hex & 255) << 16))
}
fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
    RECT {
        left: l,
        top: t,
        right: r,
        bottom: b,
    }
}
struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.0);
        }
    }
}
struct Skin {
    selected: Option<PathBuf>,
    tiles: HashMap<usize, PathBuf>,
    thumbnails: HashMap<PathBuf, Bitmap>,
    settings: bool,
}
thread_local! {static SKIN:RefCell<Skin>=RefCell::new(Skin{selected:None,tiles:HashMap::new(),thumbnails:HashMap::new(),settings:false});}
pub fn selected(path: Option<PathBuf>) {
    SKIN.with(|s| s.borrow_mut().selected = path);
}
pub fn tiles(paths: &[PathBuf]) {
    SKIN.with(|s| {
        s.borrow_mut().tiles = paths
            .iter()
            .enumerate()
            .map(|(i, p)| (TILE_BASE + i, p.clone()))
            .collect()
    });
}
pub fn thumbnail(path: PathBuf, image: HBITMAP) {
    SKIN.with(|s| {
        let mut skin = s.borrow_mut();
        if skin.thumbnails.len() >= 16 && !skin.thumbnails.contains_key(&path) {
            skin.thumbnails.clear();
        }
        skin.thumbnails.insert(path, Bitmap(image));
    });
}
pub fn settings(value: bool) {
    SKIN.with(|s| s.borrow_mut().settings = value);
}
pub fn clear() {
    SKIN.with(|s| {
        let mut skin = s.borrow_mut();
        skin.thumbnails.clear();
        skin.tiles.clear();
        skin.selected = None;
    });
    BRUSHES.with(|b| {
        for (_, brush) in b.borrow_mut().drain() {
            unsafe {
                let _ = DeleteObject(brush);
            }
        }
    });
}

pub unsafe fn fill(dc: HDC, r: RECT, color: u32) {
    // SAFETY: brush belongs to this synchronous drawing call.
    unsafe {
        let brush = CreateSolidBrush(rgb(color));
        FillRect(dc, &r, brush);
        let _ = DeleteObject(brush);
    }
}
unsafe fn rounded(dc: HDC, r: RECT, color: u32, border: u32, radius: i32) {
    // SAFETY: restore native objects before deleting the created brush and pen.
    unsafe {
        let brush = CreateSolidBrush(rgb(color));
        let pen = CreatePen(PS_SOLID, 1, rgb(border));
        let b = SelectObject(dc, brush);
        let p = SelectObject(dc, pen);
        let _ = RoundRect(dc, r.left, r.top, r.right, r.bottom, radius, radius);
        SelectObject(dc, b);
        SelectObject(dc, p);
        let _ = DeleteObject(brush);
        let _ = DeleteObject(pen);
    }
}
unsafe fn label(
    dc: HDC,
    text: &str,
    r: RECT,
    size: i32,
    bold: bool,
    color: u32,
    flags: DRAW_TEXT_FORMAT,
) {
    // SAFETY: drawing only borrows temporary UTF-16/font until native function returns.
    unsafe {
        let font = CreateFontW(
            -size,
            0,
            0,
            0,
            if bold { 600 } else { 400 },
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
        let old = SelectObject(dc, font);
        SetTextColor(dc, rgb(color));
        SetBkMode(dc, TRANSPARENT);
        let mut text: Vec<u16> = text.encode_utf16().collect();
        let mut r = r;
        DrawTextW(dc, &mut text, &mut r, flags | DT_NOPREFIX);
        SelectObject(dc, old);
        let _ = DeleteObject(font);
    }
}
unsafe fn image(dc: HDC, bitmap: HBITMAP, r: RECT) {
    // SAFETY: bitmap stays owned by Skin, temporary DC restores its original bitmap before deletion.
    unsafe {
        let mut b = BITMAP::default();
        if GetObjectW(
            bitmap,
            std::mem::size_of::<BITMAP>() as i32,
            Some((&mut b as *mut BITMAP).cast()),
        ) == 0
            || b.bmWidth <= 0
            || b.bmHeight <= 0
        {
            return;
        }
        let width = r.right - r.left;
        let height = r.bottom - r.top;
        if width <= 0 || height <= 0 {
            return;
        }
        let source = CreateCompatibleDC(dc);
        let old = SelectObject(source, bitmap);
        let (sw, sh) = if i64::from(b.bmWidth) * i64::from(height)
            > i64::from(b.bmHeight) * i64::from(width)
        {
            (b.bmHeight * width / height, b.bmHeight)
        } else {
            (b.bmWidth, b.bmWidth * height / width)
        };
        SetStretchBltMode(dc, HALFTONE);
        let _ = StretchBlt(
            dc,
            r.left,
            r.top,
            width,
            height,
            source,
            (b.bmWidth - sw) / 2,
            (b.bmHeight - sh) / 2,
            sw,
            sh,
            SRCCOPY,
        );
        SelectObject(source, old);
        let _ = DeleteDC(source);
    }
}
fn filename(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Wallpaper")
        .to_owned()
}
fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("MEDIA")
        .to_ascii_uppercase()
}
fn window_text(hwnd: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(hwnd).clamp(0, 32768) as usize;
        let mut text = vec![0u16; n + 1];
        let n = GetWindowTextW(hwnd, &mut text).max(0) as usize;
        String::from_utf16_lossy(&text[..n])
    }
}

pub unsafe fn draw_item(item: &DRAWITEMSTRUCT) {
    let id = item.CtlID as usize;
    let dpi = unsafe { GetDpiForWindow(item.hwndItem) }.max(96) as i32;
    let s = |n: i32| n * dpi / 96;
    let r = item.rcItem;
    let dc = item.hDC;
    // SAFETY: native DRAWITEMSTRUCT and HDC stay valid for this UI-thread dispatch only.
    unsafe {
        if id == PREVIEW || (TILE_BASE..TILE_BASE + 6).contains(&id) {
            SKIN.with(|skin| {
                let skin = skin.borrow();
                let path = if id == PREVIEW {
                    skin.selected.as_ref()
                } else {
                    skin.tiles.get(&id)
                };
                let active = id != PREVIEW && path.is_some() && path == skin.selected.as_ref();
                rounded(
                    dc,
                    r,
                    if active { 0x2c3843 } else { CARD },
                    if active { ACCENT } else { CARD },
                    s(8),
                );
                let area = rect(
                    r.left + s(1),
                    r.top + s(1),
                    r.right - s(1),
                    if id == PREVIEW {
                        r.bottom - s(1)
                    } else {
                        r.bottom - s(58)
                    },
                );
                if let Some(path) = path {
                    if let Some(bitmap) = skin.thumbnails.get(path) {
                        image(dc, bitmap.0, area);
                    } else {
                        label(
                            dc,
                            &extension(path),
                            area,
                            s(24),
                            true,
                            MUTED,
                            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                        );
                    }
                    if id != PREVIEW {
                        label(
                            dc,
                            &filename(path),
                            rect(
                                r.left + s(14),
                                r.bottom - s(45),
                                r.right - s(14),
                                r.bottom - s(21),
                            ),
                            s(15),
                            true,
                            TEXT,
                            DT_SINGLELINE | DT_END_ELLIPSIS,
                        );
                        label(
                            dc,
                            &format!(
                                "{}  {}",
                                extension(path),
                                if active { "Selected" } else { "Local file" }
                            ),
                            rect(
                                r.left + s(14),
                                r.bottom - s(24),
                                r.right - s(14),
                                r.bottom - s(5),
                            ),
                            s(12),
                            false,
                            if active { ACCENT } else { MUTED },
                            DT_SINGLELINE | DT_END_ELLIPSIS,
                        );
                    }
                } else {
                    label(
                        dc,
                        "Select a wallpaper",
                        area,
                        s(15),
                        false,
                        MUTED,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                    );
                }
                if active {
                    let pen = CreatePen(PS_SOLID, s(2).max(1), rgb(ACCENT));
                    let old_pen = SelectObject(dc, pen);
                    let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
                    let _ = RoundRect(dc, r.left, r.top, r.right, r.bottom, s(8), s(8));
                    SelectObject(dc, old_brush);
                    SelectObject(dc, old_pen);
                    let _ = DeleteObject(pen);
                }
            });
        } else {
            let text = window_text(item.hwndItem).replace('&', "");
            let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
            let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
            let primary = id == 102;
            let color = if disabled {
                0x2a3038
            } else if primary {
                if pressed { 0x8ab1cd } else { ACCENT }
            } else if pressed {
                0x39434e
            } else {
                0x303740
            };
            rounded(dc, r, color, if primary { color } else { 0x39424e }, s(6));
            label(
                dc,
                &text,
                rect(r.left + s(8), r.top, r.right - s(8), r.bottom),
                s(14),
                true,
                if disabled {
                    0x9aa4b1
                } else if primary {
                    0x18242e
                } else {
                    TEXT
                },
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
            );
        }
        if item.itemState.0 & ODS_FOCUS.0 != 0 {
            let focus = rect(r.left + s(4), r.top + s(4), r.right - s(4), r.bottom - s(4));
            SetTextColor(dc, rgb(TEXT));
            let _ = DrawFocusRect(dc, &focus);
        }
    }
}

pub unsafe fn paint(window: HWND) {
    // SAFETY: BeginPaint/EndPaint pair paints only invalidated UI, never runs continuously.
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(window, &mut ps);
        let mut r = RECT::default();
        let _ = GetClientRect(window, &mut r);
        fill(dc, r, CANVAS);
        let dpi = GetDpiForWindow(window).max(96) as i32;
        let s = |n: i32| n * dpi / 96;
        let width = r.right * 96 / dpi;
        let height = r.bottom * 96 / dpi;
        let split = width - 344;
        fill(dc, rect(s(split), s(76), r.right, s(height - 82)), PANEL);
        label(
            dc,
            "WallpaperLive",
            rect(s(28), s(23), s(300), s(57)),
            s(21),
            true,
            TEXT,
            DT_SINGLELINE,
        );
        label(
            dc,
            "Library",
            rect(s(28), s(99), s(split - 22), s(132)),
            s(25),
            true,
            TEXT,
            DT_SINGLELINE,
        );
        label(
            dc,
            "Local media",
            rect(s(28), s(140), s(split - 22), s(162)),
            s(13),
            false,
            MUTED,
            DT_SINGLELINE,
        );
        let (show_settings, empty) = SKIN.with(|skin| {
            let skin = skin.borrow();
            (skin.settings, skin.tiles.is_empty())
        });
        if empty && !show_settings {
            let area = rect(s(28), s(195), s(split - 28), s(height - 110));
            label(
                dc,
                "No wallpapers yet",
                rect(area.left, area.top + s(45), area.right, area.top + s(82)),
                s(23),
                true,
                TEXT,
                DT_SINGLELINE,
            );
            label(
                dc,
                "Add a video, GIF or image. Your files stay where they are.",
                rect(
                    area.left,
                    area.top + s(96),
                    area.right - s(24),
                    area.top + s(158),
                ),
                s(15),
                false,
                MUTED,
                DT_WORDBREAK,
            );
            label(
                dc,
                "MP4  GIF  MKV  WebM  MOV  AVI",
                rect(area.left, area.top + s(172), area.right, area.top + s(200)),
                s(13),
                false,
                MUTED,
                DT_SINGLELINE,
            );
        }
        label(
            dc,
            "Selected wallpaper",
            rect(s(split + 24), s(99), s(width - 24), s(125)),
            s(16),
            true,
            TEXT,
            DT_SINGLELINE,
        );
        label(
            dc,
            "Display",
            rect(s(split + 24), s(378), s(width - 24), s(400)),
            s(13),
            true,
            MUTED,
            DT_SINGLELINE,
        );
        label(
            dc,
            "Placement",
            rect(s(split + 24), s(456), s(width - 24), s(478)),
            s(13),
            true,
            MUTED,
            DT_SINGLELINE,
        );
        if show_settings {
            label(
                dc,
                "Playback",
                rect(s(28), s(186), s(split - 28), s(220)),
                s(21),
                true,
                TEXT,
                DT_SINGLELINE,
            );
            label(
                dc,
                "Wallpaper pauses when the display turns off, the session locks or a fullscreen app covers this display.",
                rect(s(28), s(235), s(split - 28), s(295)),
                s(15),
                false,
                MUTED,
                DT_WORDBREAK,
            );
        }
        fill(dc, rect(0, s(height - 82), r.right, r.bottom), 0x20252c);
        let _ = EndPaint(window, &ps);
    }
}

pub unsafe fn control_colors(message: u32, dc: HDC, window: HWND) -> Option<LRESULT> {
    if !matches!(
        message,
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN
    ) {
        return None;
    }
    // SAFETY: native control requests a persistent UI-owned brush, deleted only after windows close.
    unsafe {
        let id = GetDlgCtrlID(window) as usize;
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, rgb(if id == 113 { MUTED } else { TEXT }));
        let color = if id == 113 {
            0x20252c
        } else if matches!(id, 105 | 106) {
            0x303943
        } else {
            PANEL
        };
        SetBkColor(dc, rgb(color));
        Some(LRESULT(brush(color).0 as isize))
    }
}
thread_local! {static BRUSHES:RefCell<HashMap<u32,HBRUSH>>=RefCell::new(HashMap::new());}
pub fn brush(color: u32) -> HBRUSH {
    BRUSHES.with(|b| {
        *b.borrow_mut()
            .entry(color)
            .or_insert_with(|| unsafe { CreateSolidBrush(rgb(color)) })
    })
}
