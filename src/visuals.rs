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
            Controls::{DRAWITEMSTRUCT, ODS_DISABLED, ODS_SELECTED},
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
pub const ADD_FOLDER: usize = 122;
pub const FOLDER_FILTER: usize = 123;
pub const FPS: usize = 124;
pub const SELECT_ALL: usize = 125;
pub const VERSION: usize = 127;
pub const CACHE_INFO: usize = 128;
pub const CLEAR_CACHE: usize = 129;
pub const MEMORY_INFO: usize = 130;
pub const TAB_LIBRARY: usize = 114;
pub const TAB_SETTINGS: usize = 132;
pub const MIN_CLIENT_WIDTH: i32 = 920;
pub const MIN_CLIENT_HEIGHT: i32 = 640;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Library,
    Settings,
}

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
    pub settings_note: Rect,
    pub tile_rects: [Rect; 9],
    controls: [(usize, Rect); 28],
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
        let library = r(24, 210, split - 48, pager.top - 226);
        let tile_col_w = (library.right - library.left - 24) / 3;
        let tile_row_h = (library.bottom - library.top - 24) / 3;
        let tile_rects = std::array::from_fn(|i| {
            let c = (i % 3) as i32;
            let row = (i / 3) as i32;
            r(
                library.left + c * (tile_col_w + 12),
                library.top + row * (tile_row_h + 12),
                tile_col_w,
                tile_row_h,
            )
        });
        let side_x = split + 24;
        let side_width = 296;
        let preview_height = (height - 572).clamp(1, side_width * 9 / 16);
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
            title: r(24, 18, 140, 36),
            library_title: r(24, 92, split - 48, 32),
            library_note: r(24, 132, split - 48, 24),
            preview_title: r(side_x, 88, side_width, 24),
            display_label,
            fit_label,
            settings_note: r(24, 176, (split - 48).min(480), 36),
            tile_rects,
            controls: [
                (101, r(width - 272, 18, 118, 36)),
                (ADD_FOLDER, r(width - 146, 18, 122, 36)),
                (TAB_LIBRARY, r(176, 18, 96, 36)),
                (TAB_SETTINGS, r(280, 18, 96, 36)),
                (FOLDER_FILTER, r(24, 164, split - 164, 34)),
                (PREVIEW, preview),
                (112, file),
                (105, display),
                (106, fit),
                (AUDIO, r(side_x, audio_y, 132, 32)),
                (REMOVE, r(side_x + 144, audio_y, 152, 32)),
                (102, r(side_x, apply_y, side_width, 38)),
                (103, r(side_x, pause_y, 142, 34)),
                (104, r(side_x + 154, pause_y, 142, 34)),
                (107, r(24, 224, (split - 48).min(400), 32)),
                (108, r(24, 304, (split - 48).min(400), 32)),
                (121, r(24, 264, (split - 48).min(400), 32)),
                (VERSION, r(24, 348, (split - 48).min(400), 22)),
                (MEMORY_INFO, r(24, 374, (split - 48).min(400), 22)),
                (CACHE_INFO, r(24, 400, (split - 48).min(400), 22)),
                (CLEAR_CACHE, r(24, 430, 156, 32)),
                (FPS, r(side_x, fit.bottom + 12, side_width, 34)),
                (SELECT_ALL, r(split - 128, 164, 104, 34)),
                (PREVIOUS, r(pager.left, pager.top, 104, 36)),
                (NEXT, r(pager.left + 116, pager.top, 104, 36)),
                (
                    PAGE,
                    r(
                        pager.left + 232,
                        pager.top,
                        pager.right - pager.left - 232,
                        36,
                    ),
                ),
                (110, r(width - 104, footer.top + 20, 80, 36)),
                (113, r(24, footer.top + 12, width - 152, 52)),
            ],
        }
    }
    pub fn control(&self, id: usize) -> Option<Rect> {
        if (TILE_BASE..TILE_BASE + 9).contains(&id) {
            return Some(self.tile_rects[id - TILE_BASE]);
        }
        self.controls
            .iter()
            .find(|(key, _)| *key == id)
            .map(|(_, area)| *area)
    }
}

