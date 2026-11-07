use crate::core::Rect;
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
pub const AUDIO: usize = 118;
pub const REMOVE: usize = 119;
pub const PAGE: usize = 120;
pub const MIN_CLIENT_WIDTH: i32 = 920;
pub const MIN_CLIENT_HEIGHT: i32 = 640;

pub struct Layout {
    pub client: Rect,
    pub header: Rect,
    pub footer: Rect,
    pub library: Rect,
    pub pager: Rect,
    pub sidebar: Rect,
    pub title: Rect,
    pub library_title: Rect,
    pub library_note: Rect,
    pub preview_title: Rect,
    pub display_label: Rect,
    pub fit_label: Rect,
    pub settings_title: Rect,
    pub settings_note: Rect,
    pub tile_rects: [Rect; 4],
    controls: [(usize, Rect); 19],
}
impl Layout {
    pub fn new(width: i32, height: i32) -> Self {
        let r = |x, y, w, h| Rect {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        };
        let split = width - 344;
        let footer = r(0, height - 76, width, 76);
        let pager = r(24, footer.top - 48, split - 48, 36);
        let library = r(24, 168, split - 48, pager.top - 184);
        let tile_width = (library.right - library.left - 16) / 2;
        let tile_height = (library.bottom - library.top - 16) / 2;
        let tile_rects = std::array::from_fn(|i| {
            r(
                library.left + (i % 2) as i32 * (tile_width + 16),
                library.top + (i / 2) as i32 * (tile_height + 16),
                tile_width,
                tile_height,
            )
        });
        let side_x = split + 24;
        let side_width = 296;
        let preview_height = (height - 528).clamp(1, side_width * 9 / 16);
        let preview_width = preview_height * 16 / 9;
        let preview = r(
            side_x + (side_width - preview_width) / 2,
            120,
            preview_width,
            preview_height,
        );
        let file = r(side_x, preview.bottom + 12, side_width, 24);
        let display_label = r(side_x, file.bottom + 12, side_width, 20);
        let display = r(side_x, display_label.bottom + 4, side_width, 34);
        let fit_label = r(side_x, display.bottom + 12, side_width, 20);
        let fit = r(side_x, fit_label.bottom + 4, side_width, 34);
        let pause_y = footer.top - 50;
        let apply_y = pause_y - 48;
        let audio_y = apply_y - 44;
        Self {
            client: r(0, 0, width, height),
            header: r(0, 0, width, 72),
            footer,
            library,
            pager,
            sidebar: r(split, 72, width - split, footer.top - 72),
            title: r(24, 18, 260, 36),
            library_title: r(24, 92, split - 48, 32),
            library_note: r(24, 132, split - 48, 24),
            preview_title: r(side_x, 88, side_width, 24),
            display_label,
            fit_label,
            settings_title: r(24, 184, split - 48, 32),
            settings_note: r(24, 230, split - 48, 64),
            tile_rects,
            controls: [
                (101, r(width - 338, 18, 194, 36)),
                (114, r(width - 132, 18, 108, 36)),
                (PREVIEW, preview),
                (112, file),
                (105, display),
                (106, fit),
                (AUDIO, r(side_x, audio_y, 132, 32)),
                (REMOVE, r(side_x + 144, audio_y, 152, 32)),
                (102, r(side_x, apply_y, side_width, 38)),
                (103, r(side_x, pause_y, 142, 34)),
                (104, r(side_x + 154, pause_y, 142, 34)),
                (107, r(24, 316, split - 48, 32)),
                (108, r(24, 366, split - 48, 32)),
                (121, r(24, 416, split - 48, 32)),
                (PREVIOUS, r(pager.left, pager.top, 108, 36)),
                (NEXT, r(pager.left + 120, pager.top, 108, 36)),
                (
                    PAGE,
                    r(
                        pager.left + 244,
                        pager.top,
                        pager.right - pager.left - 244,
                        36,
                    ),
                ),
                (110, r(width - 104, footer.top + 20, 80, 36)),
                (113, r(24, footer.top + 12, width - 152, 52)),
            ],
        }
    }
    pub fn control(&self, id: usize) -> Option<Rect> {
        if (TILE_BASE..TILE_BASE + 4).contains(&id) {
            return Some(self.tile_rects[id - TILE_BASE]);
        }
        self.controls
            .iter()
            .find(|(key, _)| *key == id)
            .map(|(_, area)| *area)
    }
}

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
    playing: Option<PathBuf>,
    tiles: HashMap<usize, PathBuf>,
    thumbnails: HashMap<PathBuf, Bitmap>,
    settings: bool,
}
thread_local! {static SKIN:RefCell<Skin>=RefCell::new(Skin{selected:None,playing:None,tiles:HashMap::new(),thumbnails:HashMap::new(),settings:false});}
pub fn playing(path: Option<PathBuf>) {
    SKIN.with(|s| s.borrow_mut().playing = path);
}
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
pub fn has_thumbnail(path: &Path) -> bool {
    SKIN.with(|s| s.borrow().thumbnails.contains_key(path))
}
pub fn retain_thumbnails(paths: &[PathBuf]) {
    SKIN.with(|s| {
        s.borrow_mut()
            .thumbnails
            .retain(|path, _| paths.contains(path))
    });
}
pub fn remove_thumbnail(path: &Path) {
    SKIN.with(|s| {
        s.borrow_mut().thumbnails.remove(path);
    });
}
pub fn thumbnail(path: PathBuf, image: HBITMAP) {
    let image = Bitmap(image);
    if image.0.0.is_null() {
        return;
    }
    SKIN.with(|s| {
        let mut skin = s.borrow_mut();
        if skin.thumbnails.len() >= 12 && !skin.thumbnails.contains_key(&path) {
            let victim = skin
                .thumbnails
                .keys()
                .find(|p| {
                    Some(*p) != skin.selected.as_ref()
                        && !skin.tiles.values().any(|tile| tile == *p)
                })
                .or_else(|| skin.thumbnails.keys().next())
                .cloned();
            if let Some(victim) = victim {
                skin.thumbnails.remove(&victim);
            }
        }
        skin.thumbnails.insert(path, image);
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
        skin.playing = None;
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
        if id == PAGE {
            fill(dc, r, CANVAS);
            label(
                dc,
                &window_text(item.hwndItem),
                r,
                s(13),
                false,
                MUTED,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
        } else if id == PREVIEW || (TILE_BASE..TILE_BASE + 4).contains(&id) {
            SKIN.with(|skin| {
                let skin = skin.borrow();
                let path = if id == PREVIEW {
                    skin.selected.as_ref()
                } else {
                    skin.tiles.get(&id)
                };
                let active = id != PREVIEW && path.is_some() && path == skin.selected.as_ref();
                let playing = path.is_some() && path == skin.playing.as_ref();
                rounded(
                    dc,
                    r,
                    if active { 0x2c3843 } else { CARD },
                    if active { ACCENT } else { CARD },
                    s(8),
                );
                let mut area = rect(
                    r.left + s(1),
                    r.top + s(1),
                    r.right - s(1),
                    if id == PREVIEW {
                        r.bottom - s(1)
                    } else {
                        r.bottom - s(58)
                    },
                );
                let width = (area.right - area.left).max(1);
                let height = (area.bottom - area.top).max(1);
                if i64::from(width) * 9 > i64::from(height) * 16 {
                    let fitted = height * 16 / 9;
                    area.left += (width - fitted) / 2;
                    area.right = area.left + fitted;
                } else {
                    let fitted = width * 9 / 16;
                    area.top += (height - fitted) / 2;
                    area.bottom = area.top + fitted;
                }
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
                                match (active, playing) {
                                    (true, true) => "Selected · Active",
                                    (true, false) => "Selected",
                                    (false, true) => "Active",
                                    (false, false) => "Local file",
                                }
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
            let selector = matches!(id, 105 | 106);
            label(
                dc,
                &text,
                rect(
                    r.left + s(if selector { 12 } else { 8 }),
                    r.top,
                    r.right - s(if selector { 32 } else { 8 }),
                    r.bottom,
                ),
                s(14),
                true,
                if disabled {
                    0x9aa4b1
                } else if primary {
                    0x18242e
                } else {
                    TEXT
                },
                (if selector { DT_LEFT } else { DT_CENTER })
                    | DT_VCENTER
                    | DT_SINGLELINE
                    | DT_END_ELLIPSIS,
            );
            if selector {
                let pen = CreatePen(PS_SOLID, s(2).max(1), rgb(MUTED));
                let old = SelectObject(dc, pen);
                let x = r.right - s(18);
                let y = (r.top + r.bottom) / 2;
                let _ = MoveToEx(dc, x - s(4), y - s(2), None);
                let _ = LineTo(dc, x, y + s(2));
                let _ = LineTo(dc, x + s(4), y - s(2));
                SelectObject(dc, old);
                let _ = DeleteObject(pen);
            }
        }
        if item.itemState.0 & ODS_FOCUS.0 != 0 {
            let focus = rect(r.left + s(4), r.top + s(4), r.right - s(4), r.bottom - s(4));
            SetTextColor(dc, rgb(TEXT));
            let _ = DrawFocusRect(dc, &focus);
        }
    }
}

pub unsafe fn paint(window: HWND) {
    // SAFETY: UI-thread BeginPaint/EndPaint pair; drawing borrows shared logical layout only.
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(window, &mut ps);
        let mut client = RECT::default();
        let _ = GetClientRect(window, &mut client);
        let dpi = GetDpiForWindow(window).max(96) as i32;
        let scale = |n: i32| n * dpi / 96;
        let pixels = |r: Rect| rect(scale(r.left), scale(r.top), scale(r.right), scale(r.bottom));
        let layout = Layout::new(client.right * 96 / dpi, client.bottom * 96 / dpi);
        fill(dc, pixels(layout.client), CANVAS);
        fill(dc, pixels(layout.header), CANVAS);
        fill(dc, pixels(layout.sidebar), PANEL);
        fill(dc, pixels(layout.pager), CANVAS);
        fill(dc, pixels(layout.footer), 0x20252c);
        label(
            dc,
            "WallpaperLive",
            pixels(layout.title),
            scale(21),
            true,
            TEXT,
            DT_SINGLELINE,
        );
        let (show_settings, empty) = SKIN.with(|skin| {
            let skin = skin.borrow();
            (skin.settings, skin.tiles.is_empty())
        });
        label(
            dc,
            if show_settings { "Settings" } else { "Library" },
            pixels(layout.library_title),
            scale(25),
            true,
            TEXT,
            DT_SINGLELINE,
        );
        label(
            dc,
            if show_settings {
                "Playback preferences"
            } else {
                "Choose a thumbnail, then Set wallpaper."
            },
            pixels(layout.library_note),
            scale(13),
            false,
            MUTED,
            DT_SINGLELINE | DT_END_ELLIPSIS,
        );
        if empty && !show_settings {
            let area = layout.library;
            label(
                dc,
                "No wallpapers yet",
                pixels(Rect {
                    bottom: area.top + 40,
                    ..area
                }),
                scale(23),
                true,
                TEXT,
                DT_SINGLELINE | DT_END_ELLIPSIS,
            );
            label(
                dc,
                "Add a video, GIF or image. Your files stay where they are.",
                pixels(Rect {
                    top: area.top + 56,
                    bottom: area.top + 120,
                    ..area
                }),
                scale(15),
                false,
                MUTED,
                DT_WORDBREAK,
            );
            label(
                dc,
                "MP4  GIF  MKV  WebM  MOV  AVI",
                pixels(Rect {
                    top: area.top + 132,
                    bottom: area.top + 156,
                    ..area
                }),
                scale(13),
                false,
                MUTED,
                DT_SINGLELINE | DT_END_ELLIPSIS,
            );
        }
        label(
            dc,
            "Selected wallpaper",
            pixels(layout.preview_title),
            scale(16),
            true,
            TEXT,
            DT_SINGLELINE,
        );
        label(
            dc,
            "Display",
            pixels(layout.display_label),
            scale(13),
            true,
            MUTED,
            DT_SINGLELINE,
        );
        label(
            dc,
            "Placement",
            pixels(layout.fit_label),
            scale(13),
            true,
            MUTED,
            DT_SINGLELINE,
        );
        if show_settings {
            label(
                dc,
                "Playback",
                pixels(layout.settings_title),
                scale(21),
                true,
                TEXT,
                DT_SINGLELINE,
            );
            label(
                dc,
                "Pause preferences apply to the active wallpaper. Locking the session or turning off the display pauses playback.",
                pixels(layout.settings_note),
                scale(15),
                false,
                MUTED,
                DT_WORDBREAK,
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    fn overlap(a: Rect, b: Rect) -> bool {
        a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
    }
    #[test]
    fn layout_reserves_pager_and_footer_at_all_supported_sizes() {
        for (width, height) in [
            (MIN_CLIENT_WIDTH, MIN_CLIENT_HEIGHT),
            (1080, 740),
            (1280, 720),
            (1920, 1080),
        ] {
            let layout = Layout::new(width, height);
            for dpi in [96, 120, 144, 192] {
                let scale = |r: Rect| Rect {
                    left: r.left * dpi / 96,
                    top: r.top * dpi / 96,
                    right: r.right * dpi / 96,
                    bottom: r.bottom * dpi / 96,
                };
                let client = scale(layout.client);
                for (_, area) in &layout.controls {
                    assert!(
                        client.covers(scale(*area)),
                        "control outside {width}x{height} at {dpi} DPI: {area:?}"
                    );
                }
                for (i, tile) in layout.tile_rects.iter().enumerate() {
                    assert!(scale(layout.library).covers(scale(*tile)));
                    assert!(!overlap(scale(*tile), scale(layout.pager)));
                    assert!(!overlap(scale(*tile), scale(layout.footer)));
                    for other in &layout.tile_rects[i + 1..] {
                        assert!(!overlap(scale(*tile), scale(*other)));
                    }
                }
                for id in [PREVIEW, 112, 105, 106, AUDIO, REMOVE, 102, 103, 104] {
                    assert!(scale(layout.sidebar).covers(scale(layout.control(id).unwrap())));
                }
                for id in [PREVIOUS, NEXT, PAGE] {
                    assert!(scale(layout.pager).covers(scale(layout.control(id).unwrap())));
                }
            }
            for (i, (id, area)) in layout.controls.iter().enumerate() {
                for (_, other) in &layout.controls[i + 1..] {
                    assert!(
                        !overlap(*area, *other),
                        "control {id} overlaps {other:?} at {width}x{height}"
                    );
                }
            }
            assert!(layout.control(106).unwrap().bottom <= layout.control(AUDIO).unwrap().top);
            assert_eq!(
                layout.control(105).unwrap().bottom - layout.control(105).unwrap().top,
                34
            );
            assert_eq!(layout.control(TILE_BASE), Some(layout.tile_rects[0]));
            assert!(layout.control(TILE_BASE + 4).is_none());
        }
    }
}
