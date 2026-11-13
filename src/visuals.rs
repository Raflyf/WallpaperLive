use crate::core::Rect;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::*,
        UI::{
            Controls::{
                BP_PAINTPARAMS, BPBF_TOPDOWNDIB, BPPF_ERASE, BeginBufferedPaint, BufferedPaintInit,
                BufferedPaintSetAlpha, BufferedPaintUnInit, CloseThemeData, DRAWITEMSTRUCT,
                DTT_COMPOSITED, DTT_TEXTCOLOR, DTTOPTS, DrawThemeTextEx, EM_GETSEL,
                EndBufferedPaint, GetBufferedPaintBits, HTHEME, ODS_DISABLED, ODS_FOCUS,
                ODS_SELECTED, OpenThemeData,
            },
            HiDpi::GetDpiForWindow,
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
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
    #[cfg_attr(not(test), allow(dead_code))]
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

pub const TEXT: u32 = CREAM;
pub const TEXT_ON_DARK: u32 = CREAM;
pub const TEXT_ON_ACCENT: u32 = INK;
pub const MUTED: u32 = mix(CREAM, INK, 16, 100);
pub const MUTED_ON_DARK: u32 = MUTED;
pub const ACCENT: u32 = ORANGE;
pub const BODY: u32 = INK;
pub const CANVAS: u32 = INK;
pub const FOOTER: u32 = INK;
pub const PANEL: u32 = mix(INK, CREAM, 6, 100);
pub const CARD: u32 = mix(INK, CREAM, 8, 100);
pub const CARD_LIT: u32 = mix(INK, CREAM, 14, 100);
pub const LINE: u32 = mix(INK, CREAM, 24, 100);
pub const HIGHLIGHT: u32 = mix(INK, CREAM, 38, 100);
pub const DISABLED_TEXT: u32 = mix(CREAM, INK, 48, 100);
const DISABLED_FILL: u32 = mix(INK, CREAM, 4, 100);
const DISABLED_BORDER: u32 = mix(INK, CREAM, 12, 100);
pub const ACTIVE_SURFACE: u32 = mix(INK, ORANGE, 16, 100);
pub const ACCENT_DARK: u32 = mix(ORANGE, INK, 22, 100);
pub const ACCENT_DEEP: u32 = mix(ORANGE, INK, 38, 100);
pub const ACCENT_SOFT: u32 = mix(ORANGE, CREAM, 16, 100);
pub const PANEL_ALPHA: u8 = 196;
const ODS_NOFOCUSRECT_FLAG: u32 = 0x0200;
const UISF_HIDEFOCUS_FLAG: isize = 0x0001;

fn control_corner_ellipse(id: usize, dpi: i32) -> i32 {
    let base = if matches!(id, TAB_LIBRARY | TAB_SETTINGS) {
        18
    } else if id == PREVIEW || (TILE_BASE..TILE_BASE + 9).contains(&id) {
        12
    } else {
        10
    };
    base * dpi.max(96) / 96
}

fn control_surface(id: usize) -> (u32, bool) {
    if matches!(
        id,
        PREVIEW
            | 112
            | 105
            | 106
            | FPS
            | AUDIO
            | REMOVE
            | 102
            | 103
            | 104
            | 107
            | 108
            | 121
            | PRODUCT_WEBSITE
            | CLEAR_CACHE
            | SUPPORT
            | SAWERIA
            | VERSION
            | MEMORY_INFO
            | CACHE_INFO
            | SETTINGS_NOTE
            | SETTINGS_INFO
            | SOURCES_NOTE
    ) || (SITE_BASE..SITE_BASE + 4).contains(&id)
    {
        (CARD, false)
    } else {
        (CANVAS, true)
    }
}
const GLASS_PROPERTY: windows::core::PCWSTR = w!("WallpaperLive.Glass");

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
    FONTS.with(|f| {
        for (_, font) in f.borrow_mut().drain() {
            unsafe {
                let _ = DeleteObject(font);
            }
        }
    });
}