pub const CANVAS: u32 = 0x111111;
pub const PANEL: u32 = 0x181818;
pub const CARD: u32 = 0x202020;
pub const TEXT: u32 = 0xf3eee3;
pub const MUTED: u32 = 0xa39e93;
pub const ACCENT: u32 = 0xd9541e;

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
    marked: Vec<PathBuf>,
    playing: Option<PathBuf>,
    tiles: HashMap<usize, PathBuf>,
    thumbnails: HashMap<PathBuf, Bitmap>,
    tab: Tab,
}
thread_local! {static SKIN:RefCell<Skin>=RefCell::new(Skin{selected:None,marked:Vec::new(),playing:None,tiles:HashMap::new(),thumbnails:HashMap::new(),tab:Tab::Library});}
pub fn marked(paths: &[PathBuf]) {
    SKIN.with(|skin| skin.borrow_mut().marked = paths.to_vec());
}
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
pub fn set_tab(tab: Tab) {
    SKIN.with(|s| s.borrow_mut().tab = tab);
}
pub fn clear() {
    SKIN.with(|s| {
        let mut skin = s.borrow_mut();
        skin.thumbnails.clear();
        skin.tiles.clear();
        skin.selected = None;
        skin.marked.clear();
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
        } else if id == PREVIEW || (TILE_BASE..TILE_BASE + 9).contains(&id) {
            SKIN.with(|skin| {
                let skin = skin.borrow();
                let path = if id == PREVIEW {
                    skin.selected.as_ref()
                } else {
                    skin.tiles.get(&id)
                };
                let active = id != PREVIEW && path.is_some() && path == skin.selected.as_ref();
                let playing = path.is_some() && path == skin.playing.as_ref();
                rounded(dc, r, CARD, if active { ACCENT } else { 0x2c2c2c }, s(6));
                let mut area = rect(
                    r.left + s(2),
                    r.top + s(2),
                    r.right - s(2),
                    if id == PREVIEW {
                        r.bottom - s(2)
                    } else {
                        r.bottom - s(26)
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
                        let title_rect = rect(
                            r.left + s(6),
                            r.bottom - s(24),
                            r.right - s(6),
                            r.bottom - s(4),
                        );
                        label(
                            dc,
                            &filename(path),
                            title_rect,
                            s(11),
                            true,
                            if active { ACCENT } else { TEXT },
                            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
                        );
                        if playing {
                            let badge_w = s(42);
                            let badge_h = s(16);
                            let badge_rect = rect(
                                area.right - badge_w - s(4),
                                area.bottom - badge_h - s(4),
                                area.right - s(4),
                                area.bottom - s(4),
                            );
                            rounded(dc, badge_rect, 0x3d1f14, ACCENT, s(3));
                            label(
                                dc,
                                "Active",
                                badge_rect,
                                s(9),
                                true,
                                ACCENT,
                                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                            );
                        }
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
                if id != PREVIEW
                    && let Some(path) = path
                {
                    let checked = skin.marked.contains(path);
                    let box_rect = rect(r.left + s(6), r.top + s(6), r.left + s(22), r.top + s(22));
                    rounded(
                        dc,
                        box_rect,
                        if checked { ACCENT } else { CANVAS },
                        if checked { ACCENT } else { MUTED },
                        s(3),
                    );
                    if checked {
                        let pen = CreatePen(PS_SOLID, s(2).max(1), rgb(CANVAS));
                        let old = SelectObject(dc, pen);
                        let _ = MoveToEx(dc, box_rect.left + s(3), box_rect.top + s(8), None);
                        let _ = LineTo(dc, box_rect.left + s(6), box_rect.top + s(11));
                        let _ = LineTo(dc, box_rect.left + s(13), box_rect.top + s(4));
                        SelectObject(dc, old);
                        let _ = DeleteObject(pen);
                    }
                }
                if active {
                    let pen = CreatePen(PS_SOLID, s(2).max(1), rgb(ACCENT));
                    let old_pen = SelectObject(dc, pen);
                    let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
                    let _ = RoundRect(dc, r.left, r.top, r.right, r.bottom, s(6), s(6));
                    SelectObject(dc, old_brush);
                    SelectObject(dc, old_pen);
                    let _ = DeleteObject(pen);
                }
            });
        } else {
            let text = window_text(item.hwndItem);
            let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
            let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
            let primary = id == 102;
            let is_tab = matches!(id, TAB_LIBRARY | TAB_SETTINGS);

            let active_tab = is_tab
                && SKIN.with(|s| {
                    let tab = s.borrow().tab;
                    (tab == Tab::Library && id == TAB_LIBRARY)
                        || (tab == Tab::Settings && id == TAB_SETTINGS)
                });

            let color = if disabled {
                0x1c1c1c
            } else if active_tab {
                0x3a2118
            } else if primary {
                if pressed { 0xb84314 } else { ACCENT }
            } else if pressed {
                0x2c2c2c
            } else if is_tab {
                0x181818
            } else {
                0x242424
            };
            let border = if active_tab {
                ACCENT
            } else if is_tab {
                0x242424
            } else if primary {
                color
            } else {
                0x303030
            };
            rounded(dc, r, color, border, s(6));
            let selector = matches!(id, 105 | 106 | FOLDER_FILTER | FPS);
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
                    0x66635f
                } else if is_tab {
                    MUTED
                } else {
                    TEXT
                },
                (if selector { DT_LEFT } else { DT_CENTER })
                    | DT_VCENTER
                    | DT_SINGLELINE
                    | DT_END_ELLIPSIS
                    | DT_NOPREFIX,
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
        fill(dc, pixels(layout.footer), 0x161616);
        label(
            dc,
            "WallpaperLive",
            pixels(layout.title),
            scale(21),
            true,
            TEXT,
            DT_SINGLELINE,
        );
        let (tab, empty_lib) = SKIN.with(|skin| {
            let skin = skin.borrow();
            (skin.tab, skin.tiles.is_empty())
        });
        match tab {
            Tab::Library => {
                label(
                    dc,
                    "Library",
                    pixels(layout.library_title),
                    scale(25),
                    true,
                    TEXT,
                    DT_SINGLELINE,
                );
                label(
                    dc,
                    "Preview a thumbnail. Ctrl-click marks files for removal.",
                    pixels(layout.library_note),
                    scale(13),
                    false,
                    MUTED,
                    DT_SINGLELINE | DT_END_ELLIPSIS,
                );
                if empty_lib {
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
                        "Add a media file or folder. Originals stay where they are.",
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
            }
            Tab::Settings => {
                label(
                    dc,
                    "Settings",
                    pixels(layout.library_title),
                    scale(25),
                    true,
                    TEXT,
                    DT_SINGLELINE,
                );
                label(
                    dc,
                    "Pause, startup and cache preferences",
                    pixels(layout.library_note),
                    scale(13),
                    false,
                    MUTED,
                    DT_SINGLELINE | DT_END_ELLIPSIS,
                );
                label(
                    dc,
                    "Session lock and display sleep always pause playback.",
                    pixels(layout.settings_note),
                    scale(13),
                    false,
                    MUTED,
                    DT_WORDBREAK,
                );
            }
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
        SetTextColor(
            dc,
            rgb(if matches!(id, 113 | VERSION | MEMORY_INFO | CACHE_INFO) {
                MUTED
            } else {
                TEXT
            }),
        );
        let color = if id == 113 {
            0x161616
        } else if matches!(id, 107 | 108 | 121 | VERSION | MEMORY_INFO | CACHE_INFO) {
            CANVAS
        } else if matches!(id, 105 | 106) {
            0x242424
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
                for id in [PREVIEW, 112, 105, 106, FPS, AUDIO, REMOVE, 102, 103, 104] {
                    assert!(scale(layout.sidebar).covers(scale(layout.control(id).unwrap())));
                }
                for id in [PREVIOUS, NEXT, PAGE] {
                    assert!(scale(layout.pager).covers(scale(layout.control(id).unwrap())));
                }
                let selection = layout.control(SELECT_ALL).unwrap();
                assert_eq!(selection.top, layout.control(FOLDER_FILTER).unwrap().top);
                assert!(selection.bottom < layout.library.top);
                assert!(!overlap(scale(selection), scale(layout.pager)));
                assert_eq!(layout.control(PREVIOUS).unwrap().left, layout.pager.left);
            }
            for (i, (id, area)) in layout.controls.iter().enumerate() {
                for (_, other) in &layout.controls[i + 1..] {
                    assert!(
                        !overlap(*area, *other),
                        "control {id} overlaps {other:?} at {width}x{height}"
                    );
                }
            }
            assert!(layout.control(VERSION).unwrap().top > layout.control(108).unwrap().bottom);
            assert!(
                layout.control(MEMORY_INFO).unwrap().top > layout.control(VERSION).unwrap().bottom
            );
            assert!(
                layout.control(CACHE_INFO).unwrap().top
                    > layout.control(MEMORY_INFO).unwrap().bottom
            );
            assert!(
                layout.control(CLEAR_CACHE).unwrap().top
                    > layout.control(CACHE_INFO).unwrap().bottom
            );
            assert_eq!(
                layout.control(VERSION).unwrap().left,
                layout.control(108).unwrap().left
            );
            assert_eq!(
                layout.control(MEMORY_INFO).unwrap().left,
                layout.control(108).unwrap().left
            );
            assert_eq!(
                layout.control(CACHE_INFO).unwrap().left,
                layout.control(108).unwrap().left
            );
            assert_eq!(
                layout.control(CLEAR_CACHE).unwrap().left,
                layout.control(108).unwrap().left
            );
            assert!(layout.control(106).unwrap().bottom < layout.control(FPS).unwrap().top);
            assert!(layout.control(FPS).unwrap().bottom < layout.control(AUDIO).unwrap().top);
            assert_eq!(
                layout.control(107).unwrap().left,
                layout.control(121).unwrap().left
            );
            assert_eq!(
                layout.control(121).unwrap().left,
                layout.control(108).unwrap().left
            );
            assert_eq!(
                layout.control(105).unwrap().bottom - layout.control(105).unwrap().top,
                34
            );
            assert_eq!(layout.control(TILE_BASE), Some(layout.tile_rects[0]));
            assert!(layout.control(TILE_BASE + 9).is_none());
        }
    }
}
