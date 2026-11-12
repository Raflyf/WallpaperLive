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
pub const SEARCH: usize = 133;
pub const SORT: usize = 134;
pub const FIND_WALLPAPERS: usize = 135;
pub const PRODUCT_WEBSITE: usize = 136;
pub const SOURCES_BACK: usize = 137;
pub const SITE_BASE: usize = 140;
pub const SOURCES_NOTE: usize = 144;
pub const SETTINGS_NOTE: usize = 145;
pub const SETTINGS_INFO: usize = 146;
pub const SUPPORT: usize = 147;
pub const SAWERIA: usize = 148;
pub const MIN_CLIENT_WIDTH: i32 = 920;
pub const MIN_CLIENT_HEIGHT: i32 = 640;
pub const HEADER_HEIGHT: i32 = 72;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Library,
    Settings,
    Sources,
}

pub fn visible_on(tab: Tab, id: usize) -> bool {
    match id {
        TAB_LIBRARY | TAB_SETTINGS | 110 | 113 => true,
        101 | ADD_FOLDER => tab != Tab::Settings,
        SOURCES_BACK | SOURCES_NOTE => tab == Tab::Sources,
        id if (SITE_BASE..SITE_BASE + 4).contains(&id) => tab == Tab::Sources,
        107 | 108 | 121 | VERSION | MEMORY_INFO | CACHE_INFO | CLEAR_CACHE | PRODUCT_WEBSITE
        | SETTINGS_NOTE | SETTINGS_INFO | SUPPORT | SAWERIA => tab == Tab::Settings,
        PREVIEW | 112 | 105 | 106 | FPS | AUDIO | REMOVE | 102 | 103 | 104 | SEARCH | SORT
        | FOLDER_FILTER | SELECT_ALL | FIND_WALLPAPERS | PREVIOUS | NEXT | PAGE => {
            tab == Tab::Library
        }
        id if (TILE_BASE..TILE_BASE + 9).contains(&id) => tab == Tab::Library,
        _ => false,
    }
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
    pub settings_title: Rect,
    pub settings_subtitle: Rect,
    pub preferences_title: Rect,
    pub resources_title: Rect,
    pub settings_preferences_panel: Rect,
    pub settings_resources_panel: Rect,
    pub sources_panel: Rect,
    pub sources_title: Rect,
    pub sources_subtitle: Rect,
    pub tile_rects: [Rect; 9],
    controls: [(usize, Rect); 42],
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
        let pause_y = footer.top - 52;
        let pager = r(24, pause_y, split - 48, 36);
        let library = r(24, 248, split - 48, pager.top - 264);
        let tile_col_w = (library.right - library.left - 24) / 3;
        let tile_row_h = (library.bottom - library.top - 24) / 3;
        let tile_rects = std::array::from_fn(|i| {
            r(
                library.left + (i % 3) as i32 * (tile_col_w + 12),
                library.top + (i / 3) as i32 * (tile_row_h + 12),
                tile_col_w,
                tile_row_h,
            )
        });
        let side_x = split + 24;
        let side_width = 296;
        let apply_y = pause_y - 52;
        let audio_y = apply_y - 44;
        let preview_height = (audio_y - 332).clamp(84, side_width * 9 / 16);
        let preview = r(side_x, 128, side_width, preview_height);
        let file = r(side_x, preview.bottom + 8, side_width, 20);
        let display_label = r(side_x, file.bottom + 4, side_width, 16);
        let display = r(side_x, display_label.bottom + 4, side_width, 32);
        let fit_label = r(side_x, display.bottom + 4, side_width, 16);
        let fit = r(side_x, fit_label.bottom + 4, side_width, 32);
        let content_width = (width - 48).min(1040);
        let content_x = (width - content_width) / 2;
        let column_width = (content_width - 32) / 2;
        let resources_x = content_x + column_width + 32;
        let settings_note = r(content_x, 212, column_width, 40);
        let source_width = (width - 48).min(840);
        let source_x = (width - source_width) / 2;
        Self {
            client: r(0, 0, width, height),
            header: r(0, 0, width, HEADER_HEIGHT),
            footer,
            library,
            pager,
            sidebar: r(split, 72, width - split, footer.top - 72),
            title: r(24, 18, 140, 36),
            library_title: r(24, 96, split - 264, 32),
            library_note: r(24, 136, split - 48, 20),
            preview_title: r(side_x, 96, side_width, 24),
            display_label,
            fit_label,
            settings_title: r(content_x, 96, content_width, 32),
            settings_subtitle: r(content_x, 136, content_width, 20),
            preferences_title: r(content_x, 184, column_width, 24),
            resources_title: r(resources_x, 184, column_width, 24),
            settings_preferences_panel: r(content_x - 12, 172, column_width + 24, footer.top - 188),
            settings_resources_panel: r(resources_x - 12, 172, column_width + 24, footer.top - 188),
            sources_panel: r(source_x - 12, 168, source_width + 24, footer.top - 188),
            sources_title: r(source_x, 96, source_width - 196, 32),
            sources_subtitle: r(source_x, 136, source_width, 24),
            tile_rects,
            controls: [
                (101, r(width - 272, 18, 118, 36)),
                (ADD_FOLDER, r(width - 146, 18, 122, 36)),
                (TAB_LIBRARY, r(176, 18, 96, 36)),
                (TAB_SETTINGS, r(280, 18, 96, 36)),
                (SEARCH, r(24, 164, split - 336, 32)),
                (SORT, r(split - 300, 164, 160, 32)),
                (FOLDER_FILTER, r(24, 204, split - 48, 32)),
                (FIND_WALLPAPERS, r(split - 228, 96, 204, 32)),
                (PRODUCT_WEBSITE, r(content_x, 448, column_width, 36)),
                (SUPPORT, r(resources_x, 448, column_width, 36)),
                (SAWERIA, r(resources_x, 496, column_width, 36)),
                (PREVIEW, preview),
                (112, file),
                (105, display),
                (106, fit),
                (AUDIO, r(side_x, audio_y, 142, 32)),
                (REMOVE, r(side_x + 154, audio_y, 142, 32)),
                (102, r(side_x, apply_y, side_width, 40)),
                (103, r(side_x, pause_y, 142, 36)),
                (104, r(side_x + 154, pause_y, 142, 36)),
                (107, r(content_x, 264, column_width, 40)),
                (108, r(content_x, 368, column_width, 40)),
                (121, r(content_x, 316, column_width, 40)),
                (VERSION, r(resources_x, 212, column_width, 24)),
                (MEMORY_INFO, r(resources_x, 248, column_width, 40)),
                (CACHE_INFO, r(resources_x, 300, column_width, 24)),
                (CLEAR_CACHE, r(resources_x, 336, column_width, 36)),
                (FPS, r(side_x, fit.bottom + 12, side_width, 34)),
                (SELECT_ALL, r(split - 128, 164, 104, 32)),
                (PREVIOUS, r(pager.left, pager.top, 88, 36)),
                (NEXT, r(pager.left + 100, pager.top, 88, 36)),
                (
                    PAGE,
                    r(
                        pager.left + 200,
                        pager.top,
                        pager.right - pager.left - 200,
                        36,
                    ),
                ),
                (110, r(width - 104, footer.top + 20, 80, 36)),
                (113, r(24, footer.top + 12, width - 152, 52)),
                (SOURCES_BACK, r(source_x + source_width - 180, 96, 180, 32)),
                (SITE_BASE, r(source_x, 184, source_width, 44)),
                (SITE_BASE + 1, r(source_x, 240, source_width, 44)),
                (SITE_BASE + 2, r(source_x, 296, source_width, 44)),
                (SITE_BASE + 3, r(source_x, 352, source_width, 44)),
                (SOURCES_NOTE, r(source_x, 412, source_width, 112)),
                (SETTINGS_NOTE, settings_note),
                (SETTINGS_INFO, r(resources_x, 384, column_width, 48)),
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

// The three identity colours. Every other shade is mixed from these, so the
// palette cannot drift and no fourth hue can creep in.
pub const INK: u32 = 0x111111;
pub const CREAM: u32 = 0xf3eee3;
pub const ORANGE: u32 = 0xd9541e;

const fn mix_channel(a: u32, b: u32, shift: u32, num: u32, den: u32) -> u32 {
    let x = ((a >> shift) & 255) as i32;
    let y = ((b >> shift) & 255) as i32;
    ((x + (y - x) * num as i32 / den as i32) as u32) << shift
}

/// Linear blend `num/den` of the way from `a` towards `b`.
const fn mix(a: u32, b: u32, num: u32, den: u32) -> u32 {
    mix_channel(a, b, 16, num, den)
        | mix_channel(a, b, 8, num, den)
        | mix_channel(a, b, 0, num, den)
}

pub const TEXT: u32 = INK;
pub const TEXT_ON_DARK: u32 = CREAM;
pub const TEXT_ON_ACCENT: u32 = INK;
pub const MUTED: u32 = mix(INK, CREAM, 32, 100);
pub const MUTED_ON_DARK: u32 = mix(CREAM, INK, 24, 100);
pub const ACCENT: u32 = ORANGE;
// Semantic roles. Body and cards stay opaque; only the header uses GLASS_KEY.
pub const GLASS_KEY: u32 = 0;
pub const BODY: u32 = CREAM;
pub const CANVAS: u32 = mix(CREAM, INK, 4, 100);
pub const FOOTER: u32 = INK;
pub const PANEL: u32 = mix(CREAM, INK, 7, 100);
pub const CARD: u32 = CREAM;
pub const CARD_LIT: u32 = mix(CREAM, INK, 3, 100);
pub const LINE: u32 = mix(CREAM, INK, 20, 100);
pub const HIGHLIGHT: u32 = CREAM;
pub const DISABLED_TEXT: u32 = mix(INK, CREAM, 60, 100);
pub const ACTIVE_SURFACE: u32 = mix(CREAM, ORANGE, 22, 100);
pub const ACCENT_DARK: u32 = mix(ORANGE, INK, 22, 100);
pub const ACCENT_DEEP: u32 = mix(ORANGE, INK, 38, 100);
pub const ACCENT_SOFT: u32 = mix(ORANGE, CREAM, 16, 100);

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
    playing: Vec<PathBuf>,
    library_empty: bool,
    tiles: HashMap<usize, PathBuf>,
    thumbnails: HashMap<PathBuf, Bitmap>,
    tab: Tab,
}
thread_local! {static SKIN:RefCell<Skin>=RefCell::new(Skin{selected:None,marked:Vec::new(),playing:Vec::new(),library_empty:true,tiles:HashMap::new(),thumbnails:HashMap::new(),tab:Tab::Library});}
pub fn library_empty(value: bool) {
    SKIN.with(|s| s.borrow_mut().library_empty = value);
}
pub fn marked(paths: &[PathBuf]) {
    SKIN.with(|skin| skin.borrow_mut().marked = paths.to_vec());
}
pub fn playing(path: Option<PathBuf>) {
    SKIN.with(|s| s.borrow_mut().playing = path.into_iter().collect());
}
pub fn playing_all(paths: &[PathBuf]) {
    SKIN.with(|s| s.borrow_mut().playing = paths.to_vec());
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
        skin.playing.clear();
        skin.library_empty = true;
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
unsafe fn raised_rounded(dc: HDC, r: RECT, color: u32, border: u32, highlight: u32, radius: i32) {
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

        if highlight != 0 && r.right - r.left > radius * 2 && r.bottom - r.top > radius {
            let h_pen = CreatePen(PS_SOLID, 1, rgb(highlight));
            let old = SelectObject(dc, h_pen);
            let _ = MoveToEx(dc, r.left + radius / 2, r.top + 1, None);
            let _ = LineTo(dc, r.right - radius / 2, r.top + 1);
            SelectObject(dc, old);
            let _ = DeleteObject(h_pen);
        }
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
                let playing = path.is_some_and(|path| skin.playing.contains(path));
                let card_color = if id == PREVIEW {
                    INK
                } else if active {
                    ACTIVE_SURFACE
                } else {
                    CARD
                };
                let card_border = if active { ACCENT } else { LINE };
                let card_highlight = if id == PREVIEW {
                    0
                } else if active {
                    ACCENT_SOFT
                } else {
                    HIGHLIGHT
                };
                raised_rounded(dc, r, card_color, card_border, card_highlight, s(10));
                let area = rect(
                    r.left + s(3),
                    r.top + s(3),
                    r.right - s(3),
                    if id == PREVIEW {
                        r.bottom - s(3)
                    } else {
                        r.bottom - s(28)
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
                            if id == PREVIEW { MUTED_ON_DARK } else { MUTED },
                            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                        );
                    }
                    if id != PREVIEW {
                        let title_rect = rect(
                            r.left + s(8),
                            r.bottom - s(24),
                            r.right - s(8),
                            r.bottom - s(4),
                        );
                        label(
                            dc,
                            &filename(path),
                            title_rect,
                            s(13),
                            true,
                            TEXT,
                            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
                        );
                        if playing {
                            let badge_w = s(46);
                            let badge_h = s(18);
                            let badge_rect = rect(
                                area.right - badge_w - s(5),
                                area.bottom - badge_h - s(5),
                                area.right - s(5),
                                area.bottom - s(5),
                            );
                            raised_rounded(dc, badge_rect, ACCENT_DEEP, ACCENT, ACCENT_SOFT, s(4));
                            label(
                                dc,
                                "Active",
                                badge_rect,
                                s(10),
                                true,
                                TEXT_ON_DARK,
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
                        if id == PREVIEW { MUTED_ON_DARK } else { MUTED },
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                    );
                }
                if id != PREVIEW
                    && let Some(path) = path
                {
                    let checked = skin.marked.contains(path);
                    let box_rect = rect(r.left + s(8), r.top + s(8), r.left + s(24), r.top + s(24));
                    rounded(
                        dc,
                        box_rect,
                        if checked { ACCENT } else { PANEL },
                        if checked { ACCENT } else { MUTED },
                        s(4),
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
                    let _ = RoundRect(dc, r.left, r.top, r.right, r.bottom, s(10), s(10));
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
                    (tab != Tab::Settings && id == TAB_LIBRARY)
                        || (tab == Tab::Settings && id == TAB_SETTINGS)
                });

            let radius = if is_tab { s(18) } else { s(8) };
            let (color, border, highlight) = if disabled {
                (CANVAS, PANEL, 0)
            } else if is_tab {
                if active_tab {
                    (ACCENT, ACCENT_DARK, ACCENT_SOFT)
                } else if pressed {
                    (mix(INK, CREAM, 8, 100), LINE, 0)
                } else {
                    (INK, LINE, 0)
                }
            } else if primary {
                if pressed {
                    (ACCENT_DARK, ACCENT_DEEP, 0)
                } else {
                    (ACCENT, ACCENT_DARK, ACCENT_SOFT)
                }
            } else if pressed {
                (CARD_LIT, HIGHLIGHT, 0)
            } else {
                (CARD, LINE, HIGHLIGHT)
            };
            raised_rounded(dc, r, color, border, highlight, radius);
            let selector = matches!(id, 105 | 106 | FOLDER_FILTER | FPS | SORT);
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
                primary || active_tab,
                if disabled {
                    DISABLED_TEXT
                } else if primary && pressed {
                    TEXT_ON_DARK
                } else if primary || (is_tab && active_tab) {
                    TEXT_ON_ACCENT
                } else if is_tab {
                    TEXT_ON_DARK
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
        if item.itemState.0 & ODS_FOCUS.0 != 0 && item.itemState.0 & ODS_DISABLED.0 == 0 {
            let focus_r = if matches!(id, TAB_LIBRARY | TAB_SETTINGS) {
                s(14)
            } else {
                s(6)
            };
            let focus = rect(r.left + s(3), r.top + s(3), r.right - s(3), r.bottom - s(3));
            let focus_color = if matches!(id, TAB_LIBRARY | TAB_SETTINGS) {
                if SKIN.with(|s| {
                    let tab = s.borrow().tab;
                    (id == TAB_LIBRARY && tab != Tab::Settings)
                        || (id == TAB_SETTINGS && tab == Tab::Settings)
                }) {
                    INK
                } else {
                    TEXT_ON_DARK
                }
            } else {
                INK
            };
            let pen = CreatePen(PS_SOLID, s(2).max(1), rgb(focus_color));
            let old_pen = SelectObject(dc, pen);
            let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
            let _ = RoundRect(
                dc,
                focus.left,
                focus.top,
                focus.right,
                focus.bottom,
                focus_r,
                focus_r,
            );
            SelectObject(dc, old_brush);
            SelectObject(dc, old_pen);
            let _ = DeleteObject(pen);
        }
    }
}

pub fn enable_backdrop(window: HWND) -> bool {
    use windows::Win32::{Graphics::Dwm::*, UI::Controls::MARGINS};
    // SAFETY: window is the app-owned top-level HWND; all payloads are POD values.
    unsafe {
        let dark = 1i32;
        let round = DWMWCP_ROUND.0;
        let acrylic = DWMSBT_TRANSIENTWINDOW.0;
        let header_height = HEADER_HEIGHT * GetDpiForWindow(window).max(96) as i32 / 96;
        let margins = MARGINS {
            cxLeftWidth: 0,
            cxRightWidth: 0,
            cyTopHeight: header_height,
            cyBottomHeight: 0,
        };
        let dark_ok = DwmSetWindowAttribute(
            window,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark as *const i32).cast(),
            std::mem::size_of_val(&dark) as u32,
        )
        .is_ok();
        let round_ok = DwmSetWindowAttribute(
            window,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&round as *const i32).cast(),
            std::mem::size_of_val(&round) as u32,
        )
        .is_ok();
        let acrylic_ok = DwmSetWindowAttribute(
            window,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&acrylic as *const i32).cast(),
            std::mem::size_of_val(&acrylic) as u32,
        )
        .is_ok();
        let frame_ok = DwmExtendFrameIntoClientArea(window, &margins).is_ok();
        dark_ok && round_ok && acrylic_ok && frame_ok
    }
}

pub unsafe fn paint(window: HWND) {
    // SAFETY: UI-thread BeginPaint/EndPaint pair; supplied HDC is valid until EndPaint.
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(window, &mut ps);
        paint_into(window, dc);
        let _ = EndPaint(window, &ps);
    }
}