struct PaintRuntime {
    initialized: bool,
}
impl PaintRuntime {
    fn new() -> Self {
        Self {
            initialized: unsafe { BufferedPaintInit() }.is_ok(),
        }
    }
}
impl Drop for PaintRuntime {
    fn drop(&mut self) {
        if self.initialized {
            unsafe {
                let _ = BufferedPaintUnInit();
            }
        }
    }
}
thread_local! {
    static PAINT_RUNTIME: PaintRuntime = PaintRuntime::new();
    static GLASS_BUFFER: Cell<isize> = const { Cell::new(0) };
    static TEXT_THEME: Cell<HTHEME> = const { Cell::new(HTHEME(0)) };
}
struct PaintBuffer {
    handle: isize,
    dc: HDC,
    area: RECT,
}
impl PaintBuffer {
    unsafe fn new(target: HDC, area: RECT) -> Option<Self> {
        if target.0.is_null()
            || area.right <= area.left
            || area.bottom <= area.top
            || !PAINT_RUNTIME.with(|runtime| runtime.initialized)
        {
            return None;
        }
        let params = BP_PAINTPARAMS {
            cbSize: std::mem::size_of::<BP_PAINTPARAMS>() as u32,
            dwFlags: BPPF_ERASE,
            ..Default::default()
        };
        let mut dc = HDC::default();
        let handle =
            unsafe { BeginBufferedPaint(target, &area, BPBF_TOPDOWNDIB, Some(&params), &mut dc) };
        if handle == 0 {
            return None;
        }
        if dc.0.is_null() {
            unsafe {
                let _ = EndBufferedPaint(handle, false);
            }
            return None;
        }
        Some(Self { handle, dc, area })
    }
    unsafe fn opaque(&self) -> bool {
        unsafe { GdiFlush().as_bool() && BufferedPaintSetAlpha(self.handle, None, 255).is_ok() }
    }
    unsafe fn tint_background(&self, area: RECT, alpha: u8) -> bool {
        let width = self.area.right - self.area.left;
        let height = self.area.bottom - self.area.top;
        let mut bits = std::ptr::null_mut();
        let mut stride = 0;
        if unsafe { GetBufferedPaintBits(self.handle, &mut bits, &mut stride) }.is_err()
            || bits.is_null()
            || stride < width
            || !unsafe { GdiFlush() }.as_bool()
        {
            return false;
        }
        let Some(len) = (stride as usize)
            .checked_mul(height as usize)
            .filter(|len| *len <= isize::MAX as usize / std::mem::size_of::<RGBQUAD>())
        else {
            return false;
        };
        // SAFETY: GetBufferedPaintBits owns stride * buffer height RGBQUADs until EndBufferedPaint.
        let pixels = unsafe { std::slice::from_raw_parts_mut(bits, len) };
        for y in area.top.max(self.area.top)..area.bottom.min(self.area.bottom) {
            for x in area.left.max(self.area.left)..area.right.min(self.area.right) {
                let i =
                    (y - self.area.top) as usize * stride as usize + (x - self.area.left) as usize;
                let pixel = &mut pixels[i];
                if pixel.rgbReserved == 0 {
                    pixel.rgbBlue = (u16::from(pixel.rgbBlue) * u16::from(alpha) / 255) as u8;
                    pixel.rgbGreen = (u16::from(pixel.rgbGreen) * u16::from(alpha) / 255) as u8;
                    pixel.rgbRed = (u16::from(pixel.rgbRed) * u16::from(alpha) / 255) as u8;
                    pixel.rgbReserved = alpha;
                }
            }
        }
        true
    }
    unsafe fn apply_rounded_mask(&self, mask: &RoundedMask) -> bool {
        unsafe { self.apply_rounded_rect_mask(self.area, mask) }
    }
    unsafe fn apply_rounded_rect_mask(&self, bounds: RECT, mask: &RoundedMask) -> bool {
        let width = self.area.right - self.area.left;
        let height = self.area.bottom - self.area.top;
        let bounds_w = bounds.right - bounds.left;
        let bounds_h = bounds.bottom - bounds.top;
        let rect_left = bounds.left.max(self.area.left);
        let rect_top = bounds.top.max(self.area.top);
        let rect_right = bounds.right.min(self.area.right);
        let rect_bottom = bounds.bottom.min(self.area.bottom);
        if width <= 0
            || height <= 0
            || bounds_w <= 0
            || bounds_h <= 0
            || rect_right <= rect_left
            || rect_bottom <= rect_top
        {
            return true;
        }
        let mut bits = std::ptr::null_mut();
        let mut stride = 0;
        if unsafe { GetBufferedPaintBits(self.handle, &mut bits, &mut stride) }.is_err()
            || bits.is_null()
            || stride < width
            || !unsafe { GdiFlush() }.as_bool()
        {
            return false;
        }
        let Some(len) = (stride as usize)
            .checked_mul(height as usize)
            .filter(|len| *len <= isize::MAX as usize / std::mem::size_of::<RGBQUAD>())
        else {
            return false;
        };
        // SAFETY: GetBufferedPaintBits owns stride * buffer height RGBQUADs until EndBufferedPaint.
        let pixels = unsafe { std::slice::from_raw_parts_mut(bits, len) };
        let w = bounds_w as f32;
        let h = bounds_h as f32;
        let hw = w * 0.5;
        let hh = h * 0.5;
        let radius = (mask.ellipse as f32 * 0.5).min(hw).min(hh);
        let split_rgb = |c: u32| -> (f32, f32, f32) {
            (
                ((c >> 16) & 255) as f32,
                ((c >> 8) & 255) as f32,
                (c & 255) as f32,
            )
        };
        let (br, bg, bb) = split_rgb(mask.border);
        let (fr, fg, fb) = split_rgb(mask.focus);
        let (hr, hg, hb) = split_rgb(mask.host_color);
        let ha = f32::from(mask.host_alpha);
        let host_pre_r = hr * ha / 255.0;
        let host_pre_g = hg * ha / 255.0;
        let host_pre_b = hb * ha / 255.0;
        let safe_inset = mask.border_width
            + if mask.focus != 0 {
                mask.focus_inset + 2.0
            } else {
                1.0
            };
        let corner_band = (radius + safe_inset).ceil() as i32;
        let edge_band = (safe_inset + 1.0).ceil() as i32;
        let focus_center = -(mask.border_width + mask.focus_inset - 0.5);
        for y in rect_top..rect_bottom {
            let row_offset = (y - self.area.top) as usize * stride as usize;
            let col_start = (rect_left - self.area.left) as usize;
            let col_end = (rect_right - self.area.left) as usize;
            let row = &mut pixels[row_offset + col_start..row_offset + col_end];
            let yi = y - bounds.top;
            let py = yi as f32 + 0.5;
            let y_inner = yi >= edge_band && yi < bounds_h - edge_band;
            let y_deep = yi >= corner_band && yi < bounds_h - corner_band;
            for (offset, pixel) in row.iter_mut().enumerate() {
                let xi = rect_left - bounds.left + offset as i32;
                if y_deep && xi >= edge_band && xi < bounds_w - edge_band
                    || y_inner && xi >= corner_band && xi < bounds_w - corner_band
                {
                    pixel.rgbReserved = 255;
                    continue;
                }
                let px = xi as f32 + 0.5;
                let qx = (px - hw).abs() - (hw - radius);
                let qy = (py - hh).abs() - (hh - radius);
                let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
                if d <= -safe_inset {
                    pixel.rgbReserved = 255;
                    continue;
                }
                let mut src_r = f32::from(pixel.rgbRed);
                let mut src_g = f32::from(pixel.rgbGreen);
                let mut src_b = f32::from(pixel.rgbBlue);
                if mask.focus != 0 {
                    let focus_cov = (1.0 - (d - focus_center).abs()).clamp(0.0, 1.0);
                    if focus_cov > 0.0 {
                        src_r += (fr - src_r) * focus_cov;
                        src_g += (fg - src_g) * focus_cov;
                        src_b += (fb - src_b) * focus_cov;
                    }
                }
                let shape_cov = (0.5 - d).clamp(0.0, 1.0);
                let inner_cov = (0.5 - (d + mask.border_width)).clamp(0.0, 1.0);
                let fg_r = br + (src_r - br) * inner_cov;
                let fg_g = bg + (src_g - bg) * inner_cov;
                let fg_b = bb + (src_b - bb) * inner_cov;
                let out_a = (ha * (1.0 - shape_cov) + 255.0 * shape_cov)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let blend = |f: f32, hp: f32| -> u8 {
                    (hp * (1.0 - shape_cov) + f * shape_cov)
                        .round()
                        .clamp(0.0, f32::from(out_a)) as u8
                };
                pixel.rgbRed = blend(fg_r, host_pre_r);
                pixel.rgbGreen = blend(fg_g, host_pre_g);
                pixel.rgbBlue = blend(fg_b, host_pre_b);
                pixel.rgbReserved = out_a;
            }
        }
        true
    }
    unsafe fn finish(mut self) -> bool {
        let result = unsafe { EndBufferedPaint(self.handle, true) }.is_ok();
        self.handle = 0;
        result
    }
}
impl Drop for PaintBuffer {
    fn drop(&mut self) {
        if self.handle != 0 {
            unsafe {
                let _ = EndBufferedPaint(self.handle, false);
            }
        }
    }
}

