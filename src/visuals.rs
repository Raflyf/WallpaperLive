use crate::core::Rect;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::{Path, PathBuf},
    time::Instant,
};
use windows::{
    Win32::{
        Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::*,
        System::{
            LibraryLoader::{GetModuleHandleW, GetProcAddress},
            Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
        },
        UI::{
            Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
            Controls::{
                BP_PAINTPARAMS, BPBF_TOPDOWNDIB, BPPF_ERASE, BeginBufferedPaint, BufferedPaintInit,
                BufferedPaintSetAlpha, BufferedPaintUnInit, CloseThemeData, DRAWITEMSTRUCT,
                DTT_COMPOSITED, DTT_TEXTCOLOR, DTTOPTS, DrawThemeTextEx, EM_GETSEL,
                EndBufferedPaint, GetBufferedPaintBits, HTHEME, ODS_DISABLED, ODS_FOCUS,
                ODS_SELECTED, OpenThemeData,
            },
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent},
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    },
    core::{s, w},
};

#[link(name = "winmm")]
unsafe extern "system" {
    fn timeBeginPeriod(uPeriod: u32) -> u32;
    fn timeEndPeriod(uPeriod: u32) -> u32;
}

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
pub const ACRYLIC: usize = 149;
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
        107 | 108 | 121 | ACRYLIC | VERSION | MEMORY_INFO | CACHE_INFO | CLEAR_CACHE
        | PRODUCT_WEBSITE | SETTINGS_NOTE | SETTINGS_INFO | SUPPORT | SAWERIA => {
            tab == Tab::Settings
        }
        PREVIEW | 112 | 105 | 106 | FPS | AUDIO | REMOVE | 102 | 103 | 104 | SEARCH | SORT
        | FOLDER_FILTER | SELECT_ALL | FIND_WALLPAPERS | PREVIOUS | NEXT | PAGE => {
            tab == Tab::Library
        }
        id if (TILE_BASE..TILE_BASE + 9).contains(&id) => tab == Tab::Library,
        _ => false,
    }
}