pub unsafe fn paint_into(window: HWND, dc: HDC) {
    // SAFETY: caller supplies a valid drawing HDC; this function owns no paint session.
    unsafe {
        let mut client = RECT::default();
        let _ = GetClientRect(window, &mut client);
        let dpi = GetDpiForWindow(window).max(96) as i32;
        let scale = |n: i32| n * dpi / 96;
        let pixels = |r: Rect| rect(scale(r.left), scale(r.top), scale(r.right), scale(r.bottom));
        let layout = Layout::new(client.right * 96 / dpi, client.bottom * 96 / dpi);
        let (tab, empty_view, empty_library) = SKIN.with(|skin| {
            let skin = skin.borrow();
            (skin.tab, skin.tiles.is_empty(), skin.library_empty)
        });
        fill(dc, pixels(layout.client), BODY);
        fill(dc, pixels(layout.header), GLASS_KEY);
        fill(
            dc,
            rect(
                0,
                scale(layout.header.bottom - 1),
                scale(layout.client.right),
                scale(layout.header.bottom),
            ),
            LINE,
        );
        match tab {
            Tab::Library => {
                fill(dc, pixels(layout.sidebar), CARD);
                fill(
                    dc,
                    rect(
                        scale(layout.sidebar.left),
                        scale(layout.header.bottom),
                        scale(layout.sidebar.left + 1),
                        scale(layout.footer.top),
                    ),
                    LINE,
                );
                fill(dc, pixels(layout.pager), PANEL);
            }
            Tab::Settings => {
                raised_rounded(
                    dc,
                    pixels(layout.settings_preferences_panel),
                    CARD,
                    LINE,
                    HIGHLIGHT,
                    scale(12),
                );
                raised_rounded(
                    dc,
                    pixels(layout.settings_resources_panel),
                    CARD,
                    LINE,
                    HIGHLIGHT,
                    scale(12),
                );
            }
            Tab::Sources => {
                raised_rounded(
                    dc,
                    pixels(layout.sources_panel),
                    CARD,
                    LINE,
                    HIGHLIGHT,
                    scale(12),
                );
            }
        }
        fill(dc, pixels(layout.footer), FOOTER);
        fill(
            dc,
            rect(
                0,
                scale(layout.footer.top),
                scale(layout.client.right),
                scale(layout.footer.top + 1),
            ),
            LINE,
        );
        label(
            dc,
            "WallpaperLive",
            pixels(layout.title),
            scale(21),
            true,
            TEXT_ON_DARK,
            DT_SINGLELINE,
        );
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
                    "Bring your own live wallpapers.",
                    pixels(layout.library_note),
                    scale(13),
                    false,
                    MUTED,
                    DT_SINGLELINE | DT_END_ELLIPSIS,
                );
                if empty_view {
                    let area = layout.library;
                    label(
                        dc,
                        if empty_library {
                            "No wallpapers yet"
                        } else {
                            "No matching wallpapers"
                        },
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
                        if empty_library {
                            "Add a media file or folder. Originals stay where they are."
                        } else {
                            "Clear the search or choose All folders to see your library."
                        },
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
                        if empty_library {
                            "MP4  GIF  MKV  WebM  MOV  AVI"
                        } else {
                            "Your files remain in the library."
                        },
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
                    pixels(layout.settings_title),
                    scale(25),
                    true,
                    TEXT,
                    DT_SINGLELINE,
                );
                label(
                    dc,
                    "Playback preferences and application details",
                    pixels(layout.settings_subtitle),
                    scale(13),
                    false,
                    MUTED,
                    DT_SINGLELINE,
                );
                label(
                    dc,
                    "Playback & startup",
                    pixels(layout.preferences_title),
                    scale(16),
                    true,
                    TEXT,
                    DT_SINGLELINE,
                );
                label(
                    dc,
                    "Resources & support",
                    pixels(layout.resources_title),
                    scale(16),
                    true,
                    TEXT,
                    DT_SINGLELINE,
                );
            }
            Tab::Sources => {
                label(
                    dc,
                    "Where to find wallpapers",
                    pixels(layout.sources_title),
                    scale(25),
                    true,
                    TEXT,
                    DT_SINGLELINE,
                );
                label(
                    dc,
                    "Browse in your browser, then import local media.",
                    pixels(layout.sources_subtitle),
                    scale(14),
                    false,
                    MUTED,
                    DT_SINGLELINE,
                );
            }
        }
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
        if id == 113 {
            SetBkMode(dc, OPAQUE);
        } else {
            SetBkMode(dc, TRANSPARENT);
        }
        SetTextColor(
            dc,
            rgb(if id == 113 {
                TEXT_ON_DARK
            } else if matches!(
                id,
                VERSION | MEMORY_INFO | CACHE_INFO | SOURCES_NOTE | SETTINGS_NOTE | SETTINGS_INFO
            ) {
                MUTED
            } else {
                TEXT
            }),
        );
        let color = if id == 113 {
            FOOTER
        } else if matches!(
            id,
            107 | 108
                | 121
                | VERSION
                | MEMORY_INFO
                | CACHE_INFO
                | SOURCES_NOTE
                | SETTINGS_NOTE
                | SETTINGS_INFO
                | PAGE
        ) {
            PANEL
        } else if matches!(id, 105 | 106 | SEARCH) {
            CARD
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
    fn identity_colors_anchor_semantic_roles() {
        assert_eq!((INK, CREAM, ORANGE), (0x111111, 0xf3eee3, 0xd9541e));
        assert_eq!((BODY, CARD, HIGHLIGHT), (CREAM, CREAM, CREAM));
        assert_eq!((TEXT, TEXT_ON_ACCENT, FOOTER), (INK, INK, INK));
        assert_eq!((TEXT_ON_DARK, ACCENT), (CREAM, ORANGE));
    }

    #[test]
    fn thumbnail_blit_preserves_source_rgb() {
        // SAFETY: test owns both memory DCs and DIB sections for the whole draw.
        unsafe {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: 4,
                    biHeight: -4,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let target_dc = CreateCompatibleDC(None);
            assert!(!target_dc.0.is_null());
            let mut source_bits = std::ptr::null_mut();
            let mut target_bits = std::ptr::null_mut();
            let source = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut source_bits, None, 0)
                .expect("source DIB");
            let target =
                CreateDIBSection(target_dc, &info, DIB_RGB_COLORS, &mut target_bits, None, 0)
                    .expect("target DIB");
            std::slice::from_raw_parts_mut(source_bits as *mut u32, 16).fill(0xff2a7bc4);
            let old_target = SelectObject(target_dc, target);
            image(target_dc, source, rect(0, 0, 4, 4));
            let _ = GdiFlush();
            let pixel = *std::slice::from_raw_parts(target_bits as *const u32, 16)
                .get(5)
                .unwrap();
            assert_eq!(pixel & 0x00ff_ffff, 0x002a_7bc4);
            SelectObject(target_dc, old_target);
            let _ = DeleteObject(source);
            let _ = DeleteObject(target);
            let _ = DeleteDC(target_dc);
        }
    }

    #[test]
    fn layout_reserves_pager_and_footer_at_all_supported_sizes() {
        for (width, height) in [
            (MIN_CLIENT_WIDTH, MIN_CLIENT_HEIGHT),
            (1024, 680),
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
                    assert!(tile.right - tile.left >= 168 && tile.bottom - tile.top >= 74);
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
                assert_eq!(selection.top, layout.control(SEARCH).unwrap().top);
                assert_eq!(selection.top, layout.control(SORT).unwrap().top);
                let folder = layout.control(FOLDER_FILTER).unwrap();
                assert!(selection.bottom < folder.top);
                assert!(folder.bottom < layout.library.top);
                assert!(layout.control(SEARCH).unwrap().right < layout.control(SORT).unwrap().left);
                assert!(layout.control(SORT).unwrap().right < selection.left);
                assert!(!overlap(scale(selection), scale(layout.pager)));
                assert!(layout.control(FIND_WALLPAPERS).unwrap().left > layout.library_title.right);
                assert!(
                    layout.control(PRODUCT_WEBSITE).unwrap().top
                        > layout.control(CLEAR_CACHE).unwrap().bottom
                );
                assert!(layout.control(PRODUCT_WEBSITE).unwrap().bottom <= layout.pager.top);
                assert_eq!(layout.control(PREVIOUS).unwrap().left, layout.pager.left);
            }
            for tab in [Tab::Library, Tab::Settings, Tab::Sources] {
                for (i, (id, area)) in layout.controls.iter().enumerate() {
                    if !visible_on(tab, *id) {
                        continue;
                    }
                    for (other_id, other) in &layout.controls[i + 1..] {
                        if visible_on(tab, *other_id) {
                            assert!(
                                !overlap(*area, *other),
                                "{tab:?}: {id} overlaps {other_id} at {width}x{height}"
                            );
                        }
                    }
                    if tab == Tab::Library {
                        for tile in layout.tile_rects {
                            assert!(!overlap(*area, tile), "control {id} overlaps a tile");
                        }
                    }
                }
            }
            let area = |id| layout.control(id).unwrap();
            assert!(visible_on(Tab::Settings, SAWERIA));
            assert!(!visible_on(Tab::Library, SAWERIA));
            assert!(!visible_on(Tab::Sources, SAWERIA));
            assert_eq!(area(SAWERIA).left, area(SUPPORT).left);
            assert_eq!(area(SAWERIA).right, area(SUPPORT).right);
            assert_eq!(area(SAWERIA).top, area(SUPPORT).bottom + 12);
            assert!(area(SAWERIA).bottom + 24 <= layout.footer.top);
            for (left, right) in [(AUDIO, REMOVE), (103, 104), (PRODUCT_WEBSITE, SUPPORT)] {
                assert_eq!(area(left).top, area(right).top);
                assert_eq!(area(left).bottom, area(right).bottom);
                assert_eq!(
                    area(left).right - area(left).left,
                    area(right).right - area(right).left
                );
            }
            assert_eq!(layout.pager.top, area(103).top);
            assert_eq!(layout.preferences_title.top, layout.resources_title.top);
            assert_eq!(area(107).left, area(108).left);
            assert_eq!(area(107).left, area(121).left);
            assert_eq!(area(MEMORY_INFO).left, area(CACHE_INFO).left);
            assert!(area(FPS).bottom + 12 <= area(AUDIO).top);
            assert_eq!(area(SOURCES_BACK).top, layout.sources_title.top);
            for id in SITE_BASE..SITE_BASE + 4 {
                assert_eq!(area(id).left, area(SOURCES_NOTE).left);
                assert_eq!(area(id).right, area(SOURCES_NOTE).right);
                assert_eq!(area(id).bottom - area(id).top, 44);
                assert!(area(id).bottom < area(SOURCES_NOTE).top);
            }
            assert!(area(SOURCES_NOTE).bottom < layout.footer.top);
            assert!(area(SETTINGS_INFO).bottom < area(SUPPORT).top);
            assert_eq!(area(105).bottom - area(105).top, 32);
            assert_eq!(layout.control(TILE_BASE), Some(layout.tile_rects[0]));
            assert!(layout.control(TILE_BASE + 9).is_none());
        }
    }
}