pub fn glass_enabled(window: HWND) -> bool {
    !window.0.is_null() && unsafe { GetPropW(window, GLASS_PROPERTY).0 == window.0 }
}

pub unsafe fn fill(dc: HDC, r: RECT, color: u32) {
    // SAFETY: cached brush belongs to this UI thread and outlives synchronous fill.
    unsafe {
        FillRect(dc, &r, brush(color));
    }
}
unsafe fn rounded(dc: HDC, r: RECT, color: u32, border: u32, radius: i32) {
    // SAFETY: restore native objects before deleting the created pen; brush is thread-cached.
    unsafe {
        let pen = CreatePen(PS_SOLID, 1, rgb(border));
        let b = SelectObject(dc, brush(color));
        let p = SelectObject(dc, pen);
        let _ = RoundRect(dc, r.left, r.top, r.right, r.bottom, radius, radius);
        SelectObject(dc, b);
        SelectObject(dc, p);
        let _ = DeleteObject(pen);
    }
}
unsafe fn raised_rounded(dc: HDC, r: RECT, color: u32, border: u32, highlight: u32, radius: i32) {
    // SAFETY: restore native objects before deleting created pens; brush is thread-cached.
    unsafe {
        rounded(dc, r, color, border, radius);
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
    if text.is_empty() || r.right <= r.left || r.bottom <= r.top {
        return;
    }
    // SAFETY: drawing borrows temporary UTF-16 buffer and thread-cached font until return.
    unsafe {
        let font = cached_font(size, bold);
        let old = SelectObject(dc, font);
        SetTextColor(dc, rgb(color));
        SetBkMode(dc, TRANSPARENT);
        let mut text: Vec<u16> = text.encode_utf16().collect();
        let mut r = r;
        let theme = TEXT_THEME.with(Cell::get);
        let painted = theme.0 != 0
            && DrawThemeTextEx(
                theme,
                dc,
                0,
                0,
                &text,
                flags | DT_NOPREFIX,
                &mut r,
                Some(&DTTOPTS {
                    dwSize: std::mem::size_of::<DTTOPTS>() as u32,
                    dwFlags: DTT_COMPOSITED | DTT_TEXTCOLOR,
                    crText: rgb(color),
                    ..Default::default()
                }),
            )
            .is_ok();
        if !painted {
            DrawTextW(dc, &mut text, &mut r, flags | DT_NOPREFIX);
            let buffer = GLASS_BUFFER.with(Cell::get);
            if buffer != 0 {
                let _ = GdiFlush();
                let _ = BufferedPaintSetAlpha(buffer, Some(&r), 255);
            }
        }
        SelectObject(dc, old);
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

struct RoundedMask {
    ellipse: i32,
    border: u32,
    border_width: f32,
    focus: u32,
    focus_inset: f32,
    host_color: u32,
    host_alpha: u8,
}

pub unsafe fn draw_item(item: &DRAWITEMSTRUCT) {
    // SAFETY: the owner-draw DC stays borrowed until this synchronous dispatch returns.
    unsafe {
        let parent = if !item.hwndItem.0.is_null() {
            GetParent(item.hwndItem).unwrap_or_default()
        } else {
            HWND::default()
        };
        if item.rcItem.right <= item.rcItem.left
            || item.rcItem.bottom <= item.rcItem.top
            || (!parent.0.is_null() && IsIconic(parent).as_bool())
        {
            return;
        }
        if let Some(buffer) = PaintBuffer::new(item.hDC, item.rcItem) {
            let buffered = DRAWITEMSTRUCT {
                hDC: buffer.dc,
                ..*item
            };
            let mask = draw_item_into(&buffered, glass_enabled(parent), true);
            let ok = match mask {
                Some(mask) => buffer.apply_rounded_mask(&mask),
                None => buffer.opaque(),
            };
            if ok && buffer.finish() {
                return;
            }
        }
        if !parent.0.is_null() {
            disable_backdrop(parent);
        }
        let _ = draw_item_into(item, false, false);
    }
}

unsafe fn draw_item_into(
    item: &DRAWITEMSTRUCT,
    glass_active: bool,
    buffered: bool,
) -> Option<RoundedMask> {
    let id = item.CtlID as usize;
    let dpi = unsafe { GetDpiForWindow(item.hwndItem) }.max(96) as i32;
    let s = |n: i32| n * dpi / 96;
    let r = item.rcItem;
    let dc = item.hDC;
    let (host_color, glass_host) = control_surface(id);
    let host_alpha = if glass_host && glass_active {
        PANEL_ALPHA
    } else {
        255
    };
    let ellipse = control_corner_ellipse(id, dpi);
    // SAFETY: child rect pixels remain owned for the duration of synchronous owner-draw dispatch.
    unsafe {
        if id == PAGE {
            fill(dc, r, host_color);
            label(
                dc,
                &window_text(item.hwndItem),
                r,
                s(13),
                false,
                MUTED,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            return None;
        }
        let hide_focus = item.itemState.0 & ODS_NOFOCUSRECT_FLAG != 0
            || (!item.hwndItem.0.is_null() && {
                let parent = GetParent(item.hwndItem).unwrap_or_default();
                !parent.0.is_null()
                    && SendMessageW(parent, WM_QUERYUISTATE, WPARAM(0), LPARAM(0)).0
                        & UISF_HIDEFOCUS_FLAG
                        != 0
            });
        let focus_visible = item.itemState.0 & ODS_FOCUS.0 != 0
            && item.itemState.0 & ODS_DISABLED.0 == 0
            && !hide_focus;
        let is_tab = matches!(id, TAB_LIBRARY | TAB_SETTINGS);
        let active_tab = is_tab
            && SKIN.with(|s| {
                let tab = s.borrow().tab;
                (tab != Tab::Settings && id == TAB_LIBRARY)
                    || (tab == Tab::Settings && id == TAB_SETTINGS)
            });
        let focus_color = if !focus_visible {
            0
        } else if (is_tab && active_tab) || (id == 102 && item.itemState.0 & ODS_SELECTED.0 == 0) {
            TEXT_ON_DARK
        } else {
            ACCENT_SOFT
        };
        if id == PREVIEW || (TILE_BASE..TILE_BASE + 9).contains(&id) {
            let mut active_tile = false;
            SKIN.with(|skin| {
                let skin = skin.borrow();
                let path = if id == PREVIEW {
                    skin.selected.as_ref()
                } else {
                    skin.tiles.get(&id)
                };
                let active = id != PREVIEW && path.is_some() && path == skin.selected.as_ref();
                active_tile = active;
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
                if buffered {
                    fill(dc, r, card_color);
                    if card_highlight != 0 && r.right - r.left > ellipse {
                        let h_pen = CreatePen(PS_SOLID, 1, rgb(card_highlight));
                        let old = SelectObject(dc, h_pen);
                        let _ = MoveToEx(dc, r.left + ellipse / 2, r.top + 1, None);
                        let _ = LineTo(dc, r.right - ellipse / 2, r.top + 1);
                        SelectObject(dc, old);
                        let _ = DeleteObject(h_pen);
                    }
                } else {
                    fill(dc, r, host_color);
                    raised_rounded(dc, r, card_color, card_border, card_highlight, ellipse);
                }
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
                        if checked { ACCENT } else { INK },
                        if checked { ACCENT_DARK } else { LINE },
                        s(6),
                    );
                    if checked {
                        let pen = CreatePen(PS_SOLID, s(2).max(1), rgb(INK));
                        let old = SelectObject(dc, pen);
                        let _ = MoveToEx(dc, box_rect.left + s(4), box_rect.top + s(8), None);
                        let _ = LineTo(dc, box_rect.left + s(7), box_rect.top + s(11));
                        let _ = LineTo(dc, box_rect.left + s(12), box_rect.top + s(5));
                        SelectObject(dc, old);
                        let _ = DeleteObject(pen);
                    }
                }
                if !buffered && active {
                    let pen = CreatePen(PS_SOLID, s(2).max(1), rgb(ACCENT));
                    let old_pen = SelectObject(dc, pen);
                    let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
                    let _ = RoundRect(dc, r.left, r.top, r.right, r.bottom, ellipse, ellipse);
                    SelectObject(dc, old_brush);
                    SelectObject(dc, old_pen);
                    let _ = DeleteObject(pen);
                }
            });
            let card_border = if active_tile { ACCENT } else { LINE };
            if !buffered && focus_color != 0 {
                let focus_r = ellipse.saturating_sub(s(4)).max(s(4));
                let focus = rect(r.left + s(3), r.top + s(3), r.right - s(3), r.bottom - s(3));
                let pen = CreatePen(PS_SOLID, 1, rgb(focus_color));
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
            return Some(RoundedMask {
                ellipse,
                border: card_border,
                border_width: if active_tile { s(2).max(1) as f32 } else { 1.0 },
                focus: focus_color,
                focus_inset: s(3).max(2) as f32,
                host_color,
                host_alpha,
            });
        }

        let text = window_text(item.hwndItem);
        let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
        let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
        let primary = id == 102;
        let (color, border, highlight) = if disabled {
            (
                if glass_host { PANEL } else { DISABLED_FILL },
                DISABLED_BORDER,
                0,
            )
        } else if is_tab {
            if active_tab {
                (ACCENT, ACCENT_DARK, ACCENT_SOFT)
            } else if pressed {
                (mix(INK, CREAM, 10, 100), LINE, 0)
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
        if buffered {
            fill(dc, r, color);
            if highlight != 0 && r.right - r.left > ellipse {
                let h_pen = CreatePen(PS_SOLID, 1, rgb(highlight));
                let old = SelectObject(dc, h_pen);
                let _ = MoveToEx(dc, r.left + ellipse / 2, r.top + 1, None);
                let _ = LineTo(dc, r.right - ellipse / 2, r.top + 1);
                SelectObject(dc, old);
                let _ = DeleteObject(h_pen);
            }
        } else {
            fill(dc, r, host_color);
            raised_rounded(dc, r, color, border, highlight, ellipse);
        }
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
        if !buffered && focus_color != 0 {
            let focus_r = ellipse.saturating_sub(s(4)).max(s(4));
            let focus = rect(r.left + s(3), r.top + s(3), r.right - s(3), r.bottom - s(3));
            let pen = CreatePen(PS_SOLID, 1, rgb(focus_color));
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
        Some(RoundedMask {
            ellipse,
            border,
            border_width: 1.0,
            focus: focus_color,
            focus_inset: s(3).max(2) as f32,
            host_color,
            host_alpha,
        })
    }
}

pub fn enable_backdrop(window: HWND) -> bool {
    use windows::Win32::{Graphics::Dwm::*, UI::Controls::MARGINS};
    // SAFETY: all attributes and the property belong to this top-level HWND.
    unsafe {
        let dark = 1i32;
        let round = DWMWCP_ROUND.0;
        let acrylic = DWMSBT_TRANSIENTWINDOW.0;
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark as *const i32).cast(),
            std::mem::size_of_val(&dark) as u32,
        );
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&round as *const i32).cast(),
            std::mem::size_of_val(&round) as u32,
        );
        let enabled = PAINT_RUNTIME.with(|runtime| runtime.initialized)
            && DwmIsCompositionEnabled().is_ok_and(|value| value.as_bool())
            && DwmSetWindowAttribute(
                window,
                DWMWA_SYSTEMBACKDROP_TYPE,
                (&acrylic as *const i32).cast(),
                std::mem::size_of_val(&acrylic) as u32,
            )
            .is_ok()
            && DwmExtendFrameIntoClientArea(
                window,
                &MARGINS {
                    cxLeftWidth: -1,
                    cxRightWidth: -1,
                    cyTopHeight: -1,
                    cyBottomHeight: -1,
                },
            )
            .is_ok()
            && SetPropW(
                window,
                GLASS_PROPERTY,
                windows::Win32::Foundation::HANDLE(window.0),
            )
            .is_ok();
        if !enabled {
            disable_backdrop(window);
        }
        enabled
    }
}

fn disable_backdrop(window: HWND) {
    use windows::Win32::{Graphics::Dwm::*, UI::Controls::MARGINS};
    unsafe {
        let was_enabled = glass_enabled(window);
        let _ = RemovePropW(window, GLASS_PROPERTY);
        let none = DWMSBT_NONE.0;
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&none as *const i32).cast(),
            std::mem::size_of_val(&none) as u32,
        );
        let _ = DwmExtendFrameIntoClientArea(window, &MARGINS::default());
        if was_enabled {
            let _ = RedrawWindow(window, None, None, RDW_INVALIDATE | RDW_ALLCHILDREN);
        }
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

pub unsafe fn paint_into(window: HWND, target: HDC) {
    // SAFETY: buffers, theme handle and target DC stay on this synchronous UI-thread call.
    unsafe {
        let mut client = RECT::default();
        if GetClientRect(window, &mut client).is_err()
            || IsIconic(window).as_bool()
            || client.right <= client.left
            || client.bottom <= client.top
        {
            return;
        }
        if let Some(buffer) = PaintBuffer::new(target, client) {
            let glass = glass_enabled(window);
            let theme = if glass {
                OpenThemeData(window, w!("WINDOW"))
            } else {
                HTHEME::default()
            };
            let old_theme = TEXT_THEME.with(|slot| slot.replace(theme));
            let old_buffer = GLASS_BUFFER.with(|slot| slot.replace(buffer.handle));
            let rendered = paint_scene(
                window,
                buffer.dc,
                if glass && theme.0 != 0 {
                    Some(&buffer)
                } else {
                    None
                },
            );
            TEXT_THEME.with(|slot| slot.set(old_theme));
            GLASS_BUFFER.with(|slot| slot.set(old_buffer));
            if theme.0 != 0 {
                let _ = CloseThemeData(theme);
            }
            let ready = rendered && ((glass && theme.0 != 0) || buffer.opaque());
            if ready && buffer.finish() {
                if glass && theme.0 == 0 {
                    disable_backdrop(window);
                }
                return;
            }
        }
        disable_backdrop(window);
        paint_scene(window, target, None);
    }
}

unsafe fn paint_scene(window: HWND, dc: HDC, glass: Option<&PaintBuffer>) -> bool {
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
        fill(dc, pixels(layout.header), BODY);
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
        if let Some(buffer) = glass {
            let card_mask = RoundedMask {
                ellipse: scale(12),
                border: LINE,
                border_width: 1.0,
                focus: 0,
                focus_inset: 0.0,
                host_color: INK,
                host_alpha: PANEL_ALPHA,
            };
            let panel_ok = match tab {
                Tab::Library => buffer.tint_background(pixels(layout.sidebar), 255),
                Tab::Settings => {
                    buffer.apply_rounded_rect_mask(
                        pixels(layout.settings_preferences_panel),
                        &card_mask,
                    ) && buffer.apply_rounded_rect_mask(
                        pixels(layout.settings_resources_panel),
                        &card_mask,
                    )
                }
                Tab::Sources => {
                    buffer.apply_rounded_rect_mask(pixels(layout.sources_panel), &card_mask)
                }
            };
            // Content panels stay opaque; outer header, canvas and footer transmit the acrylic backdrop.
            if !panel_ok || !buffer.tint_background(client, PANEL_ALPHA) {
                return false;
            }
        }
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
        true
    }
}

pub fn prepare_control(window: HWND) -> Result<(), String> {
    // SAFETY: subclass executes on the control's UI thread and owns no Rust pointer.
    unsafe {
        let style = GetWindowLongW(window, GWL_STYLE) as u32;
        let mut class = [0u16; 32];
        let count = GetClassNameW(window, &mut class).max(0) as usize;
        let class_name = String::from_utf16_lossy(&class[..count]);
        if class_name == "Edit" && GetDlgCtrlID(window) as usize == SEARCH {
            // Native editing, selection and caret repaint outside WM_PAINT; composite this child opaque.
            let extended = GetWindowLongPtrW(window, GWL_EXSTYLE);
            SetWindowLongPtrW(window, GWL_EXSTYLE, extended | WS_EX_LAYERED.0 as isize);
            SetLayeredWindowAttributes(window, COLORREF(0), 255, LWA_ALPHA)
                .map_err(|error| error.to_string())?;
        }
        let button = class_name == "Button";
        if button {
            let class_style = GetClassLongPtrW(window, GCL_STYLE) as usize;
            let stripped = class_style & !(CS_PARENTDC.0 as usize);
            if stripped != class_style {
                SetClassLongPtrW(window, GCL_STYLE, stripped as isize);
            }
        }
        let owner_draw = button && style & 0xf == BS_OWNERDRAW as u32;
        if !SetWindowSubclass(window, Some(control_proc), 1, usize::from(owner_draw)).as_bool() {
            return Err("Could not initialize opaque native control painting".into());
        }
    }
    Ok(())
}

pub fn round_control(window: HWND) -> Result<(), String> {
    unsafe {
        let mut class = [0u16; 32];
        let count = GetClassNameW(window, &mut class).max(0) as usize;
        if String::from_utf16_lossy(&class[..count]) != "Button"
            || GetWindowLongW(window, GWL_STYLE) as u32 & 0xf != BS_OWNERDRAW as u32
        {
            return Ok(());
        }
        let mut area = RECT::default();
        GetClientRect(window, &mut area).map_err(|e| e.to_string())?;
        if area.right <= 0 || area.bottom <= 0 {
            return Ok(());
        }
        let id = GetDlgCtrlID(window) as usize;
        let dpi = GetDpiForWindow(window).max(96) as i32;
        let radius = control_corner_ellipse(id, dpi);
        let region = CreateRoundRectRgn(0, 0, area.right + 1, area.bottom + 1, radius, radius);
        if region.0.is_null() {
            return Err("Could not create the control's rounded region".into());
        }
        if SetWindowRgn(window, region, false) == 0 {
            let _ = DeleteObject(region);
            return Err("Could not apply the control's rounded region".into());
        }
    }
    Ok(())
}

fn control_color(id: usize) -> u32 {
    if id == 113 {
        FOOTER
    } else if id == SEARCH {
        CARD
    } else {
        control_surface(id).0
    }
}

unsafe fn paint_control_into(window: HWND, dc: HDC, area: RECT) {
    // SAFETY: DefSubclassProc reaches the original native control, not this subclass again.
    unsafe {
        let id = GetDlgCtrlID(window) as usize;
        let parent = GetParent(window).unwrap_or_default();
        if area.right <= area.left
            || area.bottom <= area.top
            || (!parent.0.is_null() && IsIconic(parent).as_bool())
        {
            return;
        }
        let mut class = [0u16; 32];
        let count = GetClassNameW(window, &mut class).max(0) as usize;
        let static_text = String::from_utf16_lossy(&class[..count]) == "Static";
        let checkbox = matches!(id, 107 | 108 | 121);
        let status_edit = id == 113;
        let mut status_selected = false;
        if status_edit {
            let (mut sel_start, mut sel_end) = (0u32, 0u32);
            SendMessageW(
                window,
                EM_GETSEL,
                WPARAM((&mut sel_start as *mut u32) as usize),
                LPARAM((&mut sel_end as *mut u32) as isize),
            );
            status_selected = sel_start != sel_end;
        }
        let color = control_color(id);
        if let Some(buffer) = PaintBuffer::new(dc, area) {
            if static_text || checkbox || (status_edit && !status_selected) {
                let glass = glass_enabled(parent);
                let (surface_color, surface_translucent) = control_surface(id);
                let theme = if glass {
                    OpenThemeData(parent, w!("WINDOW"))
                } else {
                    HTHEME(0)
                };
                fill(buffer.dc, area, surface_color);
                let alpha_ok = if theme.0 != 0 {
                    buffer.tint_background(
                        area,
                        if surface_translucent {
                            PANEL_ALPHA
                        } else {
                            255
                        },
                    )
                } else {
                    true
                };
                let old_theme = TEXT_THEME.with(|slot| slot.replace(theme));
                let old_buffer = GLASS_BUFFER.with(|slot| slot.replace(buffer.handle));
                let dpi = GetDpiForWindow(window).max(96) as i32;
                let s = |n: i32| n * dpi / 96;
                let mut text_area = area;
                if checkbox {
                    let size = s(15).max(12);
                    let top = (area.top + area.bottom - size) / 2;
                    let glyph = rect(area.left, top, area.left + size, top + size);
                    let state = SendMessageW(window, BM_GETSTATE, WPARAM(0), LPARAM(0)).0;
                    let checked = state & 3 == 1;
                    let pressed = state & 4 != 0;
                    let disabled = GetWindowLongW(window, GWL_STYLE) as u32 & WS_DISABLED.0 != 0;
                    let box_fill = if checked {
                        if pressed { ACCENT_DARK } else { ACCENT }
                    } else if pressed {
                        CARD_LIT
                    } else {
                        INK
                    };
                    let box_border = if checked {
                        if pressed { ACCENT_DEEP } else { ACCENT_DARK }
                    } else if disabled {
                        DISABLED_BORDER
                    } else {
                        HIGHLIGHT
                    };
                    rounded(buffer.dc, glyph, box_fill, box_border, s(5).max(3));
                    if checked {
                        let pen = CreatePen(PS_SOLID, s(2).max(1), rgb(INK));
                        let old_pen = SelectObject(buffer.dc, pen);
                        let x0 = glyph.left + size * 24 / 100;
                        let y0 = glyph.top + size * 52 / 100;
                        let x1 = glyph.left + size * 43 / 100;
                        let y1 = glyph.top + size * 71 / 100;
                        let x2 = glyph.left + size * 77 / 100;
                        let y2 = glyph.top + size * 31 / 100;
                        let _ = MoveToEx(buffer.dc, x0, y0, None);
                        let _ = LineTo(buffer.dc, x1, y1);
                        let _ = LineTo(buffer.dc, x2, y2);
                        SelectObject(buffer.dc, old_pen);
                        let _ = DeleteObject(pen);
                    }
                    let _ = GdiFlush();
                    let _ = BufferedPaintSetAlpha(buffer.handle, Some(&glyph), 255);
                    text_area.left += size + s(9);
                }
                label(
                    buffer.dc,
                    &window_text(window),
                    text_area,
                    s(if id == PAGE { 13 } else { 14 }),
                    false,
                    if status_edit {
                        TEXT_ON_DARK
                    } else if id == 112 || checkbox {
                        TEXT
                    } else {
                        MUTED
                    },
                    if id == PAGE {
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE
                    } else if checkbox {
                        DT_SINGLELINE | DT_VCENTER
                    } else if id == 112 {
                        DT_SINGLELINE | DT_END_ELLIPSIS
                    } else if status_edit {
                        DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX
                    } else {
                        DT_WORDBREAK
                    },
                );
                if checkbox {
                    let state = SendMessageW(window, BM_GETSTATE, WPARAM(0), LPARAM(0)).0;
                    let hide_focus = !parent.0.is_null()
                        && SendMessageW(parent, WM_QUERYUISTATE, WPARAM(0), LPARAM(0)).0
                            & UISF_HIDEFOCUS_FLAG
                            != 0;
                    if state & 8 != 0 && !hide_focus {
                        let focus_r = rect(
                            (text_area.left - s(4)).max(area.left),
                            area.top + s(2),
                            area.right - s(2),
                            area.bottom - s(2),
                        );
                        for edge in [
                            rect(focus_r.left, focus_r.top, focus_r.right, focus_r.top + 1),
                            rect(
                                focus_r.left,
                                focus_r.bottom - 1,
                                focus_r.right,
                                focus_r.bottom,
                            ),
                            rect(focus_r.left, focus_r.top, focus_r.left + 1, focus_r.bottom),
                            rect(
                                focus_r.right - 1,
                                focus_r.top,
                                focus_r.right,
                                focus_r.bottom,
                            ),
                        ] {
                            fill(buffer.dc, edge, ACCENT_SOFT);
                            let _ = GdiFlush();
                            let _ = BufferedPaintSetAlpha(buffer.handle, Some(&edge), 255);
                        }
                    }
                }
                TEXT_THEME.with(|slot| slot.set(old_theme));
                GLASS_BUFFER.with(|slot| slot.set(old_buffer));
                if theme.0 != 0 {
                    let _ = CloseThemeData(theme);
                    if alpha_ok && buffer.finish() {
                        return;
                    }
                    disable_backdrop(parent);
                } else if buffer.opaque() && buffer.finish() {
                    return;
                }
            } else {
                fill(buffer.dc, area, color);
                DefSubclassProc(
                    window,
                    WM_PRINTCLIENT,
                    WPARAM(buffer.dc.0 as usize),
                    LPARAM(PRF_CLIENT as isize),
                );
                if buffer.opaque() && buffer.finish() {
                    return;
                }
            }
        }
        if !parent.0.is_null() {
            disable_backdrop(parent);
        }
        fill(dc, area, color);
        if static_text || checkbox {
            let dpi = GetDpiForWindow(window).max(96) as i32;
            label(
                dc,
                &window_text(window),
                area,
                if id == PAGE { 13 } else { 14 } * dpi / 96,
                false,
                if id == 112 || checkbox { TEXT } else { MUTED },
                if id == PAGE {
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE
                } else if checkbox {
                    DT_SINGLELINE | DT_VCENTER
                } else if id == 112 {
                    DT_SINGLELINE | DT_END_ELLIPSIS
                } else {
                    DT_WORDBREAK
                },
            );
            return;
        }
        DefSubclassProc(
            window,
            WM_PRINTCLIENT,
            WPARAM(dc.0 as usize),
            LPARAM(PRF_CLIENT as isize),
        );
    }
}

unsafe extern "system" fn control_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        VK_DOWN, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP,
    };
    unsafe {
        let owner_draw = data != 0;
        match message {
            WM_PAINT if !owner_draw => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(window, &mut ps);
                let mut area = RECT::default();
                if GetClientRect(window, &mut area).is_ok() {
                    paint_control_into(window, dc, area);
                }
                let _ = EndPaint(window, &ps);
                LRESULT(0)
            }
            WM_PRINTCLIENT if !owner_draw && wparam.0 != 0 => {
                let mut area = RECT::default();
                if GetClientRect(window, &mut area).is_ok() {
                    paint_control_into(window, HDC(wparam.0 as *mut _), area);
                }
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
                if let Ok(parent) = GetParent(window)
                    && !parent.0.is_null()
                {
                    let _ = SendMessageW(
                        parent,
                        WM_CHANGEUISTATE,
                        WPARAM(UIS_SET as usize | ((UISF_HIDEFOCUS as usize) << 16)),
                        LPARAM(0),
                    );
                }
                let res = DefSubclassProc(window, message, wparam, lparam);
                if GetDlgCtrlID(window) as usize == 113 {
                    let _ = RedrawWindow(window, None, None, RDW_INVALIDATE | RDW_UPDATENOW);
                }
                res
            }
            WM_LBUTTONUP | WM_SETFOCUS | WM_KILLFOCUS | WM_KEYUP => {
                let res = DefSubclassProc(window, message, wparam, lparam);
                if GetDlgCtrlID(window) as usize == 113 {
                    if message == WM_SETFOCUS {
                        let _ = HideCaret(window);
                    }
                    let _ = RedrawWindow(window, None, None, RDW_INVALIDATE | RDW_UPDATENOW);
                }
                res
            }
            WM_MOUSEMOVE if wparam.0 & 1 != 0 && GetDlgCtrlID(window) as usize == 113 => {
                let res = DefSubclassProc(window, message, wparam, lparam);
                let _ = RedrawWindow(window, None, None, RDW_INVALIDATE | RDW_UPDATENOW);
                res
            }
            WM_KEYDOWN => {
                let key = wparam.0 as u16;
                if matches!(
                    key,
                    x if x == VK_TAB.0
                        || x == VK_LEFT.0
                        || x == VK_RIGHT.0
                        || x == VK_UP.0
                        || x == VK_DOWN.0
                        || x == VK_SPACE.0
                        || x == VK_RETURN.0
                ) && let Ok(parent) = GetParent(window)
                    && !parent.0.is_null()
                {
                    let _ = SendMessageW(
                        parent,
                        WM_CHANGEUISTATE,
                        WPARAM(UIS_CLEAR as usize | ((UISF_HIDEFOCUS as usize) << 16)),
                        LPARAM(0),
                    );
                }
                let res = DefSubclassProc(window, message, wparam, lparam);
                if GetDlgCtrlID(window) as usize == 113 {
                    let _ = RedrawWindow(window, None, None, RDW_INVALIDATE | RDW_UPDATENOW);
                }
                res
            }
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(window, Some(control_proc), id);
                DefSubclassProc(window, message, wparam, lparam)
            }
            _ => DefSubclassProc(window, message, wparam, lparam),
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
        let color = control_color(id);
        SetBkColor(dc, rgb(color));
        Some(LRESULT(brush(color).0 as isize))
    }
}
thread_local! {
    static BRUSHES: RefCell<HashMap<u32, HBRUSH>> = RefCell::new(HashMap::new());
    static FONTS: RefCell<HashMap<(i32, bool), HFONT>> = RefCell::new(HashMap::new());
}
pub fn brush(color: u32) -> HBRUSH {
    BRUSHES.with(|b| {
        *b.borrow_mut()
            .entry(color)
            .or_insert_with(|| unsafe { CreateSolidBrush(rgb(color)) })
    })
}
fn cached_font(size: i32, bold: bool) -> HFONT {
    FONTS.with(|f| {
        *f.borrow_mut()
            .entry((size, bold))
            .or_insert_with(|| unsafe {
                CreateFontW(
                    -size,
                    0,
                    0,
                    0,
                    if bold { 600 } else { 400 },
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET.0 as u32,
                    OUT_DEFAULT_PRECIS.0 as u32,
                    CLIP_DEFAULT_PRECIS.0 as u32,
                    CLEARTYPE_QUALITY.0 as u32,
                    0,
                    w!("Segoe UI"),
                )
            })
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
        assert_eq!((BODY, CANVAS, FOOTER), (INK, INK, INK));
        assert_eq!((TEXT, TEXT_ON_DARK, TEXT_ON_ACCENT), (CREAM, CREAM, INK));
        assert_eq!(ACCENT, ORANGE);
        fn luminance(color: u32) -> f64 {
            let channel = |shift: u32| {
                let value = f64::from((color >> shift) & 255u32) / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
        }
        for (foreground, background) in [
            (TEXT, INK),
            (TEXT, CARD),
            (MUTED, CARD),
            (INK, ORANGE),
            (TEXT, ACCENT_DARK),
            (MUTED, 0x3c3c3b),
        ] {
            let a = luminance(foreground);
            let b = luminance(background);
            assert!(
                (a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5,
                "text contrast below AA"
            );
        }
    }

    #[test]
    fn rounded_controls_cover_native_erase_and_keep_opaque_alpha() {
        // SAFETY: test-owned memory DC and bitmap never attach to the desktop.
        unsafe {
            let dc = CreateCompatibleDC(None);
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(
                dc,
                &BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: 120,
                        biHeight: -36,
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
            .expect("control DIB");
            let old = SelectObject(dc, bitmap);
            std::slice::from_raw_parts_mut(bits as *mut u32, 120 * 36).fill(0xffff_ffff);
            draw_item(&DRAWITEMSTRUCT {
                CtlID: TILE_BASE as u32,
                hDC: dc,
                rcItem: rect(0, 0, 120, 36),
                ..Default::default()
            });
            let _ = GdiFlush();
            let pixels = std::slice::from_raw_parts(bits as *const u32, 120 * 36);
            let corner = pixels[0];
            let center = pixels[18 * 120 + 100];
            SelectObject(dc, old);
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(dc);
            assert_eq!(
                corner & 0x00ff_ffff,
                INK,
                "canvas tile corners must match Ink backdrop without residual boxes"
            );
            assert_eq!(
                center,
                0xff00_0000 | CARD,
                "foreground controls must reach DWM with alpha 255"
            );
        }
    }

    #[test]
    fn host_surface_and_focus_states_match_controls() {
        assert_eq!(control_color(PAGE), INK);
        assert_eq!(control_color(SEARCH), CARD);
        assert_eq!(control_color(113), FOOTER);
        assert_eq!(control_surface(TAB_LIBRARY), (INK, true));
        assert_eq!(control_surface(PREVIOUS), (INK, true));
        assert_eq!(control_surface(PAGE), (INK, true));
        assert_eq!(control_surface(113), (FOOTER, true));
        assert_eq!(control_surface(PRODUCT_WEBSITE), (CARD, false));
        assert_eq!(control_surface(VERSION), (CARD, false));

        // SAFETY: test-owned memory DC and DIB never attach to any desktop window.
        unsafe {
            let draw_pixels = |id: usize, state: u32| -> Vec<u32> {
                let dc = CreateCompatibleDC(None);
                let mut bits = std::ptr::null_mut();
                let bitmap = CreateDIBSection(
                    dc,
                    &BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: 120,
                            biHeight: -36,
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
                .expect("control test DIB");
                let old = SelectObject(dc, bitmap);
                std::slice::from_raw_parts_mut(bits as *mut u32, 120 * 36).fill(0xffff_ffff);
                let mut item = DRAWITEMSTRUCT {
                    CtlID: id as u32,
                    hDC: dc,
                    rcItem: rect(0, 0, 120, 36),
                    ..Default::default()
                };
                item.itemState.0 = state;
                draw_item(&item);
                let _ = GdiFlush();
                let result = std::slice::from_raw_parts(bits as *const u32, 120 * 36).to_vec();
                SelectObject(dc, old);
                let _ = DeleteObject(bitmap);
                let _ = DeleteDC(dc);
                result
            };

            let tab_pixels = draw_pixels(TAB_LIBRARY, 0);
            assert_eq!(tab_pixels[0] & 0x00ff_ffff, INK);

            let panel_pixels = draw_pixels(PRODUCT_WEBSITE, 0);
            assert_eq!(panel_pixels[0] & 0x00ff_ffff, CARD);

            let disabled_prev = draw_pixels(PREVIOUS, ODS_DISABLED.0);
            assert_eq!(disabled_prev[0] & 0x00ff_ffff, INK);
            assert_eq!(disabled_prev[18 * 120 + 60] & 0x00ff_ffff, PANEL);

            let disabled_card_btn = draw_pixels(REMOVE, ODS_DISABLED.0);
            assert_eq!(disabled_card_btn[0] & 0x00ff_ffff, CARD);
            assert_eq!(
                disabled_card_btn[18 * 120 + 60] & 0x00ff_ffff,
                DISABLED_FILL
            );

            let focused_key = draw_pixels(NEXT, ODS_FOCUS.0);
            let focused_mouse = draw_pixels(NEXT, ODS_FOCUS.0 | ODS_NOFOCUSRECT_FLAG);
            let key_focus_count = focused_key
                .iter()
                .filter(|px| **px & 0x00ff_ffff == ACCENT_SOFT)
                .count();
            let mouse_focus_count = focused_mouse
                .iter()
                .filter(|px| **px & 0x00ff_ffff == ACCENT_SOFT)
                .count();
            assert!(
                key_focus_count > 20,
                "keyboard focus ring must remain visible"
            );
            assert_eq!(
                mouse_focus_count, 0,
                "ODS_NOFOCUSRECT must suppress focus outline on click"
            );
        }
    }

    #[test]
    fn rounded_panel_mask_keeps_card_opaque_and_corners_translucent() {
        // SAFETY: test-owned memory DC and DIB never attach to any desktop window.
        unsafe {
            let dc = CreateCompatibleDC(None);
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(
                dc,
                &BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: 80,
                        biHeight: -80,
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
            .expect("panel mask test DIB");
            let old = SelectObject(dc, bitmap);
            std::slice::from_raw_parts_mut(bits as *mut u32, 80 * 80).fill(0);
            let full = rect(0, 0, 80, 80);
            let panel = rect(10, 10, 70, 70);
            let buffer = PaintBuffer::new(dc, full).expect("buffered paint");
            fill(buffer.dc, full, BODY);
            raised_rounded(buffer.dc, panel, CARD, LINE, HIGHLIGHT, 12);
            assert!(buffer.apply_rounded_rect_mask(
                panel,
                &RoundedMask {
                    ellipse: 12,
                    border: LINE,
                    border_width: 1.0,
                    focus: 0,
                    focus_inset: 0.0,
                    host_color: INK,
                    host_alpha: PANEL_ALPHA,
                },
            ));
            assert!(buffer.tint_background(full, PANEL_ALPHA));
            assert!(buffer.finish());
            let _ = GdiFlush();
            let pixels = std::slice::from_raw_parts(bits as *const u8, 80 * 80 * 4);
            let px = |x: usize, y: usize| -> [u8; 4] {
                let i = (y * 80 + x) * 4;
                pixels[i..i + 4].try_into().expect("BGRA pixel")
            };
            let bg = u8::try_from(17u16 * u16::from(PANEL_ALPHA) / 255).unwrap();
            assert_eq!(px(4, 4), [bg, bg, bg, PANEL_ALPHA]);
            assert_eq!(
                px(10, 10),
                [bg, bg, bg, PANEL_ALPHA],
                "rounded panel corner outside arc must match acrylic canvas"
            );
            let center = px(40, 40);
            assert_eq!(center[3], 255, "panel interior must remain opaque");
            assert_eq!(
                u32::from(center[2]) << 16 | u32::from(center[1]) << 8 | u32::from(center[0]),
                CARD
            );
            SelectObject(dc, old);
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(dc);
        }
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