pub fn is_body_control(id: usize) -> bool {
    !matches!(
        id,
        TAB_LIBRARY | TAB_SETTINGS | 101 | ADD_FOLDER | 110 | 113
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionButton {
    Minimize,
    Maximize,
    Close,
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
    pub sidebar_divider: Option<Rect>,
    pub settings_dividers: [Option<Rect>; 2],
    pub sources_divider: Option<Rect>,
    pub caption_divider: Rect,
    pub caption_buttons: [(CaptionButton, Rect); 3],
    pub tile_rects: [Rect; 9],
    controls: [(usize, Rect); 43],
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
        let fps_bottom = fit.bottom + 46;
        let content_width = (width - 48).min(1040);
        let content_x = (width - content_width) / 2;
        let column_width = (content_width - 32) / 2;
        let resources_x = content_x + column_width + 32;
        let settings_note = r(content_x, 212, column_width, 40);
        let source_width = (width - 48).min(840);
        let source_x = (width - source_width) / 2;
        let settings_preferences_panel =
            r(content_x - 12, 172, column_width + 24, footer.top - 188);
        let settings_resources_panel =
            r(resources_x - 12, 172, column_width + 24, footer.top - 188);
        let sources_panel = r(source_x - 12, 168, source_width + 24, footer.top - 188);
        let sidebar_divider = (audio_y - fps_bottom >= 8)
            .then(|| r(side_x, (fps_bottom + audio_y) / 2, side_width, 1));
        let settings_dividers = [
            (settings_preferences_panel.bottom > 442).then(|| r(content_x, 438, column_width, 1)),
            (settings_resources_panel.bottom > 442).then(|| r(resources_x, 438, column_width, 1)),
        ];
        let sources_divider =
            (sources_panel.bottom > 408).then(|| r(source_x, 404, source_width, 1));
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
            settings_preferences_panel,
            settings_resources_panel,
            sources_panel,
            sources_title: r(source_x, 96, source_width - 196, 32),
            sources_subtitle: r(source_x, 136, source_width, 24),
            sidebar_divider,
            settings_dividers,
            sources_divider,
            caption_divider: r(width - 146, 22, 1, 28),
            caption_buttons: [
                (CaptionButton::Minimize, r(width - 134, 18, 36, 36)),
                (CaptionButton::Maximize, r(width - 94, 18, 36, 36)),
                (CaptionButton::Close, r(width - 54, 18, 36, 36)),
            ],
            tile_rects,
            controls: [
                (101, r(width - 406, 18, 118, 36)),
                (ADD_FOLDER, r(width - 280, 18, 122, 36)),
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
                (107, r(content_x, 260, column_width, 38)),
                (121, r(content_x, 304, column_width, 38)),
                (108, r(content_x, 348, column_width, 38)),
                (ACRYLIC, r(content_x, 392, column_width, 38)),
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

/// Round-to-nearest premultiplication matching float SDF `.round()`.
const fn premul_channel(channel: u8, alpha: u8) -> u8 {
    ((channel as u16 * alpha as u16 + 127) / 255) as u8
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
pub const MUTED: u32 = mix(CREAM, INK, 6, 100);
pub const MUTED_ON_DARK: u32 = MUTED;
pub const ACCENT: u32 = ORANGE;
pub const BODY: u32 = INK;
pub const CANVAS: u32 = INK;
pub const FOOTER: u32 = INK;
pub const PANEL: u32 = mix(INK, CREAM, 8, 100);
pub const CARD: u32 = mix(INK, CREAM, 14, 100);
pub const CARD_LIT: u32 = mix(INK, CREAM, 23, 100);
pub const LINE: u32 = mix(INK, CREAM, 42, 100);
pub const HIGHLIGHT: u32 = mix(INK, CREAM, 60, 100);
pub const DISABLED_TEXT: u32 = mix(CREAM, INK, 26, 100);
const DISABLED_FILL: u32 = mix(INK, CREAM, 10, 100);
const DISABLED_BORDER: u32 = mix(INK, CREAM, 26, 100);
pub const ACTIVE_SURFACE: u32 = mix(INK, ORANGE, 26, 100);
pub const ACCENT_DARK: u32 = mix(ORANGE, INK, 22, 100);
pub const ACCENT_DEEP: u32 = mix(ORANGE, INK, 38, 100);
pub const ACCENT_SOFT: u32 = mix(ORANGE, CREAM, 16, 100);
pub const ACCENT_HOVER: u32 = mix(ORANGE, CREAM, 12, 100);
pub const GLASS_RIM: u32 = mix(INK, CREAM, 74, 100);
pub const PANEL_ALPHA: u8 = 0;
pub const CARD_ALPHA: u8 = 96;
const CONTROL_ALPHA: u8 = 138;
const HOVER_ALPHA: u8 = 184;
const ACTIVE_TILE_ALPHA: u8 = 196;
const ACTIVE_ALPHA: u8 = 244;
pub const BORDER_ALPHA: u8 = 220;
const DIVIDER_ALPHA: u8 = 176;
const SEARCH_ALPHA: u8 = 208;
pub const DROPDOWN_ALPHA: u8 = 156;
// Lighter smoked Ink Acrylic tint so wallpaper colors & blur shine clearly both focused and unfocused.
const ACRYLIC_TINT_COLOR: u32 = 0x2211_1111;
const ODS_NOFOCUSRECT_FLAG: u32 = 0x0200;
const UISF_HIDEFOCUS_FLAG: isize = 0x0001;

pub(crate) fn ease_out_cubic(t: f32) -> f32 {
    let clamped = t.clamp(0.0, 1.0);
    1.0 - (1.0 - clamped).powi(3)
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let t = t.clamp(0.0, 1.0);
    (f32::from(a) + (f32::from(b) - f32::from(a)) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn lerp_rgb(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let mix_c = |shift: u32| -> u32 {
        let ca = ((a >> shift) & 255) as f32;
        let cb = ((b >> shift) & 255) as f32;
        ((ca + (cb - ca) * t).round().clamp(0.0, 255.0) as u32) << shift
    };
    mix_c(16) | mix_c(8) | mix_c(0)
}

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

fn control_surface(id: usize) -> (u32, u8) {
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
            | ACRYLIC
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
        (CARD, CARD_ALPHA)
    } else {
        (CANVAS, PANEL_ALPHA)
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
pub const MAX_CACHED_THUMBNAILS: usize = 162;

struct ThumbEntry {
    bitmap: Bitmap,
    stamp: u64,
}

struct Skin {
    selected: Option<PathBuf>,
    marked: Vec<PathBuf>,
    playing: Vec<PathBuf>,
    library_empty: bool,
    tiles: HashMap<usize, PathBuf>,
    thumbnails: HashMap<PathBuf, ThumbEntry>,
    thumb_seq: u64,
    tab: Tab,
}
thread_local! {static SKIN:RefCell<Skin>=RefCell::new(Skin{selected:None,marked:Vec::new(),playing:Vec::new(),library_empty:true,tiles:HashMap::new(),thumbnails:HashMap::new(),thumb_seq:0,tab:Tab::Library});}
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
    SKIN.with(|s| {
        let skin = &mut *s.borrow_mut();
        if let Some(ref p) = path {
            let next = skin.thumb_seq.wrapping_add(1);
            if let Some(entry) = skin.thumbnails.get_mut(p) {
                skin.thumb_seq = next;
                entry.stamp = next;
            }
        }
        skin.selected = path;
    });
}
pub fn tiles(paths: &[PathBuf]) {
    SKIN.with(|s| {
        let skin = &mut *s.borrow_mut();
        skin.tiles = paths
            .iter()
            .enumerate()
            .map(|(i, p)| (TILE_BASE + i, p.clone()))
            .collect();
        for p in paths {
            let next = skin.thumb_seq.wrapping_add(1);
            if let Some(entry) = skin.thumbnails.get_mut(p) {
                skin.thumb_seq = next;
                entry.stamp = next;
            }
        }
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
        if skin.thumbnails.len() >= MAX_CACHED_THUMBNAILS && !skin.thumbnails.contains_key(&path) {
            let victim = skin
                .thumbnails
                .iter()
                .filter(|(p, _)| {
                    Some(*p) != skin.selected.as_ref()
                        && !skin.tiles.values().any(|tile| tile == *p)
                })
                .min_by_key(|(_, entry)| entry.stamp)
                .map(|(p, _)| p)
                .or_else(|| {
                    skin.thumbnails
                        .iter()
                        .min_by_key(|(_, entry)| entry.stamp)
                        .map(|(p, _)| p)
                })
                .cloned();
            if let Some(victim) = victim {
                skin.thumbnails.remove(&victim);
            }
        }
        skin.thumb_seq = skin.thumb_seq.wrapping_add(1);
        let stamp = skin.thumb_seq;
        skin.thumbnails.insert(
            path,
            ThumbEntry {
                bitmap: image,
                stamp,
            },
        );
    });
}
pub const ANIM_TIMER_ID: usize = 3;
const TAB_ANIM_MS: u32 = 210;
const TOGGLE_ANIM_MS: u32 = 150;
pub const PAGE_ANIM_MS: u32 = 250;
pub const TAB_TRANSITION_MS: u32 = 260;

struct ToggleAnim {
    from: f32,
    pos: f32,
    target: bool,
    start: Instant,
    settled: bool,
}
struct TabSlideAnim {
    from_settings: f32,
    to_settings: f32,
    start: Instant,
}
pub struct CapturedLayer {
    pixels: Vec<u32>,
    width: i32,
    height: i32,
}
struct TransitionAnim {
    clip: RECT,
    direction: i32,
    start: Instant,
    duration_ms: u32,
    old_layer: CapturedLayer,
    new_layer: CapturedLayer,
    hidden: Vec<HWND>,
    restore_focus: Option<HWND>,
    covers_body: bool,
}
#[derive(Default)]
struct AnimState {
    hot: isize,
    caption_hot: Option<CaptionButton>,
    caption_down: Option<CaptionButton>,
    toggles: HashMap<usize, ToggleAnim>,
    tab_slide: Option<TabSlideAnim>,
    transition: Option<TransitionAnim>,
    timer_period_active: bool,
}
impl AnimState {
    fn has_active(&self) -> bool {
        self.tab_slide.is_some()
            || self.transition.is_some()
            || self.toggles.values().any(|toggle| !toggle.settled)
    }
}
thread_local! {
    static ANIM: RefCell<AnimState> = RefCell::new(AnimState::default());
}

pub fn animations_enabled(window: HWND) -> bool {
    !window.0.is_null()
        && unsafe { IsWindowVisible(window).as_bool() && !IsIconic(window).as_bool() }
}

pub fn has_transition() -> bool {
    ANIM.with(|a| a.borrow().transition.is_some())
}

fn is_hot(window: HWND) -> bool {
    !window.0.is_null() && ANIM.with(|a| a.borrow().hot == window.0 as isize)
}

fn anim_interval_ms() -> u32 {
    use windows::Win32::Graphics::Dwm::{DWM_TIMING_INFO, DwmGetCompositionTimingInfo};
    let mut info = DWM_TIMING_INFO {
        cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32,
        ..Default::default()
    };
    // Match the DWM composition refresh rate so transitions advance once per vsync instead of beating at 10ms vs 16.6ms.
    // SAFETY: sized DWM_TIMING_INFO out-parameter with null window queries the primary display cadence.
    if unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut info) }.is_ok()
        && info.rateRefresh.uiNumerator > 0
        && info.rateRefresh.uiDenominator > 0
    {
        let ms = (info.rateRefresh.uiDenominator as f64 * 1000.0
            / info.rateRefresh.uiNumerator as f64)
            .floor() as u32;
        return ms.clamp(6, 16);
    }
    16
}

fn arm_anim_timer(window: HWND) {
    if !window.0.is_null() {
        ANIM.with(|a| {
            let mut state = a.borrow_mut();
            if !state.timer_period_active {
                unsafe {
                    if timeBeginPeriod(1) == 0 {
                        state.timer_period_active = true;
                    }
                }
            }
        });
        let interval = anim_interval_ms();
        unsafe {
            let _ = SetTimer(window, ANIM_TIMER_ID, interval, None);
        }
    }
}

pub fn caption_cluster_rect(window: HWND) -> Option<RECT> {
    if window.0.is_null() {
        return None;
    }
    let mut client = RECT::default();
    // SAFETY: read-only geometry query on live window.
    if unsafe { GetClientRect(window, &mut client) }.is_err() || client.right <= 0 {
        return None;
    }
    let dpi = unsafe { GetDpiForWindow(window) }.max(96) as i32;
    let layout = Layout::new(client.right * 96 / dpi, client.bottom.max(1) * 96 / dpi);
    let first = layout.caption_buttons[0].1;
    let last = layout.caption_buttons[2].1;
    Some(rect(
        first.left * dpi / 96 - 2,
        first.top * dpi / 96 - 2,
        last.right * dpi / 96 + 2,
        last.bottom * dpi / 96 + 2,
    ))
}

pub fn caption_button_hit(window: HWND, x: i32, y: i32) -> Option<CaptionButton> {
    if window.0.is_null() {
        return None;
    }
    let mut client = RECT::default();
    // SAFETY: read-only geometry query on live window.
    if unsafe { GetClientRect(window, &mut client) }.is_err()
        || client.right <= 0
        || client.bottom <= 0
    {
        return None;
    }
    let dpi = unsafe { GetDpiForWindow(window) }.max(96) as i32;
    let layout = Layout::new(client.right * 96 / dpi, client.bottom * 96 / dpi);
    layout.caption_buttons.iter().find_map(|(btn, r)| {
        let left = r.left * dpi / 96;
        let top = r.top * dpi / 96;
        let right = r.right * dpi / 96;
        let bottom = r.bottom * dpi / 96;
        (x >= left && x < right && y >= top && y < bottom).then_some(*btn)
    })
}

pub fn set_caption_hover(window: HWND, hot: Option<CaptionButton>) -> bool {
    let changed = ANIM.with(|a| {
        let mut state = a.borrow_mut();
        if state.caption_hot == hot {
            false
        } else {
            state.caption_hot = hot;
            true
        }
    });
    if changed && let Some(area) = caption_cluster_rect(window) {
        // SAFETY: invalidates only the caption button cluster for immediate redraw.
        unsafe {
            let _ = RedrawWindow(window, Some(&area), None, RDW_INVALIDATE | RDW_UPDATENOW);
        }
    }
    changed
}

pub fn set_caption_down(window: HWND, down: Option<CaptionButton>) {
    let changed = ANIM.with(|a| {
        let mut state = a.borrow_mut();
        if state.caption_down == down {
            false
        } else {
            state.caption_down = down;
            true
        }
    });
    if changed && let Some(area) = caption_cluster_rect(window) {
        // SAFETY: invalidates only the caption button cluster for immediate press feedback.
        unsafe {
            let _ = RedrawWindow(window, Some(&area), None, RDW_INVALIDATE | RDW_UPDATENOW);
        }
    }
}

pub fn caption_down() -> Option<CaptionButton> {
    ANIM.with(|a| a.borrow().caption_down)
}

fn disarm_timer_if_idle(window: HWND) {
    let active = ANIM.with(|a| a.borrow().has_active());
    if !active {
        ANIM.with(|a| {
            let mut state = a.borrow_mut();
            if state.timer_period_active {
                unsafe {
                    let _ = timeEndPeriod(1);
                }
                state.timer_period_active = false;
            }
        });
        if !window.0.is_null() {
            unsafe {
                let _ = KillTimer(window, ANIM_TIMER_ID);
            }
        }
    }
}

fn tab_settings_weight(tab: Tab) -> f32 {
    ANIM.with(|a| {
        let state = a.borrow();
        if let Some(slide) = &state.tab_slide {
            let raw = slide.start.elapsed().as_secs_f32() * 1000.0 / TAB_ANIM_MS as f32;
            let t = ease_out_cubic(raw);
            slide.from_settings + (slide.to_settings - slide.from_settings) * t
        } else if tab == Tab::Settings {
            1.0
        } else {
            0.0
        }
    })
}

fn toggle_progress(id: usize, checked: bool, parent: HWND) -> f32 {
    let mut should_arm = false;
    let pos = ANIM.with(|a| {
        let mut state = a.borrow_mut();
        let now = Instant::now();
        let target_f = if checked { 1.0 } else { 0.0 };
        match state.toggles.get_mut(&id) {
            None => {
                state.toggles.insert(
                    id,
                    ToggleAnim {
                        from: target_f,
                        pos: target_f,
                        target: checked,
                        start: now,
                        settled: true,
                    },
                );
                target_f
            }
            Some(entry) => {
                let current_target = if entry.target { 1.0 } else { 0.0 };
                let raw =
                    now.duration_since(entry.start).as_secs_f32() * 1000.0 / TOGGLE_ANIM_MS as f32;
                entry.pos = if entry.settled || raw >= 1.0 {
                    entry.settled = true;
                    current_target
                } else {
                    entry.from + (current_target - entry.from) * ease_out_cubic(raw)
                };
                if entry.target != checked {
                    entry.from = entry.pos;
                    entry.target = checked;
                    entry.start = now;
                    if animations_enabled(parent) {
                        entry.settled = false;
                        should_arm = true;
                    } else {
                        entry.pos = target_f;
                        entry.from = target_f;
                        entry.settled = true;
                    }
                }
                entry.pos
            }
        }
    });
    if should_arm {
        arm_anim_timer(parent);
    }
    pos
}

pub fn start_tab_pill_anim(window: HWND, from_tab: Tab, to_tab: Tab) {
    let from_is_settings = from_tab == Tab::Settings;
    let to_is_settings = to_tab == Tab::Settings;
    if from_is_settings == to_is_settings || !animations_enabled(window) {
        ANIM.with(|a| a.borrow_mut().tab_slide = None);
        return;
    }
    let current = tab_settings_weight(from_tab);
    ANIM.with(|a| {
        a.borrow_mut().tab_slide = Some(TabSlideAnim {
            from_settings: current,
            to_settings: if to_is_settings { 1.0 } else { 0.0 },
            start: Instant::now(),
        });
    });
    arm_anim_timer(window);
}

unsafe fn set_child_visible_no_redraw(child: HWND, visible: bool) {
    if child.0.is_null() {
        return;
    }
    let flag = if visible {
        SWP_SHOWWINDOW
    } else {
        SWP_HIDEWINDOW
    };
    unsafe {
        if !visible || IsWindow(child).as_bool() {
            let _ = SetWindowPos(
                child,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW | flag,
            );
        }
    }
}

#[cfg(test)]
pub unsafe fn paint_children_into(
    parent: HWND,
    dc: HDC,
    controls: &[(usize, HWND)],
    filter_clip: Option<RECT>,
    dib_bits: Option<(*mut u32, i32, i32)>,
) {
    // SAFETY: parent and child HWNDs live on this synchronous UI thread; DIB pointer bounds validated.
    unsafe {
        paint_children_with_visibility(parent, dc, controls, filter_clip, dib_bits, true);
    }
}

unsafe fn paint_children_with_visibility(
    parent: HWND,
    dc: HDC,
    controls: &[(usize, HWND)],
    filter_clip: Option<RECT>,
    dib_bits: Option<(*mut u32, i32, i32)>,
    require_visible: bool,
) {
    // SAFETY: parent and child HWNDs live on this synchronous UI thread; DIB pointer bounds validated.
    unsafe {
        for &(id, child) in controls {
            if child.0.is_null()
                || (require_visible && GetWindowLongW(child, GWL_STYLE) as u32 & WS_VISIBLE.0 == 0)
            {
                continue;
            }
            let mut r = RECT::default();
            if GetWindowRect(child, &mut r).is_err() {
                continue;
            }
            let mut pt = [
                POINT {
                    x: r.left,
                    y: r.top,
                },
                POINT {
                    x: r.right,
                    y: r.bottom,
                },
            ];
            MapWindowPoints(None, parent, &mut pt);
            let child_rect = rect(pt[0].x, pt[0].y, pt[1].x, pt[1].y);
            if let Some(clip) = filter_clip
                && (child_rect.right <= clip.left
                    || child_rect.left >= clip.right
                    || child_rect.bottom <= clip.top
                    || child_rect.top >= clip.bottom)
            {
                continue;
            }
            if id == SEARCH {
                let cw = child_rect.right - child_rect.left;
                let ch = child_rect.bottom - child_rect.top;
                if cw <= 0 || ch <= 0 {
                    continue;
                }
                let temp_dc = CreateCompatibleDC(dc);
                if !temp_dc.0.is_null() {
                    let mut temp_bits = std::ptr::null_mut();
                    if let Ok(temp_bmp) = CreateDIBSection(
                        temp_dc,
                        &BITMAPINFO {
                            bmiHeader: BITMAPINFOHEADER {
                                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                                biWidth: cw,
                                biHeight: -ch,
                                biPlanes: 1,
                                biBitCount: 32,
                                biCompression: BI_RGB.0,
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        DIB_RGB_COLORS,
                        &mut temp_bits,
                        None,
                        0,
                    ) {
                        let prev_bmp = SelectObject(temp_dc, temp_bmp);
                        if !temp_bits.is_null() {
                            let total = (cw * ch) as usize;
                            std::slice::from_raw_parts_mut(temp_bits as *mut u32, total).fill(CARD);
                            SendMessageW(
                                child,
                                WM_PRINT,
                                WPARAM(temp_dc.0 as usize),
                                LPARAM((PRF_CLIENT | PRF_NONCLIENT | PRF_ERASEBKGND) as isize),
                            );
                            let _ = GdiFlush();
                            let bounds = dib_bits.and_then(|(bits, w, h)| {
                                if bits.is_null() || w <= 0 || h <= 0 {
                                    return None;
                                }
                                let x0 = child_rect.left.max(0);
                                let y0 = child_rect.top.max(0);
                                let x1 = child_rect.right.min(w);
                                let y1 = child_rect.bottom.min(h);
                                (x1 > x0 && y1 > y0).then_some((bits, w, h, x0, y0, x1, y1))
                            });
                            if let Some((bits, w, h, x0, y0, x1, y1)) = bounds {
                                let _ = GdiFlush();
                                let a = if glass_enabled(parent) {
                                    SEARCH_ALPHA
                                } else {
                                    255
                                };
                                let inv = 255u8 - a;
                                let temp_slice =
                                    std::slice::from_raw_parts(temp_bits as *const u32, total);
                                let dst_slice =
                                    std::slice::from_raw_parts_mut(bits, (w * h) as usize);
                                for y in y0..y1 {
                                    let dst_row = y as usize * w as usize;
                                    let src_row = (y - child_rect.top) as usize * cw as usize;
                                    for x in x0..x1 {
                                        let src =
                                            temp_slice[src_row + (x - child_rect.left) as usize];
                                        if a == 255 {
                                            dst_slice[dst_row + x as usize] =
                                                (255u32 << 24) | (src & 0x00ff_ffff);
                                        } else {
                                            let bg = dst_slice[dst_row + x as usize];
                                            let bg_a = ((bg >> 24) & 255) as u8;
                                            let bg_r = ((bg >> 16) & 255) as u8;
                                            let bg_g = ((bg >> 8) & 255) as u8;
                                            let bg_b = (bg & 255) as u8;
                                            let src_r = ((src >> 16) & 255) as u8;
                                            let src_g = ((src >> 8) & 255) as u8;
                                            let src_b = (src & 255) as u8;
                                            let out_a = (u16::from(a)
                                                + (u16::from(bg_a) * u16::from(inv) + 127) / 255)
                                                .min(255)
                                                as u8;
                                            let out_r = (u16::from(premul_channel(src_r, a))
                                                + (u16::from(bg_r) * u16::from(inv) + 127) / 255)
                                                .min(u16::from(out_a))
                                                as u8;
                                            let out_g = (u16::from(premul_channel(src_g, a))
                                                + (u16::from(bg_g) * u16::from(inv) + 127) / 255)
                                                .min(u16::from(out_a))
                                                as u8;
                                            let out_b = (u16::from(premul_channel(src_b, a))
                                                + (u16::from(bg_b) * u16::from(inv) + 127) / 255)
                                                .min(u16::from(out_a))
                                                as u8;
                                            dst_slice[dst_row + x as usize] = (u32::from(out_a)
                                                << 24)
                                                | (u32::from(out_r) << 16)
                                                | (u32::from(out_g) << 8)
                                                | u32::from(out_b);
                                        }
                                    }
                                }
                            } else {
                                let _ = BitBlt(
                                    dc,
                                    child_rect.left,
                                    child_rect.top,
                                    cw,
                                    ch,
                                    temp_dc,
                                    0,
                                    0,
                                    SRCCOPY,
                                );
                            }
                        }
                        SelectObject(temp_dc, prev_bmp);
                        let _ = DeleteObject(temp_bmp);
                        let _ = DeleteDC(temp_dc);
                        continue;
                    }
                    let _ = DeleteDC(temp_dc);
                }
            }
            let state = SaveDC(dc);
            let _ = SetViewportOrgEx(dc, child_rect.left, child_rect.top, None);
            let clip_rgn = CreateRectRgn(0, 0, 0, 0);
            if GetWindowRgn(child, clip_rgn).0 != ERROR {
                let _ = OffsetRgn(clip_rgn, child_rect.left, child_rect.top);
                let _ = ExtSelectClipRgn(dc, clip_rgn, RGN_AND);
            }
            let _ = DeleteObject(clip_rgn);
            SendMessageW(
                child,
                WM_PRINT,
                WPARAM(dc.0 as usize),
                LPARAM((PRF_CLIENT | PRF_NONCLIENT | PRF_ERASEBKGND) as isize),
            );
            let _ = RestoreDC(dc, state);
        }
    }
}

pub fn capture_region(
    window: HWND,
    clip: RECT,
    controls: &[(usize, HWND)],
) -> Option<CapturedLayer> {
    if window.0.is_null() {
        return None;
    }
    // SAFETY: transient offscreen 32-bit DIB and DC are deleted before returning.
    unsafe {
        let mut client = RECT::default();
        if GetClientRect(window, &mut client).is_err() {
            return None;
        }
        let bounds = rect(
            clip.left.max(0),
            clip.top.max(0),
            clip.right.min(client.right),
            clip.bottom.min(client.bottom),
        );
        let cw = bounds.right - bounds.left;
        let ch = bounds.bottom - bounds.top;
        if client.right <= 0 || client.bottom <= 0 || cw <= 0 || ch <= 0 {
            return None;
        }
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(screen);
        let _ = ReleaseDC(None, screen);
        if dc.0.is_null() {
            return None;
        }
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(
            dc,
            &BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: client.right,
                    biHeight: -client.bottom,
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
            let _ = DeleteDC(dc);
            return None;
        };
        let prev = SelectObject(dc, bitmap);
        let total = (client.right * client.bottom) as usize;
        std::slice::from_raw_parts_mut(bits as *mut u32, total).fill(0);
        paint_into(window, dc);
        let require_visible = CAPTURE_REQUIRE_VISIBLE.with(Cell::get);
        paint_children_with_visibility(
            window,
            dc,
            controls,
            Some(bounds),
            Some((bits as *mut u32, client.right, client.bottom)),
            require_visible,
        );
        let _ = GdiFlush();
        let src = std::slice::from_raw_parts(bits as *const u32, total);
        let mut pixels = Vec::with_capacity((cw * ch) as usize);
        for y in bounds.top..bounds.bottom {
            let row = y as usize * client.right as usize;
            pixels.extend_from_slice(&src[row + bounds.left as usize..row + bounds.right as usize]);
        }
        SelectObject(dc, prev);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(dc);
        Some(CapturedLayer {
            pixels,
            width: cw,
            height: ch,
        })
    }
}

pub fn capture_region_controls(
    window: HWND,
    bounds: RECT,
    controls: &[(usize, HWND)],
) -> Option<CapturedLayer> {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            CAPTURE_REQUIRE_VISIBLE.with(|slot| slot.set(self.0));
        }
    }
    let prev = CAPTURE_REQUIRE_VISIBLE.with(|slot| slot.replace(false));
    let _guard = Restore(prev);
    capture_region(window, bounds, controls)
}

#[allow(clippy::too_many_arguments)]
pub fn begin_transition(
    window: HWND,
    clip: RECT,
    direction: i32,
    duration_ms: u32,
    old_layer: CapturedLayer,
    new_layer: CapturedLayer,
    hidden: Vec<HWND>,
    restore_focus: Option<HWND>,
    covers_body: bool,
) {
    begin_transition_with_hide(
        window,
        clip,
        direction,
        duration_ms,
        old_layer,
        new_layer,
        &[],
        hidden,
        restore_focus,
        covers_body,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn begin_transition_with_hide(
    window: HWND,
    clip: RECT,
    direction: i32,
    duration_ms: u32,
    old_layer: CapturedLayer,
    new_layer: CapturedLayer,
    to_hide: &[HWND],
    hidden: Vec<HWND>,
    restore_focus: Option<HWND>,
    covers_body: bool,
) {
    cancel_transition(window);
    if !animations_enabled(window) {
        unsafe {
            for &child in to_hide {
                set_child_visible_no_redraw(child, false);
            }
            if !window.0.is_null() && IsWindow(window).as_bool() {
                let _ = RedrawWindow(window, Some(&clip), None, RDW_INVALIDATE | RDW_UPDATENOW);
            }
            for &child in &hidden {
                set_child_visible_no_redraw(child, true);
            }
            if let Some(target) = restore_focus
                && !target.0.is_null()
                && IsWindow(target).as_bool()
                && IsWindowVisible(target).as_bool()
            {
                let _ = SetFocus(target);
            }
            for &child in &hidden {
                if !child.0.is_null() && IsWindow(child).as_bool() {
                    let _ = RedrawWindow(
                        child,
                        None,
                        None,
                        RDW_INVALIDATE | RDW_FRAME | RDW_UPDATENOW,
                    );
                }
            }
        }
        return;
    }
    unsafe {
        for &child in to_hide.iter().chain(hidden.iter()) {
            set_child_visible_no_redraw(child, false);
        }
    }
    ANIM.with(|a| {
        a.borrow_mut().transition = Some(TransitionAnim {
            clip,
            direction,
            start: Instant::now(),
            duration_ms: duration_ms.max(1),
            old_layer,
            new_layer,
            hidden,
            restore_focus,
            covers_body,
        });
    });
    arm_anim_timer(window);
    unsafe {
        let _ = RedrawWindow(window, Some(&clip), None, RDW_INVALIDATE | RDW_UPDATENOW);
    }
}

fn end_transition(window: HWND, anim: TransitionAnim) {
    unsafe {
        // Repaint the parent clip before unhiding children so WS_CLIPCHILDREN cannot trap transition pixels behind WS_EX_LAYERED SEARCH.
        if !window.0.is_null() && IsWindow(window).as_bool() {
            let _ = RedrawWindow(
                window,
                Some(&anim.clip),
                None,
                RDW_INVALIDATE | RDW_UPDATENOW,
            );
        }
        for &child in &anim.hidden {
            set_child_visible_no_redraw(child, true);
        }
        if let Some(target) = anim.restore_focus
            && !target.0.is_null()
            && IsWindow(target).as_bool()
            && IsWindowVisible(target).as_bool()
        {
            let _ = SetFocus(target);
        }
        for &child in &anim.hidden {
            if !child.0.is_null() && IsWindow(child).as_bool() {
                let _ = RedrawWindow(
                    child,
                    None,
                    None,
                    RDW_INVALIDATE | RDW_FRAME | RDW_UPDATENOW,
                );
            }
        }
    }
    drop(anim.old_layer);
    drop(anim.new_layer);
}

pub fn cancel_transition(window: HWND) {
    let taken = ANIM.with(|a| a.borrow_mut().transition.take());
    if let Some(anim) = taken {
        end_transition(window, anim);
        disarm_timer_if_idle(window);
    }
}

pub fn tick_animations(window: HWND) -> bool {
    if window.0.is_null() || unsafe { !IsWindow(window).as_bool() || IsIconic(window).as_bool() } {
        cancel_transition(window);
        ANIM.with(|a| {
            let mut state = a.borrow_mut();
            state.tab_slide = None;
            state.toggles.clear();
        });
        disarm_timer_if_idle(window);
        return false;
    }
    let now = Instant::now();
    let mut invalidate_tabs = false;
    let mut invalidate_toggles = Vec::new();
    let mut finished_transition = None;
    let mut transition_clip = None;
    ANIM.with(|a| {
        let mut state = a.borrow_mut();
        if let Some(slide) = &state.tab_slide {
            invalidate_tabs = true;
            if now.duration_since(slide.start).as_secs_f32() * 1000.0 >= TAB_ANIM_MS as f32 {
                state.tab_slide = None;
            }
        }
        for (&id, entry) in &mut state.toggles {
            if !entry.settled {
                invalidate_toggles.push(id);
                let target_f = if entry.target { 1.0 } else { 0.0 };
                let raw =
                    now.duration_since(entry.start).as_secs_f32() * 1000.0 / TOGGLE_ANIM_MS as f32;
                if raw >= 1.0 {
                    entry.pos = target_f;
                    entry.from = target_f;
                    entry.settled = true;
                } else {
                    entry.pos = entry.from + (target_f - entry.from) * ease_out_cubic(raw);
                }
            }
        }
        if let Some(trans) = &state.transition {
            if now.duration_since(trans.start).as_secs_f32() * 1000.0 >= trans.duration_ms as f32 {
                finished_transition = state.transition.take();
            } else {
                transition_clip = Some(trans.clip);
            }
        }
    });
    unsafe {
        if invalidate_tabs {
            for tab_id in [TAB_LIBRARY, TAB_SETTINGS] {
                if let Ok(h) = GetDlgItem(window, tab_id as i32)
                    && !h.0.is_null()
                {
                    let _ = RedrawWindow(h, None, None, RDW_INVALIDATE | RDW_UPDATENOW);
                }
            }
        }
        for id in invalidate_toggles {
            if let Ok(h) = GetDlgItem(window, id as i32)
                && !h.0.is_null()
            {
                let _ = RedrawWindow(h, None, None, RDW_INVALIDATE | RDW_UPDATENOW);
            }
        }
        if let Some(anim) = finished_transition {
            end_transition(window, anim);
        } else if let Some(clip) = transition_clip {
            let _ = RedrawWindow(window, Some(&clip), None, RDW_INVALIDATE | RDW_UPDATENOW);
        }
    }
    disarm_timer_if_idle(window);
    ANIM.with(|a| a.borrow().has_active())
}

pub fn set_tab(tab: Tab) {
    SKIN.with(|s| s.borrow_mut().tab = tab);
}
pub fn clear() {
    SKIN.with(|s| {
        let mut skin = s.borrow_mut();
        skin.thumbnails.clear();
        skin.thumb_seq = 0;
        skin.tiles.clear();
        skin.selected = None;
        skin.marked.clear();
        skin.playing.clear();
        skin.library_empty = true;
    });
    ANIM.with(|a| {
        let mut state = a.borrow_mut();
        if state.timer_period_active {
            unsafe {
                let _ = timeEndPeriod(1);
            }
        }
        *state = AnimState::default();
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
    static USER_ACRYLIC: Cell<bool> = const { Cell::new(true) };
    static CAPTURE_REQUIRE_VISIBLE: Cell<bool> = const { Cell::new(true) };
}

pub fn set_user_acrylic(enabled: bool) {
    USER_ACRYLIC.with(|slot| slot.set(enabled));
}

pub fn user_acrylic_enabled() -> bool {
    USER_ACRYLIC.with(Cell::get)
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
                    pixel.rgbBlue = premul_channel(pixel.rgbBlue, alpha);
                    pixel.rgbGreen = premul_channel(pixel.rgbGreen, alpha);
                    pixel.rgbRed = premul_channel(pixel.rgbRed, alpha);
                    pixel.rgbReserved = alpha;
                }
            }
        }
        true
    }
    unsafe fn clear_alpha(&self, area: RECT) -> bool {
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
        let pixels = unsafe { std::slice::from_raw_parts_mut(bits, len) };
        for y in area.top.max(self.area.top)..area.bottom.min(self.area.bottom) {
            let row = (y - self.area.top) as usize * stride as usize;
            for x in area.left.max(self.area.left)..area.right.min(self.area.right) {
                pixels[row + (x - self.area.left) as usize].rgbReserved = 0;
            }
        }
        true
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
        let (tr, tg, tb) = split_rgb(mask.top_rim);
        let (dr, dg, db) = split_rgb(mask.bottom_rim);
        let (fr, fg, fb) = split_rgb(mask.focus);
        let (hr, hg, hb) = split_rgb(mask.host_color);
        let fa = f32::from(mask.fill_alpha);
        let fill_scale = fa / 255.0;
        let ba = f32::from(mask.border_alpha);
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
        let light_band = (radius + 4.0).clamp(2.0, hh.max(2.0));
        let apply_sheen = |c: f32, target: f32, weight: f32| -> f32 {
            (c + (target - c) * weight).clamp(0.0, 255.0)
        };
        for y in rect_top..rect_bottom {
            let row_offset = (y - self.area.top) as usize * stride as usize;
            let col_start = (rect_left - self.area.left) as usize;
            let col_end = (rect_right - self.area.left) as usize;
            let row = &mut pixels[row_offset + col_start..row_offset + col_end];
            let yi = y - bounds.top;
            let py = yi as f32 + 0.5;
            let top_edge = if mask.top_rim != 0 && py < light_band {
                (1.0 - (py - 0.5) / light_band).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let bot_edge = if mask.bottom_rim != 0 && py > h - light_band {
                (1.0 - ((h - 0.5) - py) / light_band).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let face_sheen = if mask.sheen_strength != 0.0 && h > 0.0 {
                let v = (py / h).clamp(0.0, 1.0);
                if v < 0.42 {
                    mask.sheen_strength * (1.0 - v / 0.42)
                } else if v > 0.62 {
                    -mask.sheen_strength * 0.55 * ((v - 0.62) / 0.38)
                } else {
                    0.0
                }
            } else {
                0.0
            };
            let (sheen_target_r, sheen_target_g, sheen_target_b, sheen_w) = if face_sheen > 0.0 {
                (250.0, 246.0, 238.0, face_sheen.clamp(0.0, 0.35))
            } else {
                (14.0, 14.0, 16.0, (-face_sheen).clamp(0.0, 0.35))
            };
            let top_w = top_edge * 0.58;
            let bot_w = bot_edge * 0.48;
            let mut row_br = br;
            let mut row_bg = bg;
            let mut row_bb = bb;
            if top_w > 0.0 {
                row_br += (tr - row_br) * top_w;
                row_bg += (tg - row_bg) * top_w;
                row_bb += (tb - row_bb) * top_w;
            }
            if bot_w > 0.0 {
                row_br += (dr - row_br) * bot_w;
                row_bg += (dg - row_bg) * bot_w;
                row_bb += (db - row_bb) * bot_w;
            }
            let row_ba = (ba + (255.0 - ba) * 0.38 * top_edge).clamp(0.0, 255.0);
            let border_scale = row_ba / 255.0;
            let border_pre_r = row_br * border_scale;
            let border_pre_g = row_bg * border_scale;
            let border_pre_b = row_bb * border_scale;
            let y_inner = yi >= edge_band && yi < bounds_h - edge_band;
            let y_deep = yi >= corner_band && yi < bounds_h - corner_band;
            for (offset, pixel) in row.iter_mut().enumerate() {
                let xi = rect_left - bounds.left + offset as i32;
                if y_deep && xi >= edge_band && xi < bounds_w - edge_band
                    || y_inner && xi >= corner_band && xi < bounds_w - corner_band
                {
                    if pixel.rgbReserved == 0 {
                        let (r, g, b) = if sheen_w > 0.0 {
                            (
                                apply_sheen(f32::from(pixel.rgbRed), sheen_target_r, sheen_w),
                                apply_sheen(f32::from(pixel.rgbGreen), sheen_target_g, sheen_w),
                                apply_sheen(f32::from(pixel.rgbBlue), sheen_target_b, sheen_w),
                            )
                        } else {
                            (
                                f32::from(pixel.rgbRed),
                                f32::from(pixel.rgbGreen),
                                f32::from(pixel.rgbBlue),
                            )
                        };
                        if mask.fill_alpha < 255 {
                            pixel.rgbRed = premul_channel(r.round() as u8, mask.fill_alpha);
                            pixel.rgbGreen = premul_channel(g.round() as u8, mask.fill_alpha);
                            pixel.rgbBlue = premul_channel(b.round() as u8, mask.fill_alpha);
                        } else {
                            pixel.rgbRed = r.round() as u8;
                            pixel.rgbGreen = g.round() as u8;
                            pixel.rgbBlue = b.round() as u8;
                        }
                        pixel.rgbReserved = mask.fill_alpha;
                    }
                    continue;
                }
                let px = xi as f32 + 0.5;
                let qx = (px - hw).abs() - (hw - radius);
                let qy = (py - hh).abs() - (hh - radius);
                let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
                if d <= -safe_inset {
                    if pixel.rgbReserved == 0 {
                        let (r, g, b) = if sheen_w > 0.0 {
                            (
                                apply_sheen(f32::from(pixel.rgbRed), sheen_target_r, sheen_w),
                                apply_sheen(f32::from(pixel.rgbGreen), sheen_target_g, sheen_w),
                                apply_sheen(f32::from(pixel.rgbBlue), sheen_target_b, sheen_w),
                            )
                        } else {
                            (
                                f32::from(pixel.rgbRed),
                                f32::from(pixel.rgbGreen),
                                f32::from(pixel.rgbBlue),
                            )
                        };
                        if mask.fill_alpha < 255 {
                            pixel.rgbRed = premul_channel(r.round() as u8, mask.fill_alpha);
                            pixel.rgbGreen = premul_channel(g.round() as u8, mask.fill_alpha);
                            pixel.rgbBlue = premul_channel(b.round() as u8, mask.fill_alpha);
                        } else {
                            pixel.rgbRed = r.round() as u8;
                            pixel.rgbGreen = g.round() as u8;
                            pixel.rgbBlue = b.round() as u8;
                        }
                        pixel.rgbReserved = mask.fill_alpha;
                    }
                    continue;
                }
                let shape_cov = (0.5 - d).clamp(0.0, 1.0);
                let inner_cov = (0.5 - (d + mask.border_width)).clamp(0.0, 1.0);
                let border_cov = (shape_cov - inner_cov).max(0.0);
                let out_cov = 1.0 - shape_cov;
                let (src_r, src_g, src_b) = if pixel.rgbReserved == 0 && sheen_w > 0.0 {
                    (
                        apply_sheen(f32::from(pixel.rgbRed), sheen_target_r, sheen_w),
                        apply_sheen(f32::from(pixel.rgbGreen), sheen_target_g, sheen_w),
                        apply_sheen(f32::from(pixel.rgbBlue), sheen_target_b, sheen_w),
                    )
                } else {
                    (
                        f32::from(pixel.rgbRed),
                        f32::from(pixel.rgbGreen),
                        f32::from(pixel.rgbBlue),
                    )
                };
                let (mut fill_pre_r, mut fill_pre_g, mut fill_pre_b, mut fill_a) =
                    if pixel.rgbReserved == 255 {
                        (src_r, src_g, src_b, 255.0)
                    } else {
                        (
                            src_r * fill_scale,
                            src_g * fill_scale,
                            src_b * fill_scale,
                            fa,
                        )
                    };
                if mask.focus != 0 {
                    let focus_cov = (1.0 - (d - focus_center).abs()).clamp(0.0, 1.0);
                    if focus_cov > 0.0 {
                        fill_pre_r = fill_pre_r * (1.0 - focus_cov) + fr * focus_cov;
                        fill_pre_g = fill_pre_g * (1.0 - focus_cov) + fg * focus_cov;
                        fill_pre_b = fill_pre_b * (1.0 - focus_cov) + fb * focus_cov;
                        fill_a = fill_a * (1.0 - focus_cov) + 255.0 * focus_cov;
                    }
                }
                let out_a = (fill_a * inner_cov + row_ba * border_cov + ha * out_cov)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let blend = |f_pre: f32, b_pre: f32, h_pre: f32| -> u8 {
                    (f_pre * inner_cov + b_pre * border_cov + h_pre * out_cov)
                        .round()
                        .clamp(0.0, f32::from(out_a)) as u8
                };
                pixel.rgbRed = blend(fill_pre_r, border_pre_r, host_pre_r);
                pixel.rgbGreen = blend(fill_pre_g, border_pre_g, host_pre_g);
                pixel.rgbBlue = blend(fill_pre_b, border_pre_b, host_pre_b);
                pixel.rgbReserved = out_a;
            }
        }
        true
    }
    unsafe fn draw_toggle_switch(
        &self,
        track: RECT,
        t: f32,
        pressed: bool,
        hot: bool,
        disabled: bool,
        glass: bool,
    ) -> bool {
        let width = self.area.right - self.area.left;
        let height = self.area.bottom - self.area.top;
        let tw = track.right - track.left;
        let th = track.bottom - track.top;
        if width <= 0 || height <= 0 || tw <= 4 || th <= 4 {
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
        let pixels = unsafe { std::slice::from_raw_parts_mut(bits, len) };
        let w = tw as f32;
        let h = th as f32;
        let hw = w * 0.5;
        let hh = h * 0.5;
        let pill_r = hh;
        let pad = (h * 0.14).max(2.0);
        let knob_r = (hh - pad).max(3.0);
        let knob_cx = pad + knob_r + t * (w - 2.0 * (pad + knob_r));
        let knob_cy = hh;

        let off_fill = if disabled {
            DISABLED_FILL
        } else if pressed {
            CARD_LIT
        } else if hot {
            mix(INK, CREAM, 12, 100)
        } else {
            mix(INK, CREAM, 7, 100)
        };
        let on_fill = if disabled {
            DISABLED_BORDER
        } else if pressed {
            ACCENT_DARK
        } else if hot {
            ACCENT_HOVER
        } else {
            ACCENT
        };
        let track_color = lerp_rgb(off_fill, on_fill, t);
        let off_alpha = if !glass {
            255
        } else if hot || pressed {
            168
        } else {
            132
        };
        let on_alpha = if glass { 244 } else { 255 };
        let track_alpha = lerp_u8(off_alpha, on_alpha, t) as f32;
        let off_border = if disabled { DISABLED_BORDER } else { HIGHLIGHT };
        let on_border = if pressed { ACCENT_DEEP } else { ACCENT_SOFT };
        let border_color = lerp_rgb(off_border, on_border, t);
        let split = |c: u32| -> (f32, f32, f32) {
            (
                ((c >> 16) & 255) as f32,
                ((c >> 8) & 255) as f32,
                (c & 255) as f32,
            )
        };
        let (tfr, tfg, tfb) = split(track_color);
        let (tbr, tbg, tbb) = split(border_color);
        let (kr_base, kg_base, kb_base) = split(if disabled { DISABLED_TEXT } else { CREAM });
        let over_pre = |dst_r: f32,
                        dst_g: f32,
                        dst_b: f32,
                        dst_a: f32,
                        src_r: f32,
                        src_g: f32,
                        src_b: f32,
                        src_a: f32|
         -> (f32, f32, f32, f32) {
            let sa = (src_a / 255.0).clamp(0.0, 1.0);
            let inv = 1.0 - sa;
            (
                src_r * sa + dst_r * inv,
                src_g * sa + dst_g * inv,
                src_b * sa + dst_b * inv,
                (src_a + dst_a * inv).clamp(0.0, 255.0),
            )
        };
        let y0 = (track.top - 1).max(self.area.top);
        let y1 = (track.bottom + 2).min(self.area.bottom);
        let x0 = (track.left - 1).max(self.area.left);
        let x1 = (track.right + 1).min(self.area.right);
        for y in y0..y1 {
            let row = (y - self.area.top) as usize * stride as usize;
            let py = (y - track.top) as f32 + 0.5;
            for x in x0..x1 {
                let pixel = &mut pixels[row + (x - self.area.left) as usize];
                let px = (x - track.left) as f32 + 0.5;
                let qx = (px - hw).abs() - (hw - pill_r).max(0.0);
                let qy = (py - hh).abs() - (hh - pill_r).max(0.0);
                let d_track = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - pill_r;
                let bg_r = f32::from(pixel.rgbRed);
                let bg_g = f32::from(pixel.rgbGreen);
                let bg_b = f32::from(pixel.rgbBlue);
                let bg_a = if !glass && pixel.rgbReserved == 0 {
                    255.0
                } else {
                    f32::from(pixel.rgbReserved)
                };

                let track_cov = (0.5 - d_track).clamp(0.0, 1.0);
                let inner_cov = (0.5 - (d_track + 1.0)).clamp(0.0, 1.0);
                let border_cov = (track_cov - inner_cov).max(0.0);
                // Concave inner recess when OFF, warm dome lift when ON.
                let v = (py / h).clamp(0.0, 1.0);
                let depth = ((0.5 - v) * 2.0) * (t * 0.14 - (1.0 - t) * 0.12);
                let face_r = (tfr * (1.0 + depth)).clamp(0.0, 255.0);
                let face_g = (tfg * (1.0 + depth)).clamp(0.0, 255.0);
                let face_b = (tfb * (1.0 + depth)).clamp(0.0, 255.0);
                let trk_a = track_alpha * inner_cov + 210.0 * border_cov;
                let trk_r = if track_cov > 0.0 {
                    (face_r * track_alpha * inner_cov + tbr * 210.0 * border_cov) / trk_a.max(1.0)
                } else {
                    0.0
                };
                let trk_g = if track_cov > 0.0 {
                    (face_g * track_alpha * inner_cov + tbg * 210.0 * border_cov) / trk_a.max(1.0)
                } else {
                    0.0
                };
                let trk_b = if track_cov > 0.0 {
                    (face_b * track_alpha * inner_cov + tbb * 210.0 * border_cov) / trk_a.max(1.0)
                } else {
                    0.0
                };
                let (mut cur_r, mut cur_g, mut cur_b, mut cur_a) = if track_cov > 0.0 {
                    over_pre(bg_r, bg_g, bg_b, bg_a, trk_r, trk_g, trk_b, trk_a)
                } else {
                    (bg_r, bg_g, bg_b, bg_a)
                };

                // Soft 3D knob shadow over track.
                let d_shadow = (px - knob_cx).hypot(py - (knob_cy + 1.2)) - (knob_r + 0.9);
                let shadow_cov = (0.5 - d_shadow).clamp(0.0, 1.0);
                if shadow_cov > 0.0 {
                    let s_a = shadow_cov * 96.0;
                    (cur_r, cur_g, cur_b, cur_a) =
                        over_pre(cur_r, cur_g, cur_b, cur_a, 10.0, 10.0, 12.0, s_a);
                }

                // 3D spherical thumb with upper-left specular highlight and grounded rim.
                let d_knob = (px - knob_cx).hypot(py - knob_cy) - knob_r;
                let knob_cov = (0.5 - d_knob).clamp(0.0, 1.0);
                if knob_cov > 0.0 {
                    let nx = ((px - knob_cx) / knob_r).clamp(-1.0, 1.0);
                    let ny = ((py - knob_cy) / knob_r).clamp(-1.0, 1.0);
                    let spec = (-nx * 0.45 - ny * 0.85).clamp(0.0, 1.0) * 0.14;
                    let rim = (d_knob / 1.0 + 0.5).clamp(0.0, 1.0) * 0.06;
                    let kr = (kr_base + (255.0 - kr_base) * spec - kr_base * rim).clamp(0.0, 255.0);
                    let kg = (kg_base + (255.0 - kg_base) * spec - kg_base * rim).clamp(0.0, 255.0);
                    let kb = (kb_base + (255.0 - kb_base) * spec - kb_base * rim).clamp(0.0, 255.0);
                    (cur_r, cur_g, cur_b, cur_a) =
                        over_pre(cur_r, cur_g, cur_b, cur_a, kr, kg, kb, knob_cov * 255.0);
                }
                let out_a = cur_a.round().clamp(0.0, 255.0) as u8;
                pixel.rgbRed = cur_r.round().clamp(0.0, f32::from(out_a)) as u8;
                pixel.rgbGreen = cur_g.round().clamp(0.0, f32::from(out_a)) as u8;
                pixel.rgbBlue = cur_b.round().clamp(0.0, f32::from(out_a)) as u8;
                pixel.rgbReserved = out_a;
            }
        }
        true
    }
    unsafe fn composite_transition(
        &self,
        trans: &TransitionAnim,
        now: Instant,
        glass_active: bool,
    ) -> bool {
        let width = self.area.right - self.area.left;
        let height = self.area.bottom - self.area.top;
        let mut bits = std::ptr::null_mut();
        let mut stride = 0;
        if width <= 0
            || height <= 0
            || unsafe { GetBufferedPaintBits(self.handle, &mut bits, &mut stride) }.is_err()
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
        let pixels = unsafe { std::slice::from_raw_parts_mut(bits, len) };
        let elapsed = now.duration_since(trans.start).as_secs_f32() * 1000.0;
        let raw = (elapsed / trans.duration_ms as f32).clamp(0.0, 1.0);
        let ease = ease_out_cubic(raw);
        let clip_w = (trans.clip.right - trans.clip.left).max(1) as f32;
        let slide = 44.0 * (clip_w / 640.0).clamp(0.85, 1.35);
        // Subpixel 256th-px shifts eliminate discrete integer pixel snapping / edge jitter.
        let dx_old_fp = (trans.direction as f32 * slide * ease * 256.0).round() as i32;
        let dx_new_fp = (-trans.direction as f32 * slide * (1.0 - ease) * 256.0).round() as i32;
        let w_new = (ease * 256.0).round().clamp(0.0, 256.0) as u32;
        let w_old = 256u32 - w_new;
        let sample_subpixel_x =
            |layer: &CapturedLayer, lx_fp: i32, ly: i32| -> (u32, u32, u32, u32) {
                if ly < 0 || ly >= layer.height {
                    return (0, 0, 0, 0);
                }
                let x0 = lx_fp >> 8;
                let fx = (lx_fp & 255) as u32;
                let inv = 256u32 - fx;
                let row = (ly * layer.width) as usize;
                let p0 = if x0 >= 0 && x0 < layer.width {
                    layer.pixels[row + x0 as usize]
                } else {
                    0
                };
                let p1 = if x0 + 1 >= 0 && x0 + 1 < layer.width {
                    layer.pixels[row + (x0 + 1) as usize]
                } else {
                    0
                };
                let mix_ch = |shift: u32| -> u32 {
                    (((p0 >> shift) & 255) * inv + ((p1 >> shift) & 255) * fx + 128) >> 8
                };
                (mix_ch(16), mix_ch(8), mix_ch(0), mix_ch(24))
            };
        let y0 = trans.clip.top.max(self.area.top);
        let y1 = trans.clip.bottom.min(self.area.bottom);
        let x0 = trans.clip.left.max(self.area.left);
        let x1 = trans.clip.right.min(self.area.right);
        for y in y0..y1 {
            let row = (y - self.area.top) as usize * stride as usize;
            let ly = y - trans.clip.top;
            for x in x0..x1 {
                let lx_fp = (x - trans.clip.left) << 8;
                let (or, og, ob, oa) = sample_subpixel_x(&trans.old_layer, lx_fp - dx_old_fp, ly);
                let (nr, ng, nb, na) = sample_subpixel_x(&trans.new_layer, lx_fp - dx_new_fp, ly);
                let mut fr = (or * w_old + nr * w_new + 128) >> 8;
                let mut fg = (og * w_old + ng * w_new + 128) >> 8;
                let mut fb = (ob * w_old + nb * w_new + 128) >> 8;
                let mut fa = ((oa * w_old + na * w_new + 128) >> 8).min(255);
                let pixel = &mut pixels[row + (x - self.area.left) as usize];
                if !glass_active {
                    let inv = 255u32.saturating_sub(fa);
                    fr = (fr + ((u32::from(pixel.rgbRed) * inv + 127) / 255)).min(255);
                    fg = (fg + ((u32::from(pixel.rgbGreen) * inv + 127) / 255)).min(255);
                    fb = (fb + ((u32::from(pixel.rgbBlue) * inv + 127) / 255)).min(255);
                    fa = 255;
                }
                let out_a = fa as u8;
                pixel.rgbRed = fr.min(fa) as u8;
                pixel.rgbGreen = fg.min(fa) as u8;
                pixel.rgbBlue = fb.min(fa) as u8;
                pixel.rgbReserved = out_a;
            }
        }
        true
    }
    unsafe fn draw_caption_glyph(
        &self,
        area: RECT,
        btn: CaptionButton,
        maximized: bool,
        dpi: i32,
        color: u32,
    ) -> bool {
        let width = self.area.right - self.area.left;
        let height = self.area.bottom - self.area.top;
        let mut bits = std::ptr::null_mut();
        let mut stride = 0;
        if width <= 0
            || height <= 0
            || unsafe { GetBufferedPaintBits(self.handle, &mut bits, &mut stride) }.is_err()
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
        let pixels = unsafe { std::slice::from_raw_parts_mut(bits, len) };
        let scale_f = dpi as f32 / 96.0;
        let cx = (area.left + area.right) as f32 * 0.5;
        let cy = (area.top + area.bottom) as f32 * 0.5;
        let half_len = 5.0 * scale_f;
        let half_w = (0.65 * scale_f).max(0.55);
        let r = ((color >> 16) & 255) as f32;
        let g = ((color >> 8) & 255) as f32;
        let b = (color & 255) as f32;
        let segment_dist = |px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32| -> f32 {
            let abx = bx - ax;
            let aby = by - ay;
            let denom = abx * abx + aby * aby;
            let t = if denom > 0.0 {
                (((px - ax) * abx + (py - ay) * aby) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let dx = px - (ax + abx * t);
            let dy = py - (ay + aby * t);
            (dx * dx + dy * dy).sqrt()
        };
        let box_outline_dist = |px: f32, py: f32, left: f32, top: f32, right: f32, bottom: f32| {
            let d1 = segment_dist(px, py, left, top, right, top);
            let d2 = segment_dist(px, py, right, top, right, bottom);
            let d3 = segment_dist(px, py, right, bottom, left, bottom);
            let d4 = segment_dist(px, py, left, bottom, left, top);
            d1.min(d2).min(d3).min(d4)
        };
        let y0 = area.top.max(self.area.top);
        let y1 = area.bottom.min(self.area.bottom);
        let x0 = area.left.max(self.area.left);
        let x1 = area.right.min(self.area.right);
        for y in y0..y1 {
            let row = (y - self.area.top) as usize * stride as usize;
            let py = y as f32 + 0.5;
            for x in x0..x1 {
                let px = x as f32 + 0.5;
                let dist = match btn {
                    CaptionButton::Minimize => {
                        segment_dist(px, py, cx - half_len, cy, cx + half_len, cy)
                    }
                    CaptionButton::Maximize if !maximized => box_outline_dist(
                        px,
                        py,
                        cx - half_len,
                        cy - half_len,
                        cx + half_len,
                        cy + half_len,
                    ),
                    CaptionButton::Maximize => {
                        let s = 3.8 * scale_f;
                        let shift = 2.0 * scale_f;
                        let front = box_outline_dist(
                            px,
                            py,
                            cx - s - shift * 0.5,
                            cy - s + shift * 0.5,
                            cx + s - shift * 0.5,
                            cy + s + shift * 0.5,
                        );
                        let back_top = segment_dist(
                            px,
                            py,
                            cx - s + shift * 0.5,
                            cy - s - shift * 0.5,
                            cx + s + shift * 0.5,
                            cy - s - shift * 0.5,
                        );
                        let back_right = segment_dist(
                            px,
                            py,
                            cx + s + shift * 0.5,
                            cy - s - shift * 0.5,
                            cx + s + shift * 0.5,
                            cy + s - shift * 0.5,
                        );
                        front.min(back_top).min(back_right)
                    }
                    CaptionButton::Close => {
                        let d1 = segment_dist(
                            px,
                            py,
                            cx - half_len,
                            cy - half_len,
                            cx + half_len,
                            cy + half_len,
                        );
                        let d2 = segment_dist(
                            px,
                            py,
                            cx + half_len,
                            cy - half_len,
                            cx - half_len,
                            cy + half_len,
                        );
                        d1.min(d2)
                    }
                };
                let cov = (half_w + 0.5 - dist).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let sa = (cov * 255.0).round() as u32;
                let inv = 255u32.saturating_sub(sa);
                let pixel = &mut pixels[row + (x - self.area.left) as usize];
                let out_a = (sa + (u32::from(pixel.rgbReserved) * inv + 127) / 255).min(255);
                let sr = (r * cov).round() as u32;
                let sg = (g * cov).round() as u32;
                let sb = (b * cov).round() as u32;
                pixel.rgbRed =
                    ((sr + (u32::from(pixel.rgbRed) * inv + 127) / 255) as u8).min(out_a as u8);
                pixel.rgbGreen =
                    ((sg + (u32::from(pixel.rgbGreen) * inv + 127) / 255) as u8).min(out_a as u8);
                pixel.rgbBlue =
                    ((sb + (u32::from(pixel.rgbBlue) * inv + 127) / 255) as u8).min(out_a as u8);
                pixel.rgbReserved = out_a as u8;
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
    border_alpha: u8,
    fill_alpha: u8,
    focus: u32,
    focus_inset: f32,
    host_color: u32,
    host_alpha: u8,
    top_rim: u32,
    bottom_rim: u32,
    sheen_strength: f32,
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
            let glass = glass_enabled(parent);
            let theme = if glass {
                OpenThemeData(parent, w!("WINDOW"))
            } else {
                HTHEME(0)
            };
            let old_theme = TEXT_THEME.with(|slot| slot.replace(theme));
            let old_buffer = GLASS_BUFFER.with(|slot| slot.replace(buffer.handle));
            let buffered = DRAWITEMSTRUCT {
                hDC: buffer.dc,
                ..*item
            };
            let ok = draw_item_into(&buffered, glass, Some(&buffer));
            TEXT_THEME.with(|slot| slot.set(old_theme));
            GLASS_BUFFER.with(|slot| slot.set(old_buffer));
            if theme.0 != 0 {
                let _ = CloseThemeData(theme);
            }
            if ok && buffer.finish() {
                return;
            }
        }
        if !parent.0.is_null() {
            disable_backdrop(parent);
        }
        let _ = draw_item_into(item, false, None);
    }
}

unsafe fn draw_item_into(
    item: &DRAWITEMSTRUCT,
    glass_active: bool,
    buffer: Option<&PaintBuffer>,
) -> bool {
    let id = item.CtlID as usize;
    let dpi = unsafe { GetDpiForWindow(item.hwndItem) }.max(96) as i32;
    let s = |n: i32| n * dpi / 96;
    let r = item.rcItem;
    let dc = item.hDC;
    let buffered = buffer.is_some();
    let (host_color, glass_host_alpha) = control_surface(id);
    let is_card_host = host_color == CARD;
    let host_alpha = if glass_active { glass_host_alpha } else { 255 };
    let ellipse = control_corner_ellipse(id, dpi);
    // SAFETY: child rect pixels remain owned for the duration of synchronous owner-draw dispatch.
    unsafe {
        if id == PAGE {
            fill(dc, r, host_color);
            if let Some(buffer) = buffer
                && !buffer.tint_background(r, host_alpha)
            {
                return false;
            }
            label(
                dc,
                &window_text(item.hwndItem),
                r,
                s(13),
                false,
                MUTED,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            return true;
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
        let current_tab = SKIN.with(|s| s.borrow().tab);
        let active_tab = is_tab
            && ((current_tab != Tab::Settings && id == TAB_LIBRARY)
                || (current_tab == Tab::Settings && id == TAB_SETTINGS));
        let tab_weight = if !is_tab {
            0.0
        } else {
            let sw = tab_settings_weight(current_tab);
            if id == TAB_SETTINGS { sw } else { 1.0 - sw }
        };
        let hot = is_hot(item.hwndItem);
        let focus_color = if !focus_visible {
            0
        } else if (is_tab && active_tab) || (id == 102 && item.itemState.0 & ODS_SELECTED.0 == 0) {
            TEXT_ON_DARK
        } else {
            ACCENT_SOFT
        };
        if id == PREVIEW || (TILE_BASE..TILE_BASE + 9).contains(&id) {
            let mut active_tile = false;
            let mut playing = false;
            let mut missing_label = None;
            let mut title_text = None;
            let mut checked = None;
            let mut card_border = LINE;
            let mut card_top_rim = 0u32;
            let mut card_bottom_rim = INK;
            let mut card_sheen = 0.0f32;
            let mut fill_alpha = 255u8;
            let mut border_alpha = 255u8;
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
            SKIN.with(|skin| {
                let skin = skin.borrow();
                let path = if id == PREVIEW {
                    skin.selected.as_ref()
                } else {
                    skin.tiles.get(&id)
                };
                let active = id != PREVIEW && path.is_some() && path == skin.selected.as_ref();
                active_tile = active;
                playing = path.is_some_and(|path| skin.playing.contains(path));
                let card_color = if id == PREVIEW {
                    INK
                } else if active {
                    ACTIVE_SURFACE
                } else if hot {
                    CARD_LIT
                } else {
                    CARD
                };
                card_border = if active {
                    ACCENT
                } else if hot && id != PREVIEW {
                    HIGHLIGHT
                } else {
                    LINE
                };
                fill_alpha = if !glass_active {
                    255
                } else if id == PREVIEW {
                    if path.is_some() { CARD_ALPHA } else { 44 }
                } else if active {
                    ACTIVE_TILE_ALPHA
                } else if hot {
                    HOVER_ALPHA
                } else {
                    CONTROL_ALPHA
                };
                border_alpha = if !glass_active || active {
                    255
                } else if hot && id != PREVIEW {
                    196
                } else {
                    BORDER_ALPHA
                };
                let card_highlight = if id == PREVIEW {
                    0
                } else if active {
                    ACCENT_SOFT
                } else if hot {
                    CREAM
                } else {
                    GLASS_RIM
                };
                card_top_rim = card_highlight;
                card_bottom_rim = if active { ACCENT_DEEP } else { INK };
                card_sheen = if id == PREVIEW {
                    0.0
                } else if hot {
                    0.08
                } else {
                    0.05
                };
                if buffered {
                    fill(dc, r, card_color);
                } else {
                    fill(dc, r, host_color);
                    raised_rounded(dc, r, card_color, card_border, card_highlight, ellipse);
                }
                if let Some(path) = path {
                    if let Some(entry) = skin.thumbnails.get(path) {
                        image(dc, entry.bitmap.0, area);
                        if let Some(buffer) = buffer {
                            let _ = GdiFlush();
                            let _ = BufferedPaintSetAlpha(buffer.handle, Some(&area), 255);
                        }
                    } else {
                        missing_label = Some(extension(path));
                    }
                    if id != PREVIEW {
                        title_text = Some(filename(path));
                        checked = Some(skin.marked.contains(path));
                    }
                } else {
                    missing_label = Some("Select a wallpaper".to_owned());
                }
            });
            if let Some(buffer) = buffer {
                let mask = RoundedMask {
                    ellipse,
                    border: card_border,
                    border_width: if active_tile { s(2).max(1) as f32 } else { 1.0 },
                    border_alpha,
                    fill_alpha,
                    focus: focus_color,
                    focus_inset: s(3).max(2) as f32,
                    host_color,
                    host_alpha,
                    top_rim: card_top_rim,
                    bottom_rim: card_bottom_rim,
                    sheen_strength: card_sheen,
                };
                if !buffer.apply_rounded_rect_mask(r, &mask) {
                    return false;
                }
            }
            if let Some(ref missing) = missing_label {
                label(
                    dc,
                    missing,
                    area,
                    s(if title_text.is_some() || missing != "Select a wallpaper" {
                        24
                    } else {
                        15
                    }),
                    missing != "Select a wallpaper",
                    if id == PREVIEW { MUTED_ON_DARK } else { MUTED },
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                );
            }
            if let Some(ref title) = title_text {
                let title_rect = rect(
                    r.left + s(8),
                    r.bottom - s(24),
                    r.right - s(8),
                    r.bottom - s(4),
                );
                label(
                    dc,
                    title,
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
                    if let Some(buffer) = buffer {
                        let _ = buffer.tint_background(badge_rect, 255);
                    }
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
            if let Some(checked) = checked {
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
                if let Some(buffer) = buffer {
                    let _ = buffer.tint_background(box_rect, 255);
                }
            }
            if !buffered && active_tile {
                let pen = CreatePen(PS_SOLID, s(2).max(1), rgb(ACCENT));
                let old_pen = SelectObject(dc, pen);
                let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
                let _ = RoundRect(dc, r.left, r.top, r.right, r.bottom, ellipse, ellipse);
                SelectObject(dc, old_brush);
                SelectObject(dc, old_pen);
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
            return true;
        }

        let text = window_text(item.hwndItem);
        let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
        let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
        let primary = id == 102;
        let (color, border, top_rim, bottom_rim, sheen_strength) = if disabled {
            (
                if is_card_host { DISABLED_FILL } else { PANEL },
                DISABLED_BORDER,
                0,
                0,
                0.0,
            )
        } else if is_tab {
            let base_c = if pressed {
                mix(INK, CREAM, 12, 100)
            } else if hot {
                CARD_LIT
            } else {
                CARD
            };
            let active_c = if pressed { ACCENT_DARK } else { ACCENT };
            let c = lerp_rgb(base_c, active_c, tab_weight);
            let b = lerp_rgb(if hot { HIGHLIGHT } else { LINE }, ACCENT_DARK, tab_weight);
            let top = lerp_rgb(
                if pressed {
                    INK
                } else if hot {
                    CREAM
                } else {
                    GLASS_RIM
                },
                if pressed { ACCENT_DEEP } else { ACCENT_SOFT },
                tab_weight,
            );
            let bot = lerp_rgb(INK, ACCENT_DEEP, tab_weight);
            let sheen = if pressed {
                -0.07
            } else {
                0.05 + 0.06 * tab_weight + if hot { 0.03 } else { 0.0 }
            };
            (c, b, top, bot, sheen)
        } else if primary {
            if pressed {
                (ACCENT_DARK, ACCENT_DEEP, ACCENT_DEEP, ACCENT_SOFT, -0.08)
            } else if hot {
                (ACCENT_HOVER, ACCENT, CREAM, ACCENT_DEEP, 0.13)
            } else {
                (ACCENT, ACCENT_DARK, ACCENT_SOFT, ACCENT_DEEP, 0.10)
            }
        } else if pressed {
            (CARD_LIT, HIGHLIGHT, INK, HIGHLIGHT, -0.07)
        } else if hot {
            (CARD_LIT, HIGHLIGHT, CREAM, INK, 0.08)
        } else {
            (CARD, LINE, GLASS_RIM, INK, 0.05)
        };
        let fill_alpha = if !glass_active {
            255
        } else if disabled {
            if is_card_host { 72 } else { 80 }
        } else if is_tab {
            let base_a = if pressed {
                176
            } else if hot {
                HOVER_ALPHA
            } else {
                CARD_ALPHA
            };
            lerp_u8(base_a, ACTIVE_ALPHA, tab_weight)
        } else if primary {
            ACTIVE_ALPHA
        } else if pressed {
            188
        } else if hot {
            HOVER_ALPHA
        } else {
            CONTROL_ALPHA
        };
        let border_alpha = if !glass_active || (primary && !disabled) {
            255
        } else if is_tab {
            lerp_u8(if hot { 216 } else { BORDER_ALPHA }, 255, tab_weight)
        } else if disabled {
            132
        } else if hot || pressed {
            224
        } else {
            BORDER_ALPHA
        };
        if buffered {
            fill(dc, r, color);
        } else {
            fill(dc, r, host_color);
            raised_rounded(dc, r, color, border, top_rim, ellipse);
        }
        if let Some(buffer) = buffer {
            let mask = RoundedMask {
                ellipse,
                border,
                border_width: 1.0,
                border_alpha,
                fill_alpha,
                focus: focus_color,
                focus_inset: s(3).max(2) as f32,
                host_color,
                host_alpha,
                top_rim,
                bottom_rim,
                sheen_strength,
            };
            if !buffer.apply_rounded_rect_mask(r, &mask) {
                return false;
            }
        }
        let selector = matches!(id, 105 | 106 | FOLDER_FILTER | FPS | SORT);
        let label_color = if disabled {
            DISABLED_TEXT
        } else if primary && pressed {
            TEXT_ON_DARK
        } else if primary {
            TEXT_ON_ACCENT
        } else if is_tab {
            lerp_rgb(
                TEXT_ON_DARK,
                TEXT_ON_ACCENT,
                if tab_weight >= 0.55 { tab_weight } else { 0.0 },
            )
        } else {
            TEXT
        };
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
            label_color,
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
            if let Some(buffer) = buffer {
                let chevron = rect(x - s(6), y - s(4), x + s(6), y + s(4));
                let _ = buffer.tint_background(chevron, 255);
            }
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
        true
    }
}

#[repr(C)]
struct WindowCompositionAttribData {
    attrib: u32,
    pv_data: *mut std::ffi::c_void,
    cb_data: usize,
}

#[repr(C)]
struct AccentPolicy {
    accent_state: u32,
    accent_flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

pub fn transparency_enabled() -> bool {
    // SAFETY: read-only OS high-contrast flag and HKCU transparency registry setting.
    unsafe {
        let mut hc = HIGHCONTRASTW {
            cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
            ..Default::default()
        };
        if SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            Some((&mut hc as *mut HIGHCONTRASTW).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok()
            && hc.dwFlags.0 & HCF_HIGHCONTRASTON.0 != 0
        {
            return false;
        }
        let mut enabled = 1u32;
        let mut bytes = std::mem::size_of::<u32>() as u32;
        let status = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("EnableTransparency"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut enabled as *mut u32).cast()),
            Some(&mut bytes),
        );
        !(status.is_ok() && bytes == std::mem::size_of::<u32>() as u32 && enabled == 0)
    }
}

unsafe fn set_acrylic_composition(window: HWND, enable: bool) -> bool {
    type SetWindowCompositionAttributeFn =
        unsafe extern "system" fn(HWND, *mut WindowCompositionAttribData) -> BOOL;
    // SAFETY: user32.dll is permanently resident in GUI processes and stack buffers outlive the call.
    unsafe {
        let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else {
            return false;
        };
        let Some(proc_addr) = GetProcAddress(user32, s!("SetWindowCompositionAttribute")) else {
            return false;
        };
        let func: SetWindowCompositionAttributeFn = std::mem::transmute(proc_addr);
        let mut policy = AccentPolicy {
            accent_state: if enable { 4 } else { 0 },
            accent_flags: 2,
            gradient_color: if enable { ACRYLIC_TINT_COLOR } else { 0 },
            animation_id: 0,
        };
        let mut data = WindowCompositionAttribData {
            attrib: 0x13,
            pv_data: (&mut policy as *mut AccentPolicy).cast(),
            cb_data: std::mem::size_of::<AccentPolicy>(),
        };
        func(window, &mut data).as_bool()
    }
}

unsafe fn sync_search_alpha(parent: HWND, search: HWND) {
    if search.0.is_null() {
        return;
    }
    let alpha = if !parent.0.is_null() && glass_enabled(parent) {
        SEARCH_ALPHA
    } else {
        255
    };
    unsafe {
        let _ = SetLayeredWindowAttributes(search, COLORREF(0), alpha, LWA_ALPHA);
    }
}

unsafe fn apply_accent_acrylic(window: HWND) -> bool {
    use windows::Win32::{Graphics::Dwm::*, UI::Controls::MARGINS};
    // SAFETY: window is a live HWND; DWM calls and transient blur region stay on UI thread.
    unsafe {
        let auto = DWMSBT_AUTO.0;
        // Reset Win11 SystemBackdrop (DWMSBT_AUTO) and client frame margins to {0} so DWM does not
        // replace Acrylic with solid active black or inactive #2B2B2B grey; enable per-pixel client
        // alpha via DwmEnableBlurBehindWindow + SetWindowCompositionAttribute(ACCENT_ENABLE_ACRYLICBLURBEHIND).
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&auto as *const i32).cast(),
            std::mem::size_of_val(&auto) as u32,
        );
        let _ = DwmExtendFrameIntoClientArea(window, &MARGINS::default());
        let region = CreateRectRgn(0, 0, -1, -1);
        let blur = DWM_BLURBEHIND {
            dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION | DWM_BB_TRANSITIONONMAXIMIZED,
            fEnable: true.into(),
            hRgnBlur: region,
            fTransitionOnMaximized: true.into(),
        };
        let blur_ok = DwmEnableBlurBehindWindow(window, &blur).is_ok();
        if !region.0.is_null() {
            let _ = DeleteObject(region);
        }
        blur_ok && set_acrylic_composition(window, true)
    }
}

unsafe fn apply_hybrid_acrylic(window: HWND) -> bool {
    use windows::Win32::Graphics::Dwm::*;
    // SAFETY: window is a live HWND; falls back to DWMSBT_TRANSIENTWINDOW only if AccentPolicy fails.
    unsafe {
        if apply_accent_acrylic(window) {
            return true;
        }
        let acrylic = DWMSBT_TRANSIENTWINDOW.0;
        DwmSetWindowAttribute(
            window,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&acrylic as *const i32).cast(),
            std::mem::size_of_val(&acrylic) as u32,
        )
        .is_ok()
    }
}

pub fn begin_backdrop_transition(window: HWND) {
    use windows::Win32::{Graphics::Dwm::*, UI::Controls::MARGINS};
    if window.0.is_null() {
        return;
    }
    // SAFETY: switches DWM to native DWMSBT_TRANSIENTWINDOW for minimize/restore transition while preserving GLASS_PROPERTY.
    unsafe {
        if !glass_enabled(window) || !user_acrylic_enabled() || !transparency_enabled() {
            return;
        }
        let _ = set_acrylic_composition(window, false);
        let blur = DWM_BLURBEHIND {
            dwFlags: DWM_BB_ENABLE,
            fEnable: false.into(),
            ..Default::default()
        };
        let _ = DwmEnableBlurBehindWindow(window, &blur);
        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        let _ = DwmExtendFrameIntoClientArea(window, &margins);
        let acrylic = DWMSBT_TRANSIENTWINDOW.0;
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&acrylic as *const i32).cast(),
            std::mem::size_of_val(&acrylic) as u32,
        );
    }
}

pub fn finish_backdrop_transition(window: HWND) {
    if window.0.is_null() {
        return;
    }
    // SAFETY: restores steady-state ACCENT_ENABLE_ACRYLICBLURBEHIND once restore animation finishes.
    unsafe {
        if IsIconic(window).as_bool() {
            return;
        }
        if glass_enabled(window) && user_acrylic_enabled() && transparency_enabled() {
            if apply_accent_acrylic(window) {
                if let Ok(search) = GetDlgItem(window, SEARCH as i32) {
                    sync_search_alpha(window, search);
                }
            } else {
                let _ = enable_backdrop(window);
            }
        } else {
            let _ = enable_backdrop(window);
        }
    }
}

pub fn enable_backdrop(window: HWND) -> bool {
    use windows::Win32::Graphics::Dwm::*;
    if window.0.is_null() {
        return false;
    }
    // SAFETY: all attributes, composition flags, and properties belong to this window HWND.
    unsafe {
        let dark = 1i32;
        let round = DWMWCP_ROUND.0;
        let allow_transitions = 0i32;
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
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            (&allow_transitions as *const i32).cast(),
            std::mem::size_of_val(&allow_transitions) as u32,
        );
        let can_glass = PAINT_RUNTIME.with(|runtime| runtime.initialized)
            && user_acrylic_enabled()
            && transparency_enabled()
            && DwmIsCompositionEnabled().is_ok_and(|value| value.as_bool());
        if !can_glass {
            disable_backdrop(window);
            return false;
        }
        if glass_enabled(window) {
            let _ = apply_hybrid_acrylic(window);
            if let Ok(search) = GetDlgItem(window, SEARCH as i32) {
                sync_search_alpha(window, search);
            }
            return true;
        }
        let backdrop_ok = apply_hybrid_acrylic(window);
        let enabled = backdrop_ok
            && SetPropW(
                window,
                GLASS_PROPERTY,
                windows::Win32::Foundation::HANDLE(window.0),
            )
            .is_ok();
        if !enabled {
            disable_backdrop(window);
            return false;
        }
        if let Ok(search) = GetDlgItem(window, SEARCH as i32) {
            sync_search_alpha(window, search);
        }
        true
    }
}

pub fn disable_backdrop(window: HWND) {
    use windows::Win32::{Graphics::Dwm::*, UI::Controls::MARGINS};
    unsafe {
        let was_enabled = glass_enabled(window);
        let _ = RemovePropW(window, GLASS_PROPERTY);
        let _ = set_acrylic_composition(window, false);
        let blur = DWM_BLURBEHIND {
            dwFlags: DWM_BB_ENABLE,
            fEnable: false.into(),
            ..Default::default()
        };
        let _ = DwmEnableBlurBehindWindow(window, &blur);
        let none = DWMSBT_NONE.0;
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&none as *const i32).cast(),
            std::mem::size_of_val(&none) as u32,
        );
        let allow_transitions = 0i32;
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            (&allow_transitions as *const i32).cast(),
            std::mem::size_of_val(&allow_transitions) as u32,
        );
        let _ = DwmExtendFrameIntoClientArea(window, &MARGINS::default());
        if let Ok(search) = GetDlgItem(window, SEARCH as i32) {
            sync_search_alpha(window, search);
        }
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
            let rendered = paint_scene(window, buffer.dc, Some(&buffer), glass && theme.0 != 0);
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
        paint_scene(window, target, None, false);
    }
}

unsafe fn paint_scene(
    window: HWND,
    dc: HDC,
    buffer: Option<&PaintBuffer>,
    glass_active: bool,
) -> bool {
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
        let (has_transition, covers_body) = ANIM.with(|a| {
            a.borrow()
                .transition
                .as_ref()
                .map_or((false, false), |t| (true, t.covers_body))
        });
        let divider_px = scale(1).max(1);
        let divider_color = if glass_active { GLASS_RIM } else { LINE };
        let top_divider = rect(0, 0, scale(layout.client.right), divider_px);
        let header_divider = rect(
            0,
            scale(layout.header.bottom) - divider_px,
            scale(layout.client.right),
            scale(layout.header.bottom),
        );
        let caption_divider = rect(
            scale(layout.caption_divider.left),
            scale(layout.caption_divider.top),
            scale(layout.caption_divider.left) + divider_px,
            scale(layout.caption_divider.bottom),
        );
        let footer_divider = rect(
            0,
            scale(layout.footer.top),
            scale(layout.client.right),
            scale(layout.footer.top) + divider_px,
        );
        let side_divider = rect(
            scale(layout.sidebar.left),
            scale(layout.header.bottom),
            scale(layout.sidebar.left) + divider_px,
            scale(layout.footer.top),
        );
        let h_divider = |r: Rect| {
            rect(
                scale(r.left),
                scale(r.top),
                scale(r.right),
                scale(r.top) + divider_px,
            )
        };
        let side_inner_divider = layout.sidebar_divider.map(h_divider);
        let prefs_divider = layout.settings_dividers[0].map(h_divider);
        let resources_divider = layout.settings_dividers[1].map(h_divider);
        let sources_divider = layout.sources_divider.map(h_divider);
        let (caption_hot, caption_down) = ANIM.with(|a| {
            let state = a.borrow();
            (state.caption_hot, state.caption_down)
        });
        let caption_style = |btn: CaptionButton| -> Option<(u32, u32, u8)> {
            let down = caption_down == Some(btn);
            let hot = caption_hot == Some(btn);
            if !down && !hot {
                return None;
            }
            if btn == CaptionButton::Close {
                Some(if down {
                    (0xa82215, 0xd14b3c, 240)
                } else {
                    (0xc42b1c, 0xe05848, 224)
                })
            } else {
                Some(if down {
                    (CARD, HIGHLIGHT, ACTIVE_TILE_ALPHA)
                } else {
                    (CARD_LIT, GLASS_RIM, HOVER_ALPHA)
                })
            }
        };

        fill(dc, pixels(layout.client), BODY);
        fill(dc, pixels(layout.header), BODY);
        fill(dc, top_divider, divider_color);
        fill(dc, header_divider, divider_color);
        fill(dc, caption_divider, divider_color);
        for &(btn, area) in &layout.caption_buttons {
            if let Some((bg, border, _)) = caption_style(btn) {
                raised_rounded(dc, pixels(area), bg, border, border, scale(8));
            }
        }
        if !covers_body {
            match tab {
                Tab::Library => {
                    fill(dc, pixels(layout.sidebar), CARD);
                    fill(dc, side_divider, divider_color);
                    if let Some(div) = side_inner_divider {
                        fill(dc, div, divider_color);
                    }
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
                    if let Some(div) = prefs_divider {
                        fill(dc, div, divider_color);
                    }
                    if let Some(div) = resources_divider {
                        fill(dc, div, divider_color);
                    }
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
                    if let Some(div) = sources_divider {
                        fill(dc, div, divider_color);
                    }
                }
            }
        }
        fill(dc, pixels(layout.footer), FOOTER);
        fill(dc, footer_divider, divider_color);
        if let Some(buffer) = buffer {
            if glass_active {
                let mut div_ok = buffer.tint_background(top_divider, DIVIDER_ALPHA)
                    && buffer.tint_background(header_divider, DIVIDER_ALPHA)
                    && buffer.tint_background(caption_divider, DIVIDER_ALPHA)
                    && buffer.tint_background(footer_divider, DIVIDER_ALPHA);
                for &(btn, area) in &layout.caption_buttons {
                    if let Some((_, border, alpha)) = caption_style(btn) {
                        div_ok &= buffer.apply_rounded_rect_mask(
                            pixels(area),
                            &RoundedMask {
                                ellipse: scale(8),
                                border,
                                border_width: 1.0,
                                border_alpha: BORDER_ALPHA,
                                fill_alpha: alpha,
                                focus: 0,
                                focus_inset: 0.0,
                                host_color: INK,
                                host_alpha: PANEL_ALPHA,
                                top_rim: GLASS_RIM,
                                bottom_rim: INK,
                                sheen_strength: 0.0,
                            },
                        );
                    }
                }
                let card_mask = RoundedMask {
                    ellipse: scale(12),
                    border: LINE,
                    border_width: 1.0,
                    border_alpha: BORDER_ALPHA,
                    fill_alpha: CARD_ALPHA,
                    focus: 0,
                    focus_inset: 0.0,
                    host_color: INK,
                    host_alpha: PANEL_ALPHA,
                    top_rim: GLASS_RIM,
                    bottom_rim: INK,
                    sheen_strength: 0.0,
                };
                let panel_ok = if covers_body {
                    true
                } else {
                    match tab {
                        Tab::Library => {
                            let inner_ok = side_inner_divider
                                .is_none_or(|d| buffer.tint_background(d, DIVIDER_ALPHA));
                            inner_ok
                                && buffer.tint_background(side_divider, DIVIDER_ALPHA)
                                && buffer.tint_background(pixels(layout.sidebar), CARD_ALPHA)
                        }
                        Tab::Settings => {
                            let p_div_ok = prefs_divider
                                .is_none_or(|d| buffer.tint_background(d, DIVIDER_ALPHA));
                            let r_div_ok = resources_divider
                                .is_none_or(|d| buffer.tint_background(d, DIVIDER_ALPHA));
                            p_div_ok
                                && r_div_ok
                                && buffer.apply_rounded_rect_mask(
                                    pixels(layout.settings_preferences_panel),
                                    &card_mask,
                                )
                                && buffer.apply_rounded_rect_mask(
                                    pixels(layout.settings_resources_panel),
                                    &card_mask,
                                )
                        }
                        Tab::Sources => {
                            let s_div_ok = sources_divider
                                .is_none_or(|d| buffer.tint_background(d, DIVIDER_ALPHA));
                            s_div_ok
                                && buffer.apply_rounded_rect_mask(
                                    pixels(layout.sources_panel),
                                    &card_mask,
                                )
                        }
                    }
                };
                // Window canvas, elevated cards, and dividers all transmit the acrylic backdrop at layered alpha levels.
                if !div_ok || !panel_ok || !buffer.tint_background(client, PANEL_ALPHA) {
                    return false;
                }
            }
            if has_transition {
                let comp_ok = ANIM.with(|a| {
                    a.borrow().transition.as_ref().is_none_or(|t| {
                        buffer.composite_transition(t, Instant::now(), glass_active)
                    })
                });
                if !comp_ok {
                    return false;
                }
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
        {
            use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsZoomed};
            let maximized = !window.0.is_null() && IsZoomed(window).as_bool();
            let focused = !window.0.is_null() && GetForegroundWindow() == window;
            for &(btn, area) in &layout.caption_buttons {
                let area_px = pixels(area);
                let active = caption_hot == Some(btn) || caption_down == Some(btn);
                let glyph_color = if active || focused {
                    TEXT_ON_DARK
                } else {
                    MUTED
                };
                if let Some(buffer) = buffer {
                    let _ = buffer.draw_caption_glyph(area_px, btn, maximized, dpi, glyph_color);
                } else {
                    let pen = CreatePen(PS_SOLID, scale(1).max(1), rgb(glyph_color));
                    let old_pen = SelectObject(dc, pen);
                    let cx = (area_px.left + area_px.right) / 2;
                    let cy = (area_px.top + area_px.bottom) / 2;
                    let r = scale(5);
                    match btn {
                        CaptionButton::Minimize => {
                            let _ = MoveToEx(dc, cx - r, cy, None);
                            let _ = LineTo(dc, cx + r, cy);
                        }
                        CaptionButton::Maximize => {
                            let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
                            let _ = Rectangle(dc, cx - r, cy - r, cx + r + 1, cy + r + 1);
                            SelectObject(dc, old_brush);
                        }
                        CaptionButton::Close => {
                            let _ = MoveToEx(dc, cx - r, cy - r, None);
                            let _ = LineTo(dc, cx + r + 1, cy + r + 1);
                            let _ = MoveToEx(dc, cx + r, cy - r, None);
                            let _ = LineTo(dc, cx - r - 1, cy + r + 1);
                        }
                    }
                    SelectObject(dc, old_pen);
                    let _ = DeleteObject(pen);
                }
            }
        }
        if !covers_body {
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
                    if empty_view && !has_transition {
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
            // Native editing, selection and caret repaint outside WM_PAINT; composite this child with frosted alpha.
            let extended = GetWindowLongPtrW(window, GWL_EXSTYLE);
            SetWindowLongPtrW(window, GWL_EXSTYLE, extended | WS_EX_LAYERED.0 as isize);
            let parent = GetParent(window).unwrap_or_default();
            sync_search_alpha(parent, window);
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
        let checkbox = matches!(id, 107 | 108 | 121 | ACRYLIC);
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
                let (surface_color, surface_alpha) = control_surface(id);
                let theme = if glass {
                    OpenThemeData(parent, w!("WINDOW"))
                } else {
                    HTHEME(0)
                };
                let dpi = GetDpiForWindow(window).max(96) as i32;
                let s = |n: i32| n * dpi / 96;
                let mut text_area = area;
                let mut toggle_track = RECT::default();
                let mut toggle_state = (false, false, false, false, 0.0f32);
                let alpha_ok = if checkbox {
                    let state = SendMessageW(window, BM_GETSTATE, WPARAM(0), LPARAM(0)).0;
                    let checked = state & 3 == 1;
                    let pressed = state & 4 != 0;
                    let disabled = GetWindowLongW(window, GWL_STYLE) as u32 & WS_DISABLED.0 != 0;
                    let hot = is_hot(window) && !disabled;
                    let t = toggle_progress(id, checked, parent);
                    toggle_state = (checked, pressed, disabled, hot, t);
                    let glass = theme.0 != 0;
                    if hot || pressed {
                        fill(buffer.dc, area, CARD_LIT);
                        let row_mask = RoundedMask {
                            ellipse: s(8),
                            border: if glass || pressed { HIGHLIGHT } else { LINE },
                            border_width: 1.0,
                            border_alpha: if !glass {
                                255
                            } else if pressed {
                                148
                            } else {
                                116
                            },
                            fill_alpha: if !glass {
                                255
                            } else if pressed {
                                96
                            } else {
                                78
                            },
                            focus: 0,
                            focus_inset: 0.0,
                            host_color: surface_color,
                            host_alpha: if glass { surface_alpha } else { 255 },
                            top_rim: if glass { CREAM } else { GLASS_RIM },
                            bottom_rim: if glass { 0 } else { INK },
                            sheen_strength: if glass { 0.04 } else { 0.02 },
                        };
                        buffer.apply_rounded_rect_mask(area, &row_mask)
                    } else {
                        fill(buffer.dc, area, surface_color);
                        buffer.tint_background(area, if glass { surface_alpha } else { 255 })
                    }
                } else {
                    fill(buffer.dc, area, surface_color);
                    if theme.0 != 0 {
                        buffer.tint_background(area, surface_alpha)
                    } else {
                        true
                    }
                };
                let old_theme = TEXT_THEME.with(|slot| slot.replace(theme));
                let old_buffer = GLASS_BUFFER.with(|slot| slot.replace(buffer.handle));
                if checkbox {
                    let track_h = (s(22).max(16) / 2) * 2;
                    let track_w = s(40).max(28);
                    let top = (area.top + area.bottom - track_h) / 2;
                    let right = (area.right - s(4)).max(area.left + track_w + s(4));
                    toggle_track = rect(right - track_w, top, right, top + track_h);
                    text_area.left = (area.left + s(6)).min(toggle_track.left);
                    text_area.right = (toggle_track.left - s(12)).max(text_area.left);
                }
                let (_, pressed, disabled, hot, toggle_t) = toggle_state;
                label(
                    buffer.dc,
                    &window_text(window),
                    text_area,
                    s(if id == PAGE { 13 } else { 14 }),
                    false,
                    if status_edit {
                        TEXT_ON_DARK
                    } else if checkbox && disabled {
                        DISABLED_TEXT
                    } else if id == 112 || checkbox {
                        TEXT
                    } else {
                        MUTED
                    },
                    if id == PAGE {
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE
                    } else if checkbox {
                        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS
                    } else if id == 112 {
                        DT_SINGLELINE | DT_END_ELLIPSIS
                    } else if status_edit {
                        DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX
                    } else {
                        DT_WORDBREAK
                    },
                );
                if checkbox {
                    let _ = buffer.draw_toggle_switch(
                        toggle_track,
                        toggle_t,
                        pressed,
                        hot,
                        disabled,
                        theme.0 != 0,
                    );
                    let state = SendMessageW(window, BM_GETSTATE, WPARAM(0), LPARAM(0)).0;
                    let hide_focus = !parent.0.is_null()
                        && SendMessageW(parent, WM_QUERYUISTATE, WPARAM(0), LPARAM(0)).0
                            & UISF_HIDEFOCUS_FLAG
                            != 0;
                    if state & 8 != 0 && !hide_focus {
                        let focus_r = rect(
                            area.left + 1,
                            area.top + s(2),
                            area.right - 1,
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
    const WM_MOUSELEAVE: u32 = 0x02A3;
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
            WM_MOUSEMOVE => {
                let ctrl_id = GetDlgCtrlID(window) as usize;
                let interactive = owner_draw || matches!(ctrl_id, 107 | 108 | 121 | ACRYLIC);
                if interactive {
                    let prev_hot = ANIM.with(|a| {
                        let mut state = a.borrow_mut();
                        let prev = state.hot;
                        state.hot = window.0 as isize;
                        prev
                    });
                    if prev_hot != window.0 as isize {
                        let mut tme = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: window,
                            dwHoverTime: 0,
                        };
                        let _ = TrackMouseEvent(&mut tme);
                        if prev_hot != 0 {
                            let old_hwnd = HWND(prev_hot as *mut _);
                            if IsWindow(old_hwnd).as_bool() {
                                let _ = InvalidateRect(old_hwnd, None, false);
                            }
                        }
                        let _ = InvalidateRect(window, None, false);
                    }
                }
                let res = DefSubclassProc(window, message, wparam, lparam);
                if wparam.0 & 1 != 0 && ctrl_id == 113 {
                    let _ = RedrawWindow(window, None, None, RDW_INVALIDATE | RDW_UPDATENOW);
                }
                res
            }
            WM_MOUSELEAVE => {
                let cleared = ANIM.with(|a| {
                    let mut state = a.borrow_mut();
                    if state.hot == window.0 as isize {
                        state.hot = 0;
                        true
                    } else {
                        false
                    }
                });
                if cleared {
                    let _ = InvalidateRect(window, None, false);
                }
                DefSubclassProc(window, message, wparam, lparam)
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
                ANIM.with(|a| {
                    let mut state = a.borrow_mut();
                    if state.hot == window.0 as isize {
                        state.hot = 0;
                    }
                });
                let _ = RemoveWindowSubclass(window, Some(control_proc), id);
                DefSubclassProc(window, message, wparam, lparam)
            }
            _ => DefSubclassProc(window, message, wparam, lparam),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) unsafe fn paint_dropdown_into(
    target: HDC,
    area: RECT,
    items: &[&str],
    checked_index: Option<usize>,
    hover_index: Option<usize>,
    item_h: i32,
    scroll_top: usize,
    reveal_t: f32,
    dpi: i32,
    glass: bool,
) -> bool {
    if area.right <= area.left || area.bottom <= area.top {
        return false;
    }
    let s = |n: i32| n * dpi.max(96) / 96;
    unsafe {
        let Some(buffer) = PaintBuffer::new(target, area) else {
            return false;
        };
        let theme = if glass {
            OpenThemeData(HWND::default(), w!("WINDOW"))
        } else {
            HTHEME(0)
        };
        let old_theme = TEXT_THEME.with(|slot| slot.replace(theme));
        let old_buffer = GLASS_BUFFER.with(|slot| slot.replace(buffer.handle));
        let ok = (|| -> bool {
            fill(buffer.dc, area, CARD);
            let outer_mask = RoundedMask {
                ellipse: s(12),
                border: LINE,
                border_width: 1.0,
                border_alpha: if glass { BORDER_ALPHA } else { 255 },
                fill_alpha: if glass { DROPDOWN_ALPHA } else { 255 },
                focus: 0,
                focus_inset: 0.0,
                host_color: INK,
                host_alpha: if glass { 0 } else { 255 },
                top_rim: GLASS_RIM,
                bottom_rim: INK,
                sheen_strength: 0.03,
            };
            if !buffer.apply_rounded_rect_mask(area, &outer_mask) {
                return false;
            }
            let pad_y = s(6);
            let pad_x = s(6);
            let gutter = if checked_index.is_some() {
                s(26)
            } else {
                s(12)
            };
            let slide_y = ((1.0 - ease_out_cubic(reveal_t)) * (-s(6) as f32)).round() as i32;
            let clip = CreateRectRgn(
                area.left + 1,
                area.top + pad_y,
                area.right - 1,
                (area.bottom - pad_y).max(area.top + pad_y + 1),
            );
            let saved = SaveDC(buffer.dc);
            if !clip.0.is_null() {
                let _ = ExtSelectClipRgn(buffer.dc, clip, RGN_AND);
                let _ = DeleteObject(clip);
            }
            let host_alpha = if glass { DROPDOWN_ALPHA } else { 255 };
            for (idx, text) in items.iter().enumerate().skip(scroll_top) {
                let top = area.top + pad_y + (idx - scroll_top) as i32 * item_h + slide_y;
                if top >= area.bottom - pad_y {
                    break;
                }
                let row = rect(area.left + pad_x, top, area.right - pad_x, top + item_h);
                if row.right <= row.left || row.bottom <= row.top {
                    continue;
                }
                if Some(idx) == hover_index {
                    fill(buffer.dc, row, CARD_LIT);
                    let _ = buffer.clear_alpha(row);
                    let hover_mask = RoundedMask {
                        ellipse: s(8),
                        border: HIGHLIGHT,
                        border_width: 1.0,
                        border_alpha: if glass { 192 } else { 255 },
                        fill_alpha: if glass { 172 } else { 255 },
                        focus: 0,
                        focus_inset: 0.0,
                        host_color: CARD,
                        host_alpha,
                        top_rim: CREAM,
                        bottom_rim: INK,
                        sheen_strength: 0.06,
                    };
                    let _ = buffer.apply_rounded_rect_mask(row, &hover_mask);
                } else if Some(idx) == checked_index {
                    fill(buffer.dc, row, ACTIVE_SURFACE);
                    let _ = buffer.clear_alpha(row);
                    let active_mask = RoundedMask {
                        ellipse: s(8),
                        border: ACCENT_DARK,
                        border_width: 1.0,
                        border_alpha: if glass { 116 } else { 200 },
                        fill_alpha: if glass { 88 } else { 255 },
                        focus: 0,
                        focus_inset: 0.0,
                        host_color: CARD,
                        host_alpha,
                        top_rim: ACCENT_SOFT,
                        bottom_rim: INK,
                        sheen_strength: 0.02,
                    };
                    let _ = buffer.apply_rounded_rect_mask(row, &active_mask);
                }
                if Some(idx) == checked_index {
                    let cy = (row.top + row.bottom) / 2;
                    let cx = row.left + gutter / 2;
                    let u = (gutter / 6).max(3);
                    let stroke = (gutter / 13).max(2);
                    let pen = CreatePen(PS_SOLID, stroke, rgb(ACCENT));
                    let old_pen = SelectObject(buffer.dc, pen);
                    let _ = MoveToEx(buffer.dc, cx - u, cy, None);
                    let _ = LineTo(buffer.dc, cx - u / 4, cy + u * 3 / 4);
                    let _ = LineTo(buffer.dc, cx + u * 5 / 4, cy - u * 3 / 4);
                    SelectObject(buffer.dc, old_pen);
                    let _ = DeleteObject(pen);
                    let check_box = rect(cx - u - 2, cy - u - 2, cx + u * 2, cy + u * 2);
                    let _ = buffer.tint_background(check_box, 255);
                }
                let text_r = rect(row.left + gutter, row.top, row.right - s(10), row.bottom);
                label(
                    buffer.dc,
                    text,
                    text_r,
                    s(14),
                    Some(idx) == checked_index,
                    TEXT,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
                );
            }
            let _ = RestoreDC(buffer.dc, saved);
            true
        })();
        TEXT_THEME.with(|slot| slot.set(old_theme));
        GLASS_BUFFER.with(|slot| slot.set(old_buffer));
        if theme.0 != 0 {
            let _ = CloseThemeData(theme);
        }
        ok && buffer.finish()
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
        if matches!(id, 113 | SEARCH) {
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
            (TEXT, 0x6a6a6a),
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
        assert_eq!(control_surface(TAB_LIBRARY), (INK, PANEL_ALPHA));
        assert_eq!(control_surface(PREVIOUS), (INK, PANEL_ALPHA));
        assert_eq!(control_surface(PAGE), (INK, PANEL_ALPHA));
        assert_eq!(control_surface(113), (FOOTER, PANEL_ALPHA));
        assert_eq!(control_surface(PRODUCT_WEBSITE), (CARD, CARD_ALPHA));
        assert_eq!(control_surface(VERSION), (CARD, CARD_ALPHA));

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
                    border_alpha: BORDER_ALPHA,
                    fill_alpha: CARD_ALPHA,
                    focus: 0,
                    focus_inset: 0.0,
                    host_color: INK,
                    host_alpha: PANEL_ALPHA,
                    top_rim: GLASS_RIM,
                    bottom_rim: INK,
                    sheen_strength: 0.0,
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
            let bg = premul_channel(17, PANEL_ALPHA);
            let card_b = premul_channel((CARD & 255) as u8, CARD_ALPHA);
            let card_g = premul_channel(((CARD >> 8) & 255) as u8, CARD_ALPHA);
            let card_r = premul_channel(((CARD >> 16) & 255) as u8, CARD_ALPHA);
            assert_eq!(px(4, 4), [bg, bg, bg, PANEL_ALPHA]);
            assert_eq!(
                px(10, 10),
                [bg, bg, bg, PANEL_ALPHA],
                "rounded panel corner outside arc must match acrylic canvas"
            );
            assert_eq!(
                px(10, 40)[3],
                BORDER_ALPHA,
                "panel border must use frosted border alpha"
            );
            let center = px(40, 40);
            assert_eq!(
                center,
                [card_b, card_g, card_r, CARD_ALPHA],
                "panel interior must use frosted Card acrylic alpha"
            );

            // Overlay rounded glyph over frosted Card must promote only painted interior pixels.
            std::slice::from_raw_parts_mut(bits as *mut u32, 80 * 80).fill(0);
            let overlay = PaintBuffer::new(dc, full).expect("overlay buffer");
            fill(overlay.dc, full, CARD);
            assert!(overlay.tint_background(full, CARD_ALPHA));
            let glyph = rect(30, 30, 50, 50);
            rounded(overlay.dc, glyph, ACCENT, ACCENT_DARK, 8);
            assert!(overlay.tint_background(glyph, 255));
            assert!(overlay.finish());
            let _ = GdiFlush();
            assert_eq!(
                px(30, 30),
                [card_b, card_g, card_r, CARD_ALPHA],
                "rounded overlay corner must preserve translucent card alpha"
            );
            assert_eq!(px(40, 40)[3], 255, "overlay interior must be opaque");
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
            assert_eq!(area(107).left, area(ACRYLIC).left);
            assert!(area(ACRYLIC).top > area(108).bottom);
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

    #[test]
    fn toggle_switch_3d_pill_and_glass_dropdown_preserve_premultiplied_alpha() {
        // SAFETY: test-owned memory DIB sections never touch any desktop window.
        unsafe {
            let create_dib = |w: i32, h: i32| {
                let dc = CreateCompatibleDC(None);
                let mut bits = std::ptr::null_mut();
                let bitmap = CreateDIBSection(
                    dc,
                    &BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
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
                )
                .expect("test DIB");
                let prev = SelectObject(dc, bitmap);
                (dc, bitmap, prev, bits)
            };

            // Verify 3D pill toggle draws Cream spherical knob on left when OFF (t=0) and Orange track on right when ON (t=1).
            let (dc, bitmap, prev, bits) = create_dib(60, 32);
            std::slice::from_raw_parts_mut(bits as *mut u32, 60 * 32).fill(0);
            let full = rect(0, 0, 60, 32);
            let track = rect(10, 5, 50, 27);
            let buf_off = PaintBuffer::new(dc, full).expect("toggle off buffer");
            fill(buf_off.dc, full, CARD);
            assert!(buf_off.tint_background(full, CARD_ALPHA));
            assert!(buf_off.draw_toggle_switch(track, 0.0, false, false, false, true));
            assert!(buf_off.finish());
            let _ = GdiFlush();
            let px_off = std::slice::from_raw_parts(bits as *const u8, 60 * 32 * 4);
            let at = |buf: &[u8], w: usize, x: usize, y: usize| -> [u8; 4] {
                let i = (y * w + x) * 4;
                buf[i..i + 4].try_into().unwrap()
            };
            // Outside pill stays CARD frosted backdrop; left knob interior is opaque Cream-lit (alpha 255); right track stays frosted.
            assert_eq!(at(px_off, 60, 2, 2)[3], CARD_ALPHA);
            assert_eq!(at(px_off, 60, 21, 16)[3], 255);
            assert!(at(px_off, 60, 21, 16)[2] > 200);
            assert!(at(px_off, 60, 42, 16)[3] >= 120 && at(px_off, 60, 42, 16)[3] < 200);

            std::slice::from_raw_parts_mut(bits as *mut u32, 60 * 32).fill(0);
            let buf_on = PaintBuffer::new(dc, full).expect("toggle on buffer");
            fill(buf_on.dc, full, CARD);
            assert!(buf_on.tint_background(full, CARD_ALPHA));
            assert!(buf_on.draw_toggle_switch(track, 1.0, false, false, false, true));
            assert!(buf_on.finish());
            let _ = GdiFlush();
            let px_on = std::slice::from_raw_parts(bits as *const u8, 60 * 32 * 4);
            assert_eq!(at(px_on, 60, 39, 16)[3], 255);
            // Left track when ON uses warm ORANGE channel (red dominant).
            let tr = at(px_on, 60, 18, 16);
            assert!(tr[2] > tr[0] + 40 && tr[3] > 200);
            assert!(
                px_on
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|p| p[..3].iter().all(|c| *c <= p[3]))
            );
            std::slice::from_raw_parts_mut(bits as *mut u32, 60 * 32).fill(0);
            let buf_opaque = PaintBuffer::new(dc, full).expect("opaque toggle buffer");
            fill(buf_opaque.dc, full, CARD);
            assert!(buf_opaque.draw_toggle_switch(track, 1.0, false, false, false, false));
            assert!(buf_opaque.opaque());
            assert!(buf_opaque.finish());
            let _ = GdiFlush();
            let px_opaque = std::slice::from_raw_parts(bits as *const u8, 60 * 32 * 4);
            assert_eq!(
                at(px_opaque, 60, 11, 6),
                [
                    (CARD & 255) as u8,
                    ((CARD >> 8) & 255) as u8,
                    ((CARD >> 16) & 255) as u8,
                    255,
                ]
            );
            SelectObject(dc, prev);
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(dc);

            // Verify frosted glass dropdown keeps transparent corners, hover highlight pill, and premultiplied pixels.
            let (mdc, mbmp, mprev, mbits) = create_dib(220, 80);
            std::slice::from_raw_parts_mut(mbits as *mut u32, 220 * 80).fill(0);
            let items = ["All folders", "Live wallpapers"];
            assert!(paint_dropdown_into(
                mdc,
                rect(0, 0, 220, 80),
                &items,
                Some(0),
                Some(1),
                32,
                0,
                1.0,
                96,
                true,
            ));
            let _ = GdiFlush();
            let mpx = std::slice::from_raw_parts(mbits as *const u8, 220 * 80 * 4);
            assert_eq!(at(mpx, 220, 0, 0), [0, 0, 0, 0]);
            assert_eq!(at(mpx, 220, 110, 3)[3], DROPDOWN_ALPHA);
            assert_eq!(at(mpx, 220, 110, 50)[3], 172);
            assert!(
                mpx.as_chunks::<4>()
                    .0
                    .iter()
                    .all(|p| p[..3].iter().all(|c| *c <= p[3]))
            );
            SelectObject(mdc, mprev);
            let _ = DeleteObject(mbmp);
            let _ = DeleteDC(mdc);
        }
    }

    #[test]
    fn thumbnail_cache_persists_across_pages_and_evicts_lru_preserving_active() {
        clear();
        let make_bitmap = || unsafe { CreateBitmap(1, 1, 1, 32, None) };
        let path = |idx: usize| PathBuf::from(format!(r"C:\wallpapers\clip-{idx:03}.mp4"));
        let paths: Vec<_> = (0..MAX_CACHED_THUMBNAILS + 2).map(path).collect();
        for p in &paths[..MAX_CACHED_THUMBNAILS] {
            let bmp = make_bitmap();
            assert!(!bmp.0.is_null());
            thumbnail(p.clone(), bmp);
        }
        selected(Some(paths[0].clone()));
        tiles(&[paths[1].clone(), paths[2].clone(), paths[10].clone()]);

        let extra = make_bitmap();
        assert!(!extra.0.is_null());
        thumbnail(paths[MAX_CACHED_THUMBNAILS].clone(), extra);

        assert!(has_thumbnail(&paths[0]));
        assert!(has_thumbnail(&paths[1]));
        assert!(has_thumbnail(&paths[2]));
        assert!(has_thumbnail(&paths[MAX_CACHED_THUMBNAILS]));
        assert!(!has_thumbnail(&paths[3]));

        // Paging to another set and retaining the library must preserve off-page cached items.
        tiles(&[paths[10].clone()]);
        retain_thumbnails(&paths);
        assert!(has_thumbnail(&paths[1]));
        assert!(has_thumbnail(&paths[10]));

        // Hiding to tray clears all cached thumbnails.
        retain_thumbnails(&[]);
        assert!(!has_thumbnail(&paths[0]));
        assert!(!has_thumbnail(&paths[MAX_CACHED_THUMBNAILS]));
        clear();
    }

    #[test]
    fn layout_dividers_fit_inside_hosts_without_overlapping_controls() {
        for (width, height) in [(1080, 740), (920, 640)] {
            let layout = Layout::new(width, height);
            let inside = |inner: Rect, outer: Rect| {
                inner.left >= outer.left
                    && inner.right <= outer.right
                    && inner.top >= outer.top
                    && inner.bottom <= outer.bottom
            };
            let sidebar_div = layout.sidebar_divider.expect("sidebar divider");
            assert!(inside(sidebar_div, layout.sidebar));
            for (id, area) in &layout.controls {
                if visible_on(Tab::Library, *id) {
                    assert!(!overlap(sidebar_div, *area));
                }
            }
            let pref_div = layout.settings_dividers[0].expect("settings prefs divider");
            let res_div = layout.settings_dividers[1].expect("settings resources divider");
            assert!(inside(pref_div, layout.settings_preferences_panel));
            assert!(inside(res_div, layout.settings_resources_panel));
            for (id, area) in &layout.controls {
                if visible_on(Tab::Settings, *id) {
                    assert!(!overlap(pref_div, *area));
                    assert!(!overlap(res_div, *area));
                }
            }
            let src_div = layout.sources_divider.expect("sources divider");
            assert!(inside(src_div, layout.sources_panel));
            for (id, area) in &layout.controls {
                if visible_on(Tab::Sources, *id) {
                    assert!(!overlap(src_div, *area));
                }
            }
        }
    }

    #[test]
    fn animations_forced_for_windows_and_transparency_query_is_safe() {
        assert!(!animations_enabled(HWND::default()));
        let _ = transparency_enabled();
    }

    #[test]
    fn custom_caption_controls_keep_drag_gap_and_non_overlapping_bounds() {
        for (width, height) in [(MIN_CLIENT_WIDTH, MIN_CLIENT_HEIGHT), (1080, 740)] {
            let layout = Layout::new(width, height);
            assert!(layout.header.covers(layout.caption_divider));
            let add_folder = layout.control(ADD_FOLDER).unwrap();
            let add_wallpaper = layout.control(101).unwrap();
            let settings_tab = layout.control(TAB_SETTINGS).unwrap();
            assert!(add_wallpaper.left - settings_tab.right >= 100);
            assert!(layout.caption_divider.left > add_folder.right);
            assert!(layout.caption_divider.right <= layout.caption_buttons[0].1.left);

            for i in 0..layout.caption_buttons.len() {
                let (_, area) = layout.caption_buttons[i];
                assert!(layout.header.covers(area));
                assert!(!overlap(area, layout.caption_divider));
                assert!(!overlap(area, add_folder));
                assert!(!overlap(area, add_wallpaper));
                assert!(!overlap(area, settings_tab));
                if i + 1 < layout.caption_buttons.len() {
                    let (_, next) = layout.caption_buttons[i + 1];
                    assert!(area.right <= next.left);
                    assert!(!overlap(area, next));
                }
            }
        }
    }

    #[test]
    fn anim_interval_ms_stays_within_vsync_clamp_bounds() {
        let interval = anim_interval_ms();
        assert!(
            (6..=16).contains(&interval),
            "expected vsync animation interval in 6..=16 ms, got {interval}"
        );
    }

    #[test]
    fn caption_glyph_rendering_preserves_premultiplied_alpha() {
        // SAFETY: test-owned memory DIB section never touches any desktop window.
        unsafe {
            let w = 36i32;
            let h = 28i32;
            let dc = CreateCompatibleDC(None);
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(
                dc,
                &BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
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
            )
            .expect("caption glyph DIB");
            let prev = SelectObject(dc, bitmap);
            let full = rect(0, 0, w, h);

            for (btn, maximized) in [
                (CaptionButton::Minimize, false),
                (CaptionButton::Maximize, false),
                (CaptionButton::Maximize, true),
                (CaptionButton::Close, false),
            ] {
                std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize).fill(0);
                let buffer = PaintBuffer::new(dc, full).expect("caption paint buffer");
                fill(buffer.dc, full, CARD);
                assert!(buffer.tint_background(full, CARD_ALPHA));
                assert!(buffer.draw_caption_glyph(full, btn, maximized, 96, TEXT_ON_DARK));
                assert!(buffer.finish());
                let _ = GdiFlush();
                let pixels = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
                let at = |x: usize, y: usize| -> [u8; 4] {
                    let idx = (y * w as usize + x) * 4;
                    pixels[idx..idx + 4].try_into().unwrap()
                };
                assert_eq!(at(0, 0)[3], CARD_ALPHA);
                match btn {
                    CaptionButton::Minimize => {
                        let a_lo = at(w as usize / 2, h as usize / 2)[3];
                        let a_hi = at(w as usize / 2, h as usize / 2 - 1)[3];
                        assert!(a_lo >= 180 && a_lo == a_hi);
                    }
                    CaptionButton::Maximize => {
                        assert_eq!(at(w as usize / 2, h as usize / 2)[3], CARD_ALPHA);
                    }
                    CaptionButton::Close => {
                        assert!(at(w as usize / 2, h as usize / 2)[3] > 200);
                    }
                }
                assert!(
                    pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]),
                    "non-premultiplied caption glyph pixel detected for {btn:?}"
                );
            }

            SelectObject(dc, prev);
            let _ = DeleteObject(bitmap);
            let _ = DeleteDC(dc);
        }
    }

    #[test]
    fn backdrop_transition_handles_null_hwnd_safely() {
        begin_backdrop_transition(HWND::default());
        finish_backdrop_transition(HWND::default());
        assert!(!glass_enabled(HWND::default()));
    }

    #[test]
    fn search_control_print_opaque_bkmode_and_end_transition_show() {
        unsafe extern "system" fn test_ctlcolor_subclass(
            window: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
            _id: usize,
            _data: usize,
        ) -> LRESULT {
            if matches!(message, WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC)
                && let Some(handled) = unsafe {
                    control_colors(message, HDC(wparam.0 as *mut _), HWND(lparam.0 as *mut _))
                }
            {
                return handled;
            }
            unsafe { DefSubclassProc(window, message, wparam, lparam) }
        }

        // SAFETY: hidden WS_POPUP parent and child EDIT live only on this thread and are destroyed before returning.
        unsafe {
            let parent = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                WS_POPUP,
                0,
                0,
                240,
                80,
                None,
                None,
                None,
                None,
            )
            .expect("test parent");
            assert!(SetWindowSubclass(parent, Some(test_ctlcolor_subclass), 991, 0).as_bool());
            let search = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                16,
                16,
                180,
                32,
                parent,
                HMENU(SEARCH as *mut _),
                None,
                None,
            )
            .expect("test search edit");
            prepare_control(search).expect("prepare search control");

            let dc = CreateCompatibleDC(None);
            let _ = SetBkMode(dc, TRANSPARENT);
            assert!(control_colors(WM_CTLCOLOREDIT, dc, search).is_some());
            assert_eq!(GetBkColor(dc), rgb(CARD));
            assert_ne!(SetBkMode(dc, TRANSPARENT), TRANSPARENT.0 as i32);
            let _ = DeleteDC(dc);

            let layer = capture_region_controls(parent, rect(0, 0, 240, 80), &[(SEARCH, search)])
                .expect("captured search layer");
            let inside = layer.pixels[(32 * 240 + 150) as usize];
            assert_eq!(inside, (255u32 << 24) | CARD);

            assert_eq!(GetWindowLongW(search, GWL_STYLE) as u32 & WS_VISIBLE.0, 0);
            end_transition(
                parent,
                TransitionAnim {
                    clip: rect(0, 0, 240, 80),
                    direction: 1,
                    start: Instant::now(),
                    duration_ms: 1,
                    old_layer: CapturedLayer {
                        pixels: vec![0; 240 * 80],
                        width: 240,
                        height: 80,
                    },
                    new_layer: CapturedLayer {
                        pixels: vec![0; 240 * 80],
                        width: 240,
                        height: 80,
                    },
                    hidden: vec![search],
                    restore_focus: None,
                    covers_body: false,
                },
            );
            assert_ne!(GetWindowLongW(search, GWL_STYLE) as u32 & WS_VISIBLE.0, 0);

            let child_dc = CreateCompatibleDC(None);
            let mut child_bits = std::ptr::null_mut();
            let child_bmp = CreateDIBSection(
                child_dc,
                &BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: 240,
                        biHeight: -80,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                DIB_RGB_COLORS,
                &mut child_bits,
                None,
                0,
            )
            .expect("child test DIB");
            let prev_bmp = SelectObject(child_dc, child_bmp);
            std::slice::from_raw_parts_mut(child_bits as *mut u32, 240 * 80).fill(0);
            paint_children_into(
                parent,
                child_dc,
                &[(SEARCH, search)],
                None,
                Some((child_bits as *mut u32, 240, 80)),
            );
            let _ = GdiFlush();
            let child_px = std::slice::from_raw_parts(child_bits as *const u32, 240 * 80);
            assert_eq!(child_px[0], 0);
            assert_eq!(child_px[32 * 240 + 150], (255u32 << 24) | CARD);
            SelectObject(child_dc, prev_bmp);
            let _ = DeleteObject(child_bmp);
            let _ = DeleteDC(child_dc);

            let _ = RemoveWindowSubclass(parent, Some(test_ctlcolor_subclass), 991);
            let _ = DestroyWindow(parent);
        }
    }
}
