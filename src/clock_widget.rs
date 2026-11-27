use crate::{
    tray,
    visuals::{
        CLOCK_CTX_24H, CLOCK_CTX_BEHIND_ICONS, CLOCK_CTX_LOCK, CLOCK_CTX_OPEN_SETTINGS,
        CLOCK_CTX_PICK_COLOR, CLOCK_CTX_POS_SAVED, CLOCK_CTX_REMOVE, CLOCK_CTX_SIZE_DELTA,
        CLOCK_REPOSITION,
    },
};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    mem::size_of,
    sync::OnceLock,
};
use windows::{
    Win32::{
        Foundation::{
            BOOL, COLORREF, FreeLibrary, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE,
            SYSTEMTIME, WPARAM,
        },
        Graphics::{
            Dwm::{DWMWA_CLOAKED, DwmFlush, DwmGetWindowAttribute},
            Dxgi::{
                Common::{
                    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
                },
                DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
                DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice,
                IDXGIDevice3, IDXGIFactory2, IDXGISwapChain1,
            },
            Gdi::{
                AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, AddFontMemResourceEx, BI_RGB,
                BITMAPINFO, BITMAPINFOHEADER, BLACK_BRUSH, BLENDFUNCTION, BitBlt,
                CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW,
                DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CENTER, DT_NOPREFIX,
                DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FF_DONTCARE, FW_BOLD,
                GdiFlush, GetDC, GetMonitorInfoW, GetStockObject, HDC, HGDIOBJ,
                MONITOR_DEFAULTTONEAREST, MONITORINFO, MapWindowPoints, MonitorFromPoint,
                MonitorFromRect, MonitorFromWindow, OUT_DEFAULT_PRECIS, ReleaseDC, SRCCOPY,
                ScreenToClient, SelectObject, SetBkMode, SetTextCharacterExtra, SetTextColor,
                TRANSPARENT,
            },
        },
        System::LibraryLoader::{
            GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
        },
        UI::{
            Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
            HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI},
            Input::KeyboardAndMouse::{
                GetCapture, GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_DOWN, VK_ESCAPE,
                VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_UP,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{GUID, HRESULT, IUnknown, Interface, PCSTR, PCWSTR, s, w},
};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLocalTime(lpSystemTime: *mut SYSTEMTIME);
}

unsafe fn local_time() -> SYSTEMTIME {
    let mut st = SYSTEMTIME::default();
    // SAFETY: valid writable stack SYSTEMTIME for kernel32 GetLocalTime.
    unsafe {
        GetLocalTime(&mut st);
    }
    st
}

const CLOCK_CLASS: PCWSTR = w!("WallpaperLive.DayClock");
const PICKER_CLASS: PCWSTR = w!("WallpaperLive.ColorPicker");
const LOUPE_CLASS: PCWSTR = w!("WallpaperLive.ColorLoupe");
const CLOCK_TIMER_ID: usize = 91;
const CLOCK_ZORDER_SETTLE_TIMER_ID: usize = 92;
const CLOCK_ZORDER_LATE_TIMER_ID: usize = 93;
const CTX_LOCAL_DONE_REPOS: usize = 9001;
const CTX_LOCAL_SIZE_UP: usize = 9002;
const CTX_LOCAL_SIZE_DOWN: usize = 9003;

const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
const WINEVENT_SKIPOWNPROCESS: u32 = 0x0002;

static QUICKSAND_OTF: &[u8] = include_bytes!("../assets/Quicksand-Bold.otf");
static FONT_REGISTERED: OnceLock<bool> = OnceLock::new();

fn ensure_quicksand_loaded() {
    let _ = FONT_REGISTERED.get_or_init(|| {
        let count = 0u32;
        // SAFETY: QUICKSAND_OTF is static process-lifetime bytes; AddFontMemResourceEx registers it cleanly.
        let handle = unsafe {
            AddFontMemResourceEx(
                QUICKSAND_OTF.as_ptr() as *const c_void,
                QUICKSAND_OTF.len() as u32,
                None,
                &count,
            )
        };
        !handle.0.is_null()
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockConfig {
    pub enabled: bool,
    pub locked: bool,
    pub behind_icons: bool,
    pub use_24h: bool,
    pub size_pct: u32,
    pub opacity_pct: u32,
    pub color: u32,
    pub pos: Option<(i32, i32)>,
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            locked: false,
            behind_icons: true,
            use_24h: true,
            size_pct: 100,
            opacity_pct: 100,
            color: 0x00FF_FFFF,
            pos: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DesktopLayer {
    parent: HWND,
    icons: HWND,
    raised: bool,
}

#[derive(Default)]
struct DesktopSearch {
    progman_pid: u32,
    worker: HWND,
    icons: HWND,
}

fn belong_to_pid(hwnd: HWND, expected_pid: u32) -> bool {
    if hwnd.0.is_null() || expected_pid == 0 {
        return false;
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    pid == expected_pid
}

fn is_workerw_class(hwnd: HWND) -> bool {
    let mut cls = [0u16; 16];
    let len = unsafe { GetClassNameW(hwnd, &mut cls) };
    len == 7
        && cls[..7]
            == [
                'W' as u16, 'o' as u16, 'r' as u16, 'k' as u16, 'e' as u16, 'r' as u16, 'W' as u16,
            ]
}

unsafe extern "system" fn search_desktop_cb(window: HWND, data: LPARAM) -> BOOL {
    // SAFETY: synchronous EnumWindows reading live Explorer hierarchy.
    unsafe {
        let out = &mut *(data.0 as *mut DesktopSearch);
        if belong_to_pid(window, out.progman_pid)
            && let Ok(icons) = FindWindowExW(window, None, w!("SHELLDLL_DefView"), PCWSTR::null())
        {
            out.icons = icons;
            if let Ok(worker) = FindWindowExW(None, window, w!("WorkerW"), PCWSTR::null()) {
                out.worker = worker;
            }
            return BOOL(0);
        }
    }
    BOOL(1)
}

fn discover_desktop_layer() -> Option<DesktopLayer> {
    // SAFETY: Win32 discovery matching desktop.rs and Rainmeter 24H2+ / legacy hierarchy.
    unsafe {
        let progman = FindWindowW(w!("Progman"), PCWSTR::null()).ok()?;
        let mut progman_pid = 0u32;
        GetWindowThreadProcessId(progman, Some(&mut progman_pid));
        let raised =
            GetWindowLongPtrW(progman, GWL_EXSTYLE) as u32 & WS_EX_NOREDIRECTIONBITMAP.0 != 0;
        let mut msg_res = 0;
        let _ = SendMessageTimeoutW(
            progman,
            0x052c,
            WPARAM(0xD),
            LPARAM(1),
            SMTO_ABORTIFHUNG,
            1000,
            Some(&mut msg_res),
        );
        if raised {
            let icons =
                FindWindowExW(progman, None, w!("SHELLDLL_DefView"), PCWSTR::null()).ok()?;
            if !IsWindow(progman).as_bool() || !IsWindow(icons).as_bool() {
                return None;
            }
            return Some(DesktopLayer {
                parent: progman,
                icons,
                raised: true,
            });
        }
        let mut search = DesktopSearch {
            progman_pid,
            worker: HWND::default(),
            icons: HWND::default(),
        };
        let _ = EnumWindows(
            Some(search_desktop_cb),
            LPARAM((&mut search as *mut DesktopSearch) as isize),
        );
        if search.worker.0.is_null()
            || search.icons.0.is_null()
            || !IsWindow(search.worker).as_bool()
        {
            return None;
        }
        Some(DesktopLayer {
            parent: search.worker,
            icons: search.icons,
            raised: false,
        })
    }
}

fn layer_still_valid(layer: DesktopLayer) -> bool {
    !layer.parent.0.is_null()
        && !layer.icons.0.is_null()
        && unsafe { IsWindow(layer.parent).as_bool() && IsWindow(layer.icons).as_bool() }
}

// --- DirectComposition presenter for Win11 24H2+ WS_CHILD behind SHELLDLL_DefView ---

const IID_IDCOMPOSITION_DEVICE: GUID = GUID::from_u128(0xc37ea93a_e7aa_450d_b16f_9746cb0407f3);
const IID_ID3D11_TEXTURE2D: GUID = GUID::from_u128(0x6f15aaf2_d208_4e89_9ab4_489535d34f9c);
const D3D_DRIVER_TYPE_HARDWARE: i32 = 1;
const D3D_DRIVER_TYPE_WARP: i32 = 5;
const D3D11_CREATE_DEVICE_SINGLETHREADED: u32 = 1;
const D3D11_CREATE_DEVICE_BGRA_SUPPORT: u32 = 32;
const D3D11_SDK_VERSION: u32 = 7;

type D3D11CreateDeviceFn = unsafe extern "system" fn(
    *mut c_void,
    i32,
    HMODULE,
    u32,
    *const u32,
    u32,
    u32,
    *mut *mut c_void,
    *mut u32,
    *mut *mut c_void,
) -> HRESULT;

type DCompositionCreateDeviceFn =
    unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;

struct ComPtr(*mut c_void);

impl ComPtr {
    fn as_raw(&self) -> *mut c_void {
        self.0
    }
    unsafe fn vtable_slot(&self, idx: usize) -> usize {
        unsafe {
            let vtbl = *(self.0 as *const *const usize);
            *vtbl.add(idx)
        }
    }
}

impl Drop for ComPtr {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: owns a valid COM pointer implementing IUnknown (Release at vtable slot 2).
            unsafe {
                let release: unsafe extern "system" fn(*mut c_void) -> u32 =
                    std::mem::transmute(self.vtable_slot(2));
                let _ = release(self.0);
            }
            self.0 = std::ptr::null_mut();
        }
    }
}

struct SystemDCompProcs {
    create_d3d: D3D11CreateDeviceFn,
    create_dcomp: DCompositionCreateDeviceFn,
}

static DCOMP_PROCS: OnceLock<Option<SystemDCompProcs>> = OnceLock::new();

fn load_system_dcomp_procs() -> Option<&'static SystemDCompProcs> {
    DCOMP_PROCS
        .get_or_init(|| {
            // SAFETY: loads OS system DLLs strictly from System32 once for process lifetime; frees on partial failure.
            unsafe {
                let d3d_dll =
                    LoadLibraryExW(w!("d3d11.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
                let Ok(dcomp_dll) =
                    LoadLibraryExW(w!("dcomp.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
                else {
                    let _ = FreeLibrary(d3d_dll);
                    return None;
                };
                let Some(d3d_proc) = GetProcAddress(d3d_dll, s!("D3D11CreateDevice") as PCSTR)
                else {
                    let _ = FreeLibrary(dcomp_dll);
                    let _ = FreeLibrary(d3d_dll);
                    return None;
                };
                let Some(dcomp_proc) =
                    GetProcAddress(dcomp_dll, s!("DCompositionCreateDevice") as PCSTR)
                else {
                    let _ = FreeLibrary(dcomp_dll);
                    let _ = FreeLibrary(d3d_dll);
                    return None;
                };
                Some(SystemDCompProcs {
                    create_d3d: std::mem::transmute::<
                        unsafe extern "system" fn() -> isize,
                        D3D11CreateDeviceFn,
                    >(d3d_proc),
                    create_dcomp: std::mem::transmute::<
                        unsafe extern "system" fn() -> isize,
                        DCompositionCreateDeviceFn,
                    >(dcomp_proc),
                })
            }
        })
        .as_ref()
}

struct DCompPresenter {
    // Drop order: release DComp children (visual, target, device) before swapchain and D3D11 device.
    dcomp_visual: ComPtr,
    dcomp_target: ComPtr,
    dcomp_device: ComPtr,
    swap_chain: IDXGISwapChain1,
    d3d_ctx: ComPtr,
    _d3d_device: IUnknown,
    w: u32,
    h: u32,
}

impl DCompPresenter {
    fn new(hwnd: HWND, w: u32, h: u32) -> Option<Self> {
        let w = w.max(1);
        let h = h.max(1);
        let procs = load_system_dcomp_procs()?;
        let create_d3d = procs.create_d3d;
        let create_dcomp = procs.create_dcomp;
        // SAFETY: uses verified Win32 System32 COM ABI entry points loaded once via OnceLock.
        unsafe {
            let mut dev_raw = std::ptr::null_mut();
            let mut ctx_raw = std::ptr::null_mut();
            let attempts = [
                (
                    D3D_DRIVER_TYPE_HARDWARE,
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_SINGLETHREADED,
                ),
                (D3D_DRIVER_TYPE_HARDWARE, D3D11_CREATE_DEVICE_BGRA_SUPPORT),
                (D3D_DRIVER_TYPE_WARP, D3D11_CREATE_DEVICE_BGRA_SUPPORT),
            ];
            let mut created = false;
            for (driver, flags) in attempts {
                let hr = create_d3d(
                    std::ptr::null_mut(),
                    driver,
                    HMODULE::default(),
                    flags,
                    std::ptr::null(),
                    0,
                    D3D11_SDK_VERSION,
                    &mut dev_raw,
                    std::ptr::null_mut(),
                    &mut ctx_raw,
                );
                if hr.is_ok() && !dev_raw.is_null() && !ctx_raw.is_null() {
                    created = true;
                    break;
                }
                if !ctx_raw.is_null() {
                    drop(ComPtr(ctx_raw));
                    ctx_raw = std::ptr::null_mut();
                }
                if !dev_raw.is_null() {
                    drop(ComPtr(dev_raw));
                    dev_raw = std::ptr::null_mut();
                }
            }
            if !created {
                return None;
            }
            let d3d_device = IUnknown::from_raw(dev_raw);
            let d3d_ctx = ComPtr(ctx_raw);
            let dxgi_device: IDXGIDevice = d3d_device.cast().ok()?;
            let adapter = dxgi_device.GetAdapter().ok()?;
            let factory2: IDXGIFactory2 = adapter.GetParent().ok()?;

            let mut dcomp_dev_raw = std::ptr::null_mut();
            if create_dcomp(
                dxgi_device.as_raw(),
                &IID_IDCOMPOSITION_DEVICE,
                &mut dcomp_dev_raw,
            )
            .is_err()
                || dcomp_dev_raw.is_null()
            {
                return None;
            }
            let dcomp_device = ComPtr(dcomp_dev_raw);

            // IDCompositionDevice::CreateTargetForHwnd = slot 6
            let mut target_raw = std::ptr::null_mut();
            let create_target: unsafe extern "system" fn(
                *mut c_void,
                HWND,
                BOOL,
                *mut *mut c_void,
            ) -> HRESULT = std::mem::transmute(dcomp_device.vtable_slot(6));
            if create_target(dcomp_device.as_raw(), hwnd, BOOL(1), &mut target_raw).is_err()
                || target_raw.is_null()
            {
                return None;
            }
            let dcomp_target = ComPtr(target_raw);

            // IDCompositionDevice::CreateVisual = slot 7
            let mut visual_raw = std::ptr::null_mut();
            let create_visual: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT =
                std::mem::transmute(dcomp_device.vtable_slot(7));
            if create_visual(dcomp_device.as_raw(), &mut visual_raw).is_err()
                || visual_raw.is_null()
            {
                return None;
            }
            let dcomp_visual = ComPtr(visual_raw);

            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: w,
                Height: h,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: BOOL(0),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                Flags: 0,
            };
            let swap_chain = factory2
                .CreateSwapChainForComposition(&dxgi_device, &desc, None)
                .ok()?;

            // IDCompositionVisual::SetContent = slot 15; IDCompositionTarget::SetRoot = slot 3; Commit = slot 3
            let set_content: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT =
                std::mem::transmute(dcomp_visual.vtable_slot(15));
            let set_root: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT =
                std::mem::transmute(dcomp_target.vtable_slot(3));
            let commit: unsafe extern "system" fn(*mut c_void) -> HRESULT =
                std::mem::transmute(dcomp_device.vtable_slot(3));

            if set_content(dcomp_visual.as_raw(), swap_chain.as_raw()).is_err()
                || set_root(dcomp_target.as_raw(), dcomp_visual.as_raw()).is_err()
                || commit(dcomp_device.as_raw()).is_err()
            {
                return None;
            }

            Some(Self {
                _d3d_device: d3d_device,
                d3d_ctx,
                dcomp_device,
                dcomp_target,
                dcomp_visual,
                swap_chain,
                w,
                h,
            })
        }
    }

    fn present_bgra(&mut self, buf: &[u32], w: u32, h: u32) -> bool {
        let w = w.max(1);
        let h = h.max(1);
        if buf.len() < (w as usize).saturating_mul(h as usize) {
            return false;
        }
        // SAFETY: owns D3D11/DXGI/DComp COM objects strictly on the UI thread.
        unsafe {
            if self.w != w || self.h != h {
                if self
                    .swap_chain
                    .ResizeBuffers(2, w, h, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SWAP_CHAIN_FLAG(0))
                    .is_err()
                {
                    return false;
                }
                self.w = w;
                self.h = h;
                let set_content: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT =
                    std::mem::transmute(self.dcomp_visual.vtable_slot(15));
                let _ = set_content(self.dcomp_visual.as_raw(), self.swap_chain.as_raw());
            }
            let Ok(backbuf) = self.swap_chain.GetBuffer::<IUnknown>(0) else {
                return false;
            };
            let mut tex_raw = std::ptr::null_mut();
            if backbuf.query(&IID_ID3D11_TEXTURE2D, &mut tex_raw).is_err() || tex_raw.is_null() {
                return false;
            }
            let _tex_guard = IUnknown::from_raw(tex_raw);

            // ID3D11DeviceContext::UpdateSubresource = slot 48 (7 DeviceChild + 41 Context methods before it).
            let update_subresource: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                u32,
                *const c_void,
                *const c_void,
                u32,
                u32,
            ) = std::mem::transmute(self.d3d_ctx.vtable_slot(48));
            update_subresource(
                self.d3d_ctx.as_raw(),
                tex_raw,
                0,
                std::ptr::null(),
                buf.as_ptr().cast(),
                w * 4,
                0,
            );
            if self.swap_chain.Present(0, DXGI_PRESENT(0)).is_err() {
                return false;
            }
            let set_root: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT =
                std::mem::transmute(self.dcomp_target.vtable_slot(3));
            let commit: unsafe extern "system" fn(*mut c_void) -> HRESULT =
                std::mem::transmute(self.dcomp_device.vtable_slot(3));
            let _ = set_root(self.dcomp_target.as_raw(), self.dcomp_visual.as_raw());
            commit(self.dcomp_device.as_raw()).is_ok()
        }
    }

    fn trim_gpu(&self) {
        // SAFETY: IDXGIDevice3::Trim reclaims temporary D3D11/DXGI staging/command memory on idle ticks.
        if let Ok(dxgi3) = self._d3d_device.cast::<IDXGIDevice3>() {
            unsafe {
                dxgi3.Trim();
            }
        }
    }
}

impl Drop for DCompPresenter {
    fn drop(&mut self) {
        // SAFETY: detaches DComp visual before releasing swapchain/target.
        unsafe {
            let set_content: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT =
                std::mem::transmute(self.dcomp_visual.vtable_slot(15));
            let set_root: unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT =
                std::mem::transmute(self.dcomp_target.vtable_slot(3));
            let commit: unsafe extern "system" fn(*mut c_void) -> HRESULT =
                std::mem::transmute(self.dcomp_device.vtable_slot(3));
            let _ = set_content(self.dcomp_visual.as_raw(), std::ptr::null_mut());
            let _ = set_root(self.dcomp_target.as_raw(), std::ptr::null_mut());
            let _ = commit(self.dcomp_device.as_raw());
        }
    }
}

#[derive(Default)]
struct ClockRuntime {
    hwnd: isize,
    owner: isize,
    action_msg: u32,
    cfg: ClockConfig,
    behind_attached: bool,
    cached_layer: Option<DesktopLayer>,
    dcomp: Option<DCompPresenter>,
    input: Option<ClockInput>,
    repositioning: bool,
    dragging: bool,
    drag_moved: bool,
    drag_start_x: i32,
    drag_start_y: i32,
    drag_offset_x: i32,
    drag_offset_y: i32,
    width: i32,
    height: i32,
    last_stamp: (u16, u16, u16, u16, u16, u16),
    last_bounds: Option<(i32, i32, i32, i32, bool, bool)>,
    pending_wheel_delta: i32,
    hooks: [isize; 2],
}

struct ClockInput {
    hwnd: HWND,
    hook: HWINEVENTHOOK,
    last_w: i32,
    last_h: i32,
    last_holes: Vec<[i32; 4]>,
    cached_shortcuts: Vec<RECT>,
    cached_view: isize,
    shortcuts_dirty: bool,
}
impl Drop for ClockInput {
    fn drop(&mut self) {
        // SAFETY: only this UI thread owns the input-only child and event hook.
        unsafe {
            if !self.hook.0.is_null() {
                let _ = UnhookWinEvent(self.hook);
            }
            if GetCapture() == self.hwnd {
                let _ = ReleaseCapture();
            }
            let _ = KillTimer(self.hwnd, CLOCK_INPUT_REFRESH as usize);
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

const CLOCK_INPUT_CLASS: PCWSTR = w!("WallpaperLive.ClockInput");
const CLOCK_INPUT_REFRESH: u32 = WM_APP + 81;

fn desktop_shortcut_rects(view: HWND) -> Vec<RECT> {
    use windows::Win32::{System::Com::*, UI::Accessibility::*};
    if view.0.is_null() {
        return Vec::new();
    }
    // SAFETY: MSAA marshals Explorer's accessible child bounds; skip any individual item errors.
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let rects = (|| {
            let mut raw = std::ptr::null_mut();
            if AccessibleObjectFromWindow(view, OBJID_CLIENT.0 as u32, &IAccessible::IID, &mut raw)
                .is_err()
                || raw.is_null()
            {
                return Vec::new();
            }
            let acc = IAccessible::from_raw(raw);
            let Ok(count) = acc.accChildCount() else {
                return Vec::new();
            };
            if !(0..=4096).contains(&count) {
                return Vec::new();
            }
            let mut rects = Vec::with_capacity(count as usize);
            for child in 1..=count {
                let (mut x, mut y, mut w, mut h) = (0, 0, 0, 0);
                if acc
                    .accLocation(
                        &mut x,
                        &mut y,
                        &mut w,
                        &mut h,
                        &windows::core::VARIANT::from(child),
                    )
                    .is_ok()
                    && w > 0
                    && h > 0
                {
                    rects.push(RECT {
                        left: x,
                        top: y,
                        right: x.saturating_add(w),
                        bottom: y.saturating_add(h),
                    });
                }
            }
            rects
        })();
        if initialized {
            CoUninitialize();
        }
        rects
    }
}

unsafe extern "system" fn clock_input_event(
    _: HWINEVENTHOOK,
    event: u32,
    event_hwnd: HWND,
    id_object: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    if !matches!(
        event,
        EVENT_OBJECT_CREATE
            | EVENT_OBJECT_DESTROY
            | EVENT_OBJECT_SHOW
            | EVENT_OBJECT_HIDE
            | EVENT_OBJECT_REORDER
            | EVENT_OBJECT_LOCATIONCHANGE
    ) {
        return;
    }
    // Skip high-frequency cursor movement location events (OBJID_CURSOR = -9) so mouse hover stays idle.
    if event == EVENT_OBJECT_LOCATIONCHANGE
        && id_object != OBJID_CLIENT.0
        && id_object != OBJID_WINDOW.0
    {
        return;
    }
    let state = CLOCK_RUNTIME.with(|s| {
        s.borrow()
            .as_ref()
            .filter(|r| !r.dragging)
            .and_then(|r| Some((r.input.as_ref()?.hwnd, r.cached_layer?.icons)))
    });
    if let Some((input, icons)) = state {
        unsafe {
            if event_hwnd == icons || IsChild(icons, event_hwnd).as_bool() {
                CLOCK_RUNTIME.with(|s| {
                    if let Some(inp) = s.borrow_mut().as_mut().and_then(|r| r.input.as_mut()) {
                        inp.shortcuts_dirty = true;
                    }
                });
                let _ = SetTimer(input, CLOCK_INPUT_REFRESH as usize, 80, None);
            }
        }
    }
}

fn sync_clock_input() {
    sync_clock_input_mode(true);
}

fn sync_clock_input_mode(refresh_icons: bool) {
    let state = CLOCK_RUNTIME.with(|s| {
        s.borrow().as_ref().map(|r| {
            (
                r.hwnd,
                r.behind_attached && !r.cfg.locked && !r.repositioning,
                r.cached_layer,
                r.width,
                r.height,
                r.input.as_ref().map(|i| i.hwnd),
                r.dragging,
            )
        })
    });
    let Some((raw, enabled, layer, w, h, input, dragging)) = state else {
        return;
    };
    if dragging && enabled {
        return;
    }
    let layer = layer.filter(|&l| layer_still_valid(l));
    if !enabled || layer.is_none() {
        let old = CLOCK_RUNTIME.with(|s| s.borrow_mut().as_mut().and_then(|r| r.input.take()));
        drop(old);
        return;
    }
    let layer = layer.unwrap();
    // SAFETY: unowned WS_POPUP input handle sits above desktop/below apps so Explorer never buries or blocks it.
    unsafe {
        let mut area = RECT::default();
        if GetWindowRect(HWND(raw as *mut c_void), &mut area).is_err() {
            return;
        }
        let view = FindWindowExW(layer.icons, None, w!("SysListView32"), PCWSTR::null())
            .unwrap_or_default();
        let cached = if !refresh_icons {
            CLOCK_RUNTIME.with(|s| {
                s.borrow()
                    .as_ref()
                    .and_then(|r| r.input.as_ref())
                    .filter(|i| !i.shortcuts_dirty && i.cached_view == view.0 as isize)
                    .map(|i| i.cached_shortcuts.clone())
            })
        } else {
            None
        };
        let (exclusions, refreshed_shortcuts) = match cached {
            Some(rects) => (rects, false),
            None => (desktop_shortcut_rects(view), true),
        };
        let handle = drag_handle_rect(w, h);
        let mut clipped_holes = Vec::new();
        for &icon in &exclusions {
            let left = (icon.left - area.left).max(handle.left);
            let top = (icon.top - area.top).max(handle.top);
            let right = (icon.right - area.left).min(handle.right);
            let bottom = (icon.bottom - area.top).min(handle.bottom);
            if left < right && top < bottom {
                clipped_holes.push([left, top, right, bottom]);
            }
        }
        clipped_holes.sort_unstable();
        clipped_holes.dedup();

        let (hwnd, needs_rgn) = if let Some(input) = input {
            let same_rgn = CLOCK_RUNTIME.with(|s| {
                if let Some(inp) = s.borrow_mut().as_mut().and_then(|r| r.input.as_mut()) {
                    if refreshed_shortcuts {
                        inp.cached_shortcuts = exclusions;
                        inp.cached_view = view.0 as isize;
                        inp.shortcuts_dirty = false;
                    }
                    inp.last_w == w && inp.last_h == h && inp.last_holes == clipped_holes
                } else {
                    false
                }
            });
            (input, !same_rgn)
        } else {
            if ensure_class(
                CLOCK_INPUT_CLASS,
                Some(clock_input_proc),
                HGDIOBJ::default(),
                IDC_SIZEALL,
            )
            .is_err()
            {
                return;
            }
            let Ok(instance) = GetModuleHandleW(None) else {
                return;
            };
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLOCK_INPUT_CLASS,
                w!("WallpaperLive clock drag handle"),
                WS_POPUP,
                0,
                0,
                w,
                h,
                HWND::default(),
                None,
                instance,
                None,
            ) else {
                return;
            };
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw);
            if SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA).is_err() {
                let _ = DestroyWindow(hwnd);
                return;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(layer.icons, Some(&mut pid));
            let hook = SetWinEventHook(
                EVENT_OBJECT_CREATE,
                EVENT_OBJECT_LOCATIONCHANGE,
                None,
                Some(clock_input_event),
                pid,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            );
            CLOCK_RUNTIME.with(|s| {
                if let Some(rt) = s.borrow_mut().as_mut() {
                    rt.input = Some(ClockInput {
                        hwnd,
                        hook,
                        last_w: 0,
                        last_h: 0,
                        last_holes: Vec::new(),
                        cached_shortcuts: exclusions,
                        cached_view: view.0 as isize,
                        shortcuts_dirty: false,
                    });
                }
            });
            (hwnd, true)
        };

        if needs_rgn {
            let region = windows::Win32::Graphics::Gdi::CreateRectRgnIndirect(&handle);
            if region.0.is_null() {
                return;
            }
            for &[left, top, right, bottom] in &clipped_holes {
                let hole = windows::Win32::Graphics::Gdi::CreateRectRgn(left, top, right, bottom);
                if !hole.0.is_null() {
                    windows::Win32::Graphics::Gdi::CombineRgn(
                        region,
                        region,
                        hole,
                        windows::Win32::Graphics::Gdi::RGN_DIFF,
                    );
                    let _ = DeleteObject(HGDIOBJ(hole.0));
                }
            }
            if windows::Win32::Graphics::Gdi::SetWindowRgn(hwnd, region, false) == 0 {
                let _ = DeleteObject(HGDIOBJ(region.0));
                return;
            }
            CLOCK_RUNTIME.with(|s| {
                if let Some(inp) = s.borrow_mut().as_mut().and_then(|r| r.input.as_mut()) {
                    inp.last_w = w;
                    inp.last_h = h;
                    inp.last_holes = clipped_holes;
                }
            });
        }
        place_popup_on_desktop(hwnd, area.left, area.top, w, h, true, false);
    }
}

unsafe extern "system" fn clock_input_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    unsafe {
        let clock = HWND(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut c_void);
        if !IsWindow(clock).as_bool() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        match msg {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_QUERYOPEN => LRESULT(1),
            WM_SYSCOMMAND => {
                let cmd = (wp.0 & 0xFFF0) as u32;
                if cmd == SC_MINIMIZE || cmd == SC_MAXIMIZE {
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_WINDOWPOSCHANGING => {
                if lp.0 != 0 {
                    let pos = &mut *(lp.0 as *mut WINDOWPOS);
                    if !ALLOW_INTERNAL_HIDE.with(Cell::get) {
                        pos.flags.0 &= !SWP_HIDEWINDOW.0;
                    }
                    if !in_zorder_sync() {
                        if (pos.flags.0 & SWP_NOMOVE.0 == 0) && (pos.x <= -30000 || pos.y <= -30000)
                        {
                            pos.flags |= SWP_NOMOVE | SWP_NOSIZE;
                        }
                        if pos.flags.0 & SWP_NOZORDER.0 == 0 {
                            pos.flags |= SWP_NOZORDER;
                        }
                    }
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_NCHITTEST => LRESULT(HTCLIENT as isize),
            WM_LBUTTONDOWN => {
                let _ = windows::Win32::Graphics::Gdi::SetWindowRgn(hwnd, None, false);
                CLOCK_RUNTIME.with(|s| {
                    if let Some(inp) = s.borrow_mut().as_mut().and_then(|r| r.input.as_mut()) {
                        inp.last_w = 0;
                        inp.last_holes.clear();
                    }
                });
                let result = SendMessageW(clock, msg, wp, lp);
                if CLOCK_RUNTIME.with(|s| s.borrow().as_ref().is_some_and(|r| r.dragging)) {
                    let _ = SetCapture(hwnd);
                } else {
                    sync_clock_input();
                }
                result
            }
            WM_MOUSEMOVE | WM_LBUTTONUP | WM_MOUSEWHEEL | WM_RBUTTONUP => {
                let result = SendMessageW(clock, msg, wp, lp);
                if msg == WM_MOUSEMOVE
                    && CLOCK_RUNTIME.with(|s| s.borrow().as_ref().is_some_and(|r| r.dragging))
                {
                    let mut rect = RECT::default();
                    if GetWindowRect(clock, &mut rect).is_ok() {
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            rect.left,
                            rect.top,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                }
                if msg == WM_LBUTTONUP {
                    if GetCapture() == hwnd {
                        let _ = ReleaseCapture();
                    }
                    sync_clock_input();
                }
                result
            }
            WM_CAPTURECHANGED => {
                if CLOCK_RUNTIME.with(|s| s.borrow().as_ref().is_some_and(|r| r.dragging)) {
                    let _ = SendMessageW(clock, WM_LBUTTONUP, WPARAM(0), LPARAM(0));
                    sync_clock_input();
                }
                LRESULT(0)
            }
            WM_TIMER if wp.0 == CLOCK_INPUT_REFRESH as usize => {
                let _ = KillTimer(hwnd, CLOCK_INPUT_REFRESH as usize);
                sync_clock_input();
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
                windows::Win32::Graphics::Gdi::BeginPaint(hwnd, &mut ps);
                let _ = windows::Win32::Graphics::Gdi::EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

#[derive(Default)]
struct ClockTextScratch {
    dc: isize,
    bmp: isize,
    bits: usize,
    w: i32,
    h: i32,
}

impl ClockTextScratch {
    fn free(&mut self) {
        unsafe {
            // Delete DC before the selected bitmap so GDI frees the HBITMAP kernel object cleanly.
            if self.dc != 0 {
                let _ = DeleteDC(HDC(self.dc as *mut c_void));
            }
            if self.bmp != 0 {
                let _ = DeleteObject(HGDIOBJ(self.bmp as *mut c_void));
            }
        }
        *self = Self::default();
    }

    fn ensure(&mut self, w: i32, h: i32) -> Option<(HDC, *mut u32)> {
        if w <= 0 || h <= 0 {
            return None;
        }
        if self.w != w || self.h != h || self.dc == 0 || self.bmp == 0 || self.bits == 0 {
            self.free();
            // SAFETY: owns memory DC and 32bpp DIB section strictly inside UI thread-local scratch.
            unsafe {
                let dc = CreateCompatibleDC(None);
                if dc.0.is_null() {
                    return None;
                }
                let mut bits = std::ptr::null_mut();
                let Ok(bmp) = CreateDIBSection(
                    dc,
                    &BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: size_of::<BITMAPINFOHEADER>() as u32,
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
                ) else {
                    let _ = DeleteDC(dc);
                    return None;
                };
                if bits.is_null() {
                    let _ = DeleteObject(bmp);
                    let _ = DeleteDC(dc);
                    return None;
                }
                let _ = SelectObject(dc, HGDIOBJ(bmp.0));
                self.dc = dc.0 as isize;
                self.bmp = bmp.0 as isize;
                self.bits = bits as usize;
                self.w = w;
                self.h = h;
            }
        }
        Some((HDC(self.dc as *mut c_void), self.bits as *mut u32))
    }
}

#[derive(Default)]
struct ClockFontCache {
    entries: Vec<(i32, isize)>,
}

impl ClockFontCache {
    fn get_or_create(&mut self, height_px: i32) -> isize {
        let key = height_px.max(10);
        if let Some(&(_, handle)) = self.entries.iter().find(|&&(px, _)| px == key) {
            return handle;
        }
        // SAFETY: creates a bold antialiased Quicksand GDI font strictly owned by UI thread cache.
        let font = unsafe {
            CreateFontW(
                -key,
                0,
                0,
                0,
                FW_BOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                ANTIALIASED_QUALITY.0 as u32,
                (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                w!("Quicksand"),
            )
        };
        if font.0.is_null() {
            return 0;
        }
        let raw = font.0 as isize;
        if self.entries.len() >= 6 {
            let (_, evicted) = self.entries.remove(0);
            if evicted != 0 {
                // SAFETY: evicted HFONT is never selected outside synchronous DrawTextW calls.
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(evicted as *mut c_void));
                }
            }
        }
        self.entries.push((key, raw));
        raw
    }

    fn free(&mut self) {
        for (_, raw) in self.entries.drain(..) {
            if raw != 0 {
                // SAFETY: freed only during widget teardown when no DrawTextW is active.
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(raw as *mut c_void));
                }
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct AnuratiCacheKey {
    text: String,
    width: i32,
    height: i32,
    top_bits: u32,
    cap_bits: u32,
    cx_bits: u32,
}

#[derive(Default)]
struct AnuratiWordCache {
    key: Option<AnuratiCacheKey>,
    band_y0: i32,
    band_y1: i32,
    sh_buf: Vec<u8>,
    fg_buf: Vec<u8>,
    edges: Vec<(f32, f32, f32, f32)>,
    crossings: Vec<f32>,
    cov_grid: Vec<u8>,
    hits: Vec<u8>,
}

impl AnuratiWordCache {
    fn free(&mut self) {
        *self = Self::default();
    }
    fn release_build_scratch(&mut self) {
        self.edges = Vec::new();
        self.crossings = Vec::new();
        self.cov_grid = Vec::new();
        self.hits = Vec::new();
    }
}

thread_local! {
    static CLOCK_RUNTIME: RefCell<Option<ClockRuntime>> = const { RefCell::new(None) };
    static CLOCK_SAVED_POS: RefCell<Option<(i32, i32)>> = const { RefCell::new(None) };
    static CLOCK_WHEEL_STEPS: RefCell<i32> = const { RefCell::new(0) };
    static CLOCK_PICKED_COLOR: Cell<Option<u32>> = const { Cell::new(None) };
    static CLOCK_TEXT_SCRATCH: RefCell<ClockTextScratch> = const { RefCell::new(ClockTextScratch {
        dc: 0,
        bmp: 0,
        bits: 0,
        w: 0,
        h: 0,
    }) };
    static CLOCK_CANVAS_SCRATCH: RefCell<ClockTextScratch> = const { RefCell::new(ClockTextScratch {
        dc: 0,
        bmp: 0,
        bits: 0,
        w: 0,
        h: 0,
    }) };
    static CLOCK_FONT_CACHE: RefCell<ClockFontCache> = const { RefCell::new(ClockFontCache {
        entries: Vec::new(),
    }) };
    static ANURATI_WORD_CACHE: RefCell<AnuratiWordCache> = const { RefCell::new(AnuratiWordCache {
        key: None,
        band_y0: 0,
        band_y1: 0,
        sh_buf: Vec::new(),
        fg_buf: Vec::new(),
        edges: Vec::new(),
        crossings: Vec::new(),
        cov_grid: Vec::new(),
        hits: Vec::new(),
    }) };
    static ALLOW_INTERNAL_HIDE: Cell<bool> = const { Cell::new(false) };
    static IN_ZORDER_SYNC: Cell<u32> = const { Cell::new(0) };
    static COLOR_PICK_SESSION: RefCell<Option<ColorPickState>> = const { RefCell::new(None) };
}

struct ZOrderSyncGuard;

impl ZOrderSyncGuard {
    fn enter() -> Self {
        IN_ZORDER_SYNC.with(|g| g.set(g.get().saturating_add(1)));
        Self
    }
}

impl Drop for ZOrderSyncGuard {
    fn drop(&mut self) {
        IN_ZORDER_SYNC.with(|g| g.set(g.get().saturating_sub(1)));
    }
}

fn in_zorder_sync() -> bool {
    IN_ZORDER_SYNC.with(|g| g.get() > 0)
}

fn minute_timer_delay_ms(second: u16) -> u32 {
    // Wait until right after the next minute boundary so idle CPU stays near zero.
    (60u32.saturating_sub(second.min(59) as u32)).clamp(1, 60) * 1000 + 50
}

pub fn take_saved_position() -> Option<(i32, i32)> {
    CLOCK_SAVED_POS.with(|slot| slot.borrow_mut().take())
}

pub fn take_wheel_steps() -> i32 {
    CLOCK_WHEEL_STEPS.with(|slot| std::mem::take(&mut *slot.borrow_mut()))
}

pub fn take_picked_color() -> Option<u32> {
    CLOCK_PICKED_COLOR.with(|slot| slot.take())
}

pub fn is_repositioning() -> bool {
    CLOCK_RUNTIME.with(|slot| slot.borrow().as_ref().is_some_and(|rt| rt.repositioning))
}

/// Central drag zone (`~46% width x 54% height`) prevents accidental drags when touching outer widget margins.
pub fn drag_handle_rect(w: i32, h: i32) -> RECT {
    if w <= 0 || h <= 0 {
        return RECT::default();
    }
    let inner_w = (w * 46 / 100).clamp(80.min(w), w);
    let inner_h = (h * 54 / 100).clamp(48.min(h), h);
    let left = (w - inner_w) / 2;
    let top = (h - inner_h) / 2;
    RECT {
        left,
        top,
        right: left + inner_w,
        bottom: top + inner_h,
    }
}

pub fn in_drag_handle(pt_x: i32, pt_y: i32, w: i32, h: i32) -> bool {
    let r = drag_handle_rect(w, h);
    pt_x >= r.left && pt_x < r.right && pt_y >= r.top && pt_y < r.bottom
}

fn unhook_events(hooks: [isize; 2]) {
    for raw in hooks {
        if raw != 0 {
            // SAFETY: unhooks HWINEVENTHOOK created on this UI thread.
            unsafe {
                let _ = UnhookWinEvent(HWINEVENTHOOK(raw as *mut c_void));
            }
        }
    }
}

fn install_events() -> [isize; 2] {
    // SAFETY: out-of-context UI-thread hooks; own panel activation must also demote the clock.
    unsafe {
        let h1 = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        let h2 = SetWinEventHook(
            EVENT_SYSTEM_MINIMIZESTART,
            EVENT_SYSTEM_MINIMIZEEND,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        [h1.0 as isize, h2.0 as isize]
    }
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    event_hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    let (clock_raw, behind, input_raw) = CLOCK_RUNTIME.with(|s| {
        s.borrow().as_ref().map_or((0, false, 0), |r| {
            (
                r.hwnd,
                r.behind_attached,
                r.input.as_ref().map_or(0, |i| i.hwnd.0 as isize),
            )
        })
    });
    if clock_raw == 0
        || (behind && input_raw == 0)
        || event_hwnd.0 as isize == clock_raw
        || event_hwnd.0 as isize == input_raw
    {
        return;
    }
    ensure_desktop_visible();
    let clock_hwnd = HWND(clock_raw as *mut c_void);
    if unsafe { IsWindow(clock_hwnd).as_bool() } {
        // Two-stage settle matching Rainmeter INTERVAL_SHOWDESKTOP so slow/rapid Win+D never races.
        unsafe {
            let _ = SetTimer(clock_hwnd, CLOCK_ZORDER_SETTLE_TIMER_ID, 60, None);
            let _ = SetTimer(clock_hwnd, CLOCK_ZORDER_LATE_TIMER_ID, 250, None);
        }
    }
}

#[derive(Default)]
struct DesktopTopSearch {
    progman: HWND,
    progman_pid: u32,
    top_shell: HWND,
}

fn is_desktop_shell_window(hwnd: HWND, progman: HWND, progman_pid: u32) -> bool {
    if hwnd.0.is_null() {
        return false;
    }
    if !progman.0.is_null() && hwnd == progman {
        return true;
    }
    // SAFETY: read-only child and rect lookups on a live top-level desktop window handle.
    unsafe {
        if FindWindowExW(hwnd, None, w!("SHELLDLL_DefView"), PCWSTR::null()).is_ok()
            || FindWindowExW(hwnd, None, w!("WallpaperLive.Surface"), PCWSTR::null()).is_ok()
        {
            return true;
        }
        if is_workerw_class(hwnd) && belong_to_pid(hwnd, progman_pid) {
            let mut rc = RECT::default();
            if GetWindowRect(hwnd, &mut rc).is_ok()
                && (rc.right - rc.left) >= 640
                && (rc.bottom - rc.top) >= 480
            {
                return true;
            }
        }
        false
    }
}

unsafe extern "system" fn find_top_shell_cb(window: HWND, data: LPARAM) -> BOOL {
    // SAFETY: read-only EnumWindows top-to-bottom scan matching Explorer's desktop host windows.
    unsafe {
        if !IsWindowVisible(window).as_bool() {
            return BOOL(1);
        }
        let out = &mut *(data.0 as *mut DesktopTopSearch);
        if is_desktop_shell_window(window, out.progman, out.progman_pid) {
            out.top_shell = window;
            return BOOL(0);
        }
    }
    BOOL(1)
}

fn window_belongs_to_shell(hwnd: HWND, shell_top: HWND, progman: HWND, progman_pid: u32) -> bool {
    if hwnd.0.is_null() {
        return false;
    }
    if (!shell_top.0.is_null() && hwnd == shell_top)
        || is_desktop_shell_window(hwnd, progman, progman_pid)
    {
        return true;
    }
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    (!shell_top.0.is_null() && root == shell_top)
        || is_desktop_shell_window(root, progman, progman_pid)
}

fn is_window_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: DwmGetWindowAttribute reads a 4-byte cloaked flag into stack storage.
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
    }
}

fn is_shell_helper_class(hwnd: HWND) -> bool {
    const SHELL_HELPERS: [&str; 7] = [
        "Shell_TrayWnd",
        "Shell_SecondaryTrayWnd",
        "NotifyIconOverflowWindow",
        "EdgeUiInputTopWndClass",
        "Shell_InputSwitchTopLevelWindow",
        "ForegroundStaging",
        "ApplicationManager_DesktopShellWindow",
    ];
    let mut cls = [0u16; 64];
    // SAFETY: read-only class name lookup on live top-level window handle.
    let n = unsafe { GetClassNameW(hwnd, &mut cls) }.clamp(0, cls.len() as i32) as usize;
    SHELL_HELPERS
        .iter()
        .any(|name| n == name.len() && cls[..n].iter().copied().eq(name.bytes().map(u16::from)))
}

fn is_real_app_window_above_shell(
    cur: HWND,
    clock_hwnd: HWND,
    own_hwnds: [isize; 2],
    progman: HWND,
    progman_pid: u32,
) -> bool {
    if cur.0.is_null() || cur == clock_hwnd || own_hwnds.contains(&(cur.0 as isize)) {
        return false;
    }
    // SAFETY: read-only window visibility, ownership, layered alpha, bounds, and DWM cloaked queries.
    unsafe {
        if !IsWindowVisible(cur).as_bool() || IsIconic(cur).as_bool() {
            return false;
        }
        if GetWindow(cur, GW_OWNER).is_ok_and(|o| !o.0.is_null()) {
            return false;
        }
        let ex = GetWindowLongPtrW(cur, GWL_EXSTYLE) as u32;
        if ex & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_TRANSPARENT.0) != 0 {
            return false;
        }
        if ex & WS_EX_LAYERED.0 != 0 {
            let mut alpha = 0u8;
            let mut flags = LAYERED_WINDOW_ATTRIBUTES_FLAGS::default();
            if GetLayeredWindowAttributes(cur, None, Some(&mut alpha), Some(&mut flags)).is_ok()
                && flags.contains(LWA_ALPHA)
                && alpha == 0
            {
                return false;
            }
        }
        if is_shell_helper_class(cur)
            || is_desktop_shell_window(cur, progman, progman_pid)
            || is_window_cloaked(cur)
        {
            return false;
        }
        let mut rc = RECT::default();
        GetWindowRect(cur, &mut rc).is_ok() && rc.right > rc.left && rc.bottom > rc.top
    }
}

fn is_above_in_zorder(upper: HWND, lower: HWND) -> bool {
    if upper.0.is_null() || lower.0.is_null() || upper == lower {
        return false;
    }
    // SAFETY: read-only Z-order walk via GW_HWNDPREV from lower toward screen front.
    unsafe {
        let mut cur = GetWindow(lower, GW_HWNDPREV).unwrap_or_default();
        let mut hops = 0usize;
        while !cur.0.is_null() && hops < 4096 {
            if cur == upper {
                return true;
            }
            cur = GetWindow(cur, GW_HWNDPREV).unwrap_or_default();
            hops += 1;
        }
    }
    false
}

fn popup_placement_verified(swp_ok: bool, visible: bool, above_shell: bool) -> bool {
    swp_ok && visible && above_shell
}

#[derive(Clone)]
enum PopupZTarget {
    ShowDesktopTopmost(HWND),
    BehindApps {
        candidates: Vec<HWND>,
        owner_fallback: HWND,
        shell_top: HWND,
    },
}

fn detect_popup_z_target(clock_hwnd: HWND, repositioning: bool) -> PopupZTarget {
    if repositioning {
        return PopupZTarget::ShowDesktopTopmost(HWND::default());
    }
    let (own_hwnds, owner_raw) = CLOCK_RUNTIME.with(|s| {
        s.borrow().as_ref().map_or(([0, 0], 0), |r| {
            (
                [r.hwnd, r.input.as_ref().map_or(0, |i| i.hwnd.0 as isize)],
                r.owner,
            )
        })
    });
    // SAFETY: walks global top-level Z-order to mirror Rainmeter System::PrepareHelperWindow.
    unsafe {
        let owner_hwnd = HWND(owner_raw as *mut c_void);
        let owner_fallback = if !owner_hwnd.0.is_null()
            && owner_hwnd != clock_hwnd
            && !own_hwnds.contains(&(owner_hwnd.0 as isize))
            && IsWindow(owner_hwnd).as_bool()
            && IsWindowVisible(owner_hwnd).as_bool()
            && !IsIconic(owner_hwnd).as_bool()
            && (GetWindowLongPtrW(owner_hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 == 0)
        {
            owner_hwnd
        } else {
            HWND::default()
        };

        let progman = FindWindowW(w!("Progman"), PCWSTR::null()).unwrap_or_default();
        let mut progman_pid = 0u32;
        if !progman.0.is_null() {
            GetWindowThreadProcessId(progman, Some(&mut progman_pid));
        }
        let mut search = DesktopTopSearch {
            progman,
            progman_pid,
            top_shell: HWND::default(),
        };
        let _ = EnumWindows(
            Some(find_top_shell_cb),
            LPARAM((&mut search as *mut DesktopTopSearch) as isize),
        );
        let shell_top = if !search.top_shell.0.is_null() {
            search.top_shell
        } else {
            progman
        };
        if shell_top.0.is_null() {
            return PopupZTarget::BehindApps {
                candidates: vec![HWND_BOTTOM],
                owner_fallback,
                shell_top: HWND::default(),
            };
        }

        let shell_ex = GetWindowLongPtrW(shell_top, GWL_EXSTYLE) as u32;
        if shell_ex & WS_EX_TOPMOST.0 != 0 {
            let mut last_topmost = HWND::default();
            let mut cur = GetWindow(shell_top, GW_HWNDPREV).unwrap_or_default();
            let mut hops = 0usize;
            while !cur.0.is_null() && hops < 4096 {
                hops += 1;
                if cur != clock_hwnd
                    && !own_hwnds.contains(&(cur.0 as isize))
                    && IsWindowVisible(cur).as_bool()
                    && !is_desktop_shell_window(cur, progman, progman_pid)
                {
                    let ex = GetWindowLongPtrW(cur, GWL_EXSTYLE) as u32;
                    if ex & WS_EX_TOPMOST.0 != 0 {
                        last_topmost = cur;
                        break;
                    }
                }
                cur = GetWindow(cur, GW_HWNDPREV).unwrap_or_default();
            }
            return PopupZTarget::ShowDesktopTopmost(last_topmost);
        }

        let fg = GetForegroundWindow();
        let mut candidates = Vec::new();
        let mut last_topmost = HWND::default();
        let mut cur = GetWindow(shell_top, GW_HWNDPREV).unwrap_or_default();
        let mut hops = 0usize;
        while !cur.0.is_null() && hops < 4096 {
            hops += 1;
            if cur == clock_hwnd
                || own_hwnds.contains(&(cur.0 as isize))
                || !IsWindowVisible(cur).as_bool()
                || is_desktop_shell_window(cur, progman, progman_pid)
            {
                cur = GetWindow(cur, GW_HWNDPREV).unwrap_or_default();
                continue;
            }
            let ex = GetWindowLongPtrW(cur, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOPMOST.0 != 0 {
                if last_topmost.0.is_null() {
                    last_topmost = cur;
                }
                break;
            }
            if is_real_app_window_above_shell(cur, clock_hwnd, own_hwnds, progman, progman_pid) {
                candidates.push(cur);
            }
            cur = GetWindow(cur, GW_HWNDPREV).unwrap_or_default();
        }

        // Exact Rainmeter ShowDesktop rule: if desktop is foreground or sits directly under TOPMOST band.
        let show_desktop =
            candidates.is_empty() || window_belongs_to_shell(fg, shell_top, progman, progman_pid);
        if show_desktop {
            PopupZTarget::ShowDesktopTopmost(last_topmost)
        } else {
            PopupZTarget::BehindApps {
                candidates,
                owner_fallback,
                shell_top,
            }
        }
    }
}

fn place_popup_on_desktop(
    hwnd: HWND,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    resize_move: bool,
    repositioning: bool,
) {
    let mut base_flags = SWP_NOACTIVATE | SWP_SHOWWINDOW;
    if !resize_move {
        base_flags |= SWP_NOMOVE | SWP_NOSIZE;
    }
    let target = detect_popup_z_target(hwnd, repositioning);
    let _z_guard = ZOrderSyncGuard::enter();
    // SAFETY: owns clock HWND; ZOrderSyncGuard prevents WM_WINDOWPOSCHANGING recursion.
    unsafe {
        match target {
            PopupZTarget::ShowDesktopTopmost(after_topmost) => {
                // Step 1: enter topmost band (only below Taskbar/Start/Notifications) so Explorer cannot raise Progman above clock.
                let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, base_flags);
                if !after_topmost.0.is_null() && after_topmost != hwnd {
                    let _ = SetWindowPos(
                        hwnd,
                        after_topmost,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                }
                if !IsWindowVisible(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                }
            }
            PopupZTarget::BehindApps {
                candidates,
                owner_fallback,
                shell_top,
            } => {
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
                if ex & WS_EX_TOPMOST.0 != 0 {
                    let _ = SetWindowPos(
                        hwnd,
                        HWND_NOTOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
                let try_place = |after: HWND| -> bool {
                    let ok = SetWindowPos(hwnd, after, x, y, w, h, base_flags).is_ok();
                    let visible = IsWindowVisible(hwnd).as_bool();
                    let above_shell = shell_top.0.is_null() || is_above_in_zorder(hwnd, shell_top);
                    popup_placement_verified(ok, visible, above_shell)
                };
                let _ = candidates.iter().copied().any(try_place)
                    || (!owner_fallback.0.is_null()
                        && !candidates.contains(&owner_fallback)
                        && try_place(owner_fallback))
                    || try_place(HWND_TOP)
                    || try_place(HWND_TOPMOST);
                if !IsWindowVisible(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    if resize_move {
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
            }
        }
    }
}

fn restore_if_iconic(hwnd: HWND) {
    // SAFETY: SetWindowPlacement clears minimized WS_MINIMIZE silently without stealing active focus.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let mut wp = WINDOWPLACEMENT {
                length: size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            if GetWindowPlacement(hwnd, &mut wp).is_ok() {
                wp.showCmd = SW_SHOWNOACTIVATE.0 as u32;
                let _ = SetWindowPlacement(hwnd, &wp);
            }
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        } else if !IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }
}

pub fn ensure_desktop_visible() {
    let state = CLOCK_RUNTIME.with(|slot| {
        slot.borrow().as_ref().and_then(|rt| {
            (rt.hwnd != 0 && rt.cfg.enabled).then_some((
                rt.hwnd,
                rt.behind_attached,
                rt.cached_layer,
                rt.repositioning,
                rt.input.as_ref().map(|i| i.hwnd),
            ))
        })
    });
    let Some((raw, behind_attached, cached_layer, repositioning, input_hwnd)) = state else {
        return;
    };
    let hwnd = HWND(raw as *mut c_void);
    if unsafe { !IsWindow(hwnd).as_bool() } {
        return;
    }
    restore_if_iconic(hwnd);

    if behind_attached && !repositioning {
        let layer = cached_layer
            .filter(|&l| layer_still_valid(l))
            .or_else(discover_desktop_layer);
        if let Some(l) = layer {
            let after = if l.raised { l.icons } else { HWND_TOP };
            let _z_guard = ZOrderSyncGuard::enter();
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    after,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
        }
        if let Some(input) = input_hwnd
            && unsafe { IsWindow(input).as_bool() }
        {
            restore_if_iconic(input);
            place_popup_on_desktop(input, 0, 0, 0, 0, false, false);
        }
    } else {
        place_popup_on_desktop(hwnd, 0, 0, 0, 0, false, repositioning);
    }
}

pub fn sync_zorder() {
    ensure_desktop_visible();
}

fn ensure_class(name: PCWSTR, proc: WNDPROC, hbr: HGDIOBJ, cursor: PCWSTR) -> Result<(), String> {
    let instance = unsafe { GetModuleHandleW(None) }.map_err(|e| e.to_string())?;
    let mut existing = WNDCLASSEXW::default();
    if unsafe { GetClassInfoExW(instance, name, &mut existing) }.is_ok() {
        return Ok(());
    }
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: proc,
        hInstance: instance.into(),
        lpszClassName: name,
        hCursor: unsafe { LoadCursorW(None, cursor) }.map_err(|e| e.to_string())?,
        hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(hbr.0),
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&wc) } == 0 {
        let mut check = WNDCLASSEXW::default();
        if unsafe { GetClassInfoExW(instance, name, &mut check) }.is_err() {
            return Err("Could not register desktop clock window class".into());
        }
    }
    Ok(())
}

fn destroy_clock_hwnd(raw: isize, hooks: [isize; 2]) {
    unhook_events(hooks);
    if raw == 0 {
        return;
    }
    let hwnd = HWND(raw as *mut c_void);
    ALLOW_INTERNAL_HIDE.with(|g| g.set(true));
    // SAFETY: owns this single HWND on UI thread.
    unsafe {
        if IsWindow(hwnd).as_bool() {
            let _ = KillTimer(hwnd, CLOCK_TIMER_ID);
            let _ = KillTimer(hwnd, CLOCK_ZORDER_SETTLE_TIMER_ID);
            let _ = KillTimer(hwnd, CLOCK_ZORDER_LATE_TIMER_ID);
            let _ = DestroyWindow(hwnd);
        }
    }
    ALLOW_INTERNAL_HIDE.with(|g| g.set(false));
}

pub fn clear_font_cache() {
    CLOCK_FONT_CACHE.with(|s| s.borrow_mut().free());
    ANURATI_WORD_CACHE.with(|s| s.borrow_mut().free());
}

/// Releases offscreen DIB scratch buffers and temporary D3D11/DXGI staging memory while idle
/// between minute ticks or when the control panel hides to tray.
pub fn trim_transient_caches() {
    CLOCK_TEXT_SCRATCH.with(|s| s.borrow_mut().free());
    CLOCK_CANVAS_SCRATCH.with(|s| s.borrow_mut().free());
    ANURATI_WORD_CACHE.with(|s| s.borrow_mut().release_build_scratch());
    CLOCK_RUNTIME.with(|slot| {
        if let Some(dc) = slot.borrow().as_ref().and_then(|rt| rt.dcomp.as_ref()) {
            dc.trim_gpu();
        }
    });
}

pub fn hide_all() {
    cancel_color_pick();
    let old = CLOCK_RUNTIME.with(|slot| slot.borrow_mut().take());
    if let Some(mut rt) = old {
        drop(rt.input.take());
        destroy_clock_hwnd(rt.hwnd, rt.hooks);
    }
    CLOCK_TEXT_SCRATCH.with(|s| s.borrow_mut().free());
    CLOCK_CANVAS_SCRATCH.with(|s| s.borrow_mut().free());
    CLOCK_FONT_CACHE.with(|s| s.borrow_mut().free());
    ANURATI_WORD_CACHE.with(|s| s.borrow_mut().free());
}

pub fn begin_reposition(owner: HWND, action_msg: u32, cfg: ClockConfig) {
    if !cfg.enabled {
        return;
    }
    CLOCK_RUNTIME.with(|slot| {
        if let Some(rt) = slot.borrow_mut().as_mut() {
            rt.repositioning = true;
        }
    });
    sync_inner(owner, action_msg, cfg, true);
}

pub fn end_reposition(owner: HWND, action_msg: u32, cfg: ClockConfig) {
    CLOCK_RUNTIME.with(|slot| {
        if let Some(rt) = slot.borrow_mut().as_mut() {
            rt.repositioning = false;
            rt.dragging = false;
            rt.drag_moved = false;
        }
    });
    if cfg.enabled {
        sync_inner(owner, action_msg, cfg, false);
    }
}

pub fn sync(owner: HWND, action_msg: u32, cfg: ClockConfig) {
    let rep = is_repositioning();
    sync_inner(owner, action_msg, cfg, rep);
}

/// Live preview update during slider/wheel/eyedropper drags: updates clock rendering without
/// issuing cross-process MSAA `accLocation` IPC on every mouse move.
pub fn sync_live_preview(owner: HWND, action_msg: u32, cfg: ClockConfig) {
    if !cfg.enabled {
        hide_all();
        return;
    }
    let existing = CLOCK_RUNTIME.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let rt = borrow.as_mut()?;
        let valid = rt.hwnd != 0 && unsafe { IsWindow(HWND(rt.hwnd as *mut c_void)).as_bool() };
        if !valid {
            return None;
        }
        rt.owner = owner.0 as isize;
        rt.action_msg = action_msg;
        rt.cfg = cfg;
        Some(HWND(rt.hwnd as *mut c_void))
    });
    if let Some(hwnd) = existing {
        render_and_place_mode(hwnd, true);
    } else {
        sync(owner, action_msg, cfg);
    }
}

/// Pure click-through rule separating `Lock position` (blocks drag/wheel only) from `Behind desktop shortcuts`.
pub fn is_click_through(cfg: ClockConfig, repositioning: bool) -> bool {
    !repositioning && cfg.behind_icons
}

fn sync_inner(owner: HWND, action_msg: u32, cfg: ClockConfig, repositioning: bool) {
    if !cfg.enabled {
        hide_all();
        return;
    }
    if ensure_class(CLOCK_CLASS, Some(clock_proc), HGDIOBJ::default(), IDC_ARROW).is_err() {
        return;
    }
    ensure_quicksand_loaded();

    let want_behind = cfg.behind_icons && !repositioning;
    let cached_layer = CLOCK_RUNTIME.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|rt| rt.cached_layer.filter(|&l| layer_still_valid(l)))
    });
    let layer = if want_behind {
        cached_layer.or_else(discover_desktop_layer)
    } else {
        None
    };
    let actual_behind = want_behind && layer.is_some();

    let (need_recreate, old_raw, old_hooks) = CLOCK_RUNTIME.with(|slot| {
        let borrow = slot.borrow();
        let Some(rt) = borrow.as_ref() else {
            return (true, 0, [0, 0]);
        };
        let valid = rt.hwnd != 0 && unsafe { IsWindow(HWND(rt.hwnd as *mut c_void)).as_bool() };
        if !valid || rt.behind_attached != actual_behind {
            (true, rt.hwnd, rt.hooks)
        } else {
            (false, rt.hwnd, rt.hooks)
        }
    });

    if need_recreate {
        let old = CLOCK_RUNTIME.with(|slot| slot.borrow_mut().take());
        drop(old);
        destroy_clock_hwnd(old_raw, old_hooks);
        let Ok(instance) = (unsafe { GetModuleHandleW(None) }) else {
            return;
        };

        let mut attached_layer = layer;
        let mut attached_behind = actual_behind;
        let mut dcomp: Option<DCompPresenter> = None;

        let hwnd = if let Some(l) = attached_layer {
            let child_ex = if l.raised {
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE
            } else {
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE
            };
            let h = unsafe {
                CreateWindowExW(
                    child_ex,
                    CLOCK_CLASS,
                    w!("WallpaperLive Day Clock"),
                    WS_CHILD | WS_CLIPSIBLINGS,
                    0,
                    0,
                    100,
                    100,
                    l.parent,
                    None,
                    instance,
                    None,
                )
            };
            match h {
                Ok(child) if l.raised => {
                    if let Some(dc) = DCompPresenter::new(child, 400, 176) {
                        dcomp = Some(dc);
                        child
                    } else {
                        // DComp unavailable on stripped GPU/RDP: cleanly fall back to Rainmeter WS_POPUP.
                        unsafe {
                            let _ = DestroyWindow(child);
                        }
                        attached_behind = false;
                        attached_layer = None;
                        let Ok(pop) = (unsafe {
                            CreateWindowExW(
                                WS_EX_LAYERED
                                    | WS_EX_TOOLWINDOW
                                    | WS_EX_NOACTIVATE
                                    | WS_EX_TRANSPARENT,
                                CLOCK_CLASS,
                                w!("WallpaperLive Day Clock"),
                                WS_POPUP,
                                0,
                                0,
                                100,
                                100,
                                HWND::default(),
                                None,
                                instance,
                                None,
                            )
                        }) else {
                            return;
                        };
                        pop
                    }
                }
                Ok(child) => child,
                Err(_) => {
                    attached_behind = false;
                    attached_layer = None;
                    let Ok(pop) = (unsafe {
                        CreateWindowExW(
                            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
                            CLOCK_CLASS,
                            w!("WallpaperLive Day Clock"),
                            WS_POPUP,
                            0,
                            0,
                            100,
                            100,
                            HWND::default(),
                            None,
                            instance,
                            None,
                        )
                    }) else {
                        return;
                    };
                    pop
                }
            }
        } else {
            let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
            let Ok(pop) = (unsafe {
                CreateWindowExW(
                    ex,
                    CLOCK_CLASS,
                    w!("WallpaperLive Day Clock"),
                    WS_POPUP,
                    0,
                    0,
                    100,
                    100,
                    HWND::default(),
                    None,
                    instance,
                    None,
                )
            }) else {
                return;
            };
            pop
        };

        let hooks = install_events();
        let now = unsafe { local_time() };
        unsafe {
            let _ = SetTimer(
                hwnd,
                CLOCK_TIMER_ID,
                minute_timer_delay_ms(now.wSecond),
                None,
            );
        }
        CLOCK_RUNTIME.with(|slot| {
            *slot.borrow_mut() = Some(ClockRuntime {
                hwnd: hwnd.0 as isize,
                owner: owner.0 as isize,
                action_msg,
                cfg,
                behind_attached: attached_behind,
                cached_layer: attached_layer,
                dcomp,
                input: None,
                repositioning,
                dragging: false,
                drag_moved: false,
                drag_start_x: 0,
                drag_start_y: 0,
                drag_offset_x: 0,
                drag_offset_y: 0,
                width: 100,
                height: 100,
                last_stamp: (0, 0, 0, 0, 0, 0),
                last_bounds: None,
                pending_wheel_delta: 0,
                hooks,
            });
        });
        render_and_place(hwnd);
    } else {
        let hwnd = HWND(old_raw as *mut c_void);
        CLOCK_RUNTIME.with(|slot| {
            if let Some(rt) = slot.borrow_mut().as_mut() {
                rt.owner = owner.0 as isize;
                rt.action_msg = action_msg;
                rt.cfg = cfg;
                rt.repositioning = repositioning;
                if layer.is_some() {
                    rt.cached_layer = layer;
                }
            }
        });
        if !actual_behind {
            let pass_all = is_click_through(cfg, repositioning);
            unsafe {
                let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
                let next = if pass_all {
                    cur | WS_EX_TRANSPARENT.0
                } else {
                    cur & !WS_EX_TRANSPARENT.0
                };
                if next != cur {
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next as isize);
                }
            }
        }
        render_and_place(hwnd);
    }
}

fn month_name(m: u16) -> &'static str {
    match m {
        1 => "JANUARY",
        2 => "FEBRUARY",
        3 => "MARCH",
        4 => "APRIL",
        5 => "MAY",
        6 => "JUNE",
        7 => "JULY",
        8 => "AUGUST",
        9 => "SEPTEMBER",
        10 => "OCTOBER",
        11 => "NOVEMBER",
        12 => "DECEMBER",
        _ => "",
    }
}

fn weekday_name(dow: u16) -> &'static str {
    match dow {
        0 => "SUNDAY",
        1 => "MONDAY",
        2 => "TUESDAY",
        3 => "WEDNESDAY",
        4 => "THURSDAY",
        5 => "FRIDAY",
        6 => "SATURDAY",
        _ => "DAY",
    }
}

pub fn format_clock_lines(
    year: u16,
    month: u16,
    dow: u16,
    day: u16,
    hour: u16,
    minute: u16,
    use_24h: bool,
) -> (String, String, String) {
    let day_str = weekday_name(dow).to_owned();
    let date_str = format!("{day:02}  {},  {year}.", month_name(month));
    let time_str = if use_24h {
        format!("- {hour:02}:{minute:02} -")
    } else {
        let suffix = if hour >= 12 { "PM" } else { "AM" };
        let h12 = match hour % 12 {
            0 => 12,
            h => h,
        };
        format!("- {h12}:{minute:02} {suffix} -")
    };
    (day_str, date_str, time_str)
}

#[derive(Clone, Copy)]
enum AnuratiSeg {
    Move(f32, f32),
    Line(f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

#[derive(Clone, Copy)]
struct AnuratiGlyph {
    advance: f32,
    visual_width: f32,
    ops: &'static [AnuratiSeg],
}

use AnuratiSeg::{Close as Z, Cubic as C, Line as L, Move as M};

// Exact Mond Anurati glyph contours at em=100.0 (cap top=-80.0, baseline=0.0).
const GLYPH_S: AnuratiGlyph = AnuratiGlyph {
    advance: 80.3,
    visual_width: 66.6,
    ops: &[
        M(43.2, -33.3),
        C(46.0, -33.3, 48.3, -32.3, 50.3, -30.3),
        C(52.2, -28.3, 53.3, -26.0, 53.3, -23.3),
        C(53.3, -21.9, 53.0, -20.6, 52.5, -19.4),
        C(52.0, -18.1, 51.3, -17.1, 50.4, -16.2),
        C(49.5, -15.3, 48.4, -14.6, 47.1, -14.1),
        C(45.9, -13.6, 44.6, -13.4, 43.2, -13.4),
        L(43.2, 0.0),
        C(49.7, 0.0, 55.2, -2.3, 59.7, -6.8),
        C(64.3, -11.4, 66.6, -16.9, 66.6, -23.3),
        C(66.6, -29.7, 64.3, -35.2, 59.7, -39.8),
        C(55.2, -44.3, 49.7, -46.6, 43.2, -46.6),
        L(23.3, -46.6),
        C(20.4, -46.6, 18.1, -47.6, 16.1, -49.6),
        C(14.2, -51.6, 13.3, -53.9, 13.3, -56.7),
        C(13.3, -58.0, 13.5, -59.3, 14.0, -60.5),
        C(14.5, -61.7, 15.2, -62.8, 16.1, -63.7),
        C(17.1, -64.7, 18.1, -65.4, 19.4, -65.9),
        C(20.6, -66.4, 21.9, -66.6, 23.3, -66.6),
        L(23.3, -80.0),
        C(16.8, -80.0, 11.3, -77.7, 6.8, -73.1),
        C(2.3, -68.5, 0.0, -63.1, 0.0, -56.7),
        C(0.0, -50.2, 2.3, -44.7, 6.8, -40.1),
        C(11.3, -35.6, 16.8, -33.3, 23.3, -33.3),
        Z,
    ],
};

const GLYPH_U: AnuratiGlyph = AnuratiGlyph {
    advance: 78.2,
    visual_width: 66.6,
    ops: &[
        M(53.3, -33.3),
        C(53.3, -27.9, 51.2, -23.2, 47.3, -19.3),
        C(43.4, -15.4, 38.7, -13.4, 33.2, -13.4),
        C(27.7, -13.4, 22.9, -15.3, 19.0, -19.3),
        C(15.2, -23.1, 13.3, -27.8, 13.3, -33.3),
        L(13.3, -80.0),
        L(0.0, -80.0),
        L(0.0, -33.3),
        C(0.0, -24.2, 3.2, -16.2, 9.7, -9.7),
        C(16.2, -3.2, 24.1, 0.0, 33.2, 0.0),
        C(42.3, 0.0, 50.3, -3.2, 56.8, -9.7),
        C(63.4, -16.2, 66.6, -24.1, 66.6, -33.3),
        Z,
    ],
};

const GLYPH_N: AnuratiGlyph = AnuratiGlyph {
    advance: 81.8,
    visual_width: 66.5,
    ops: &[
        M(53.2, -80.0),
        L(53.2, -32.1),
        L(0.0, -80.0),
        L(0.0, -59.8),
        L(66.5, 0.0),
        L(66.5, -80.0),
        Z,
    ],
};

const GLYPH_D: AnuratiGlyph = AnuratiGlyph {
    advance: 83.0,
    visual_width: 66.6,
    ops: &[
        M(63.5, -55.4),
        C(61.3, -60.3, 58.5, -64.5, 54.8, -68.2),
        C(51.2, -71.8, 46.9, -74.7, 42.1, -76.8),
        C(37.2, -78.9, 32.1, -80.0, 26.6, -80.0),
        L(0.0, -80.0),
        L(13.3, -66.6),
        L(26.6, -66.6),
        C(30.2, -66.6, 33.7, -65.9, 36.9, -64.6),
        C(40.1, -63.2, 43.0, -61.2, 45.4, -58.8),
        C(47.9, -56.3, 49.8, -53.5, 51.2, -50.3),
        C(52.6, -47.0, 53.3, -43.6, 53.3, -40.0),
        C(53.3, -36.4, 52.6, -33.0, 51.2, -29.7),
        C(49.8, -26.5, 47.9, -23.7, 45.5, -21.2),
        C(43.1, -18.7, 40.2, -16.8, 37.0, -15.4),
        C(33.8, -14.1, 30.3, -13.4, 26.6, -13.4),
        L(13.3, -13.4),
        L(0.0, 0.0),
        L(26.6, 0.0),
        C(32.1, 0.0, 37.3, -1.1, 42.1, -3.2),
        C(47.0, -5.3, 51.3, -8.2, 54.9, -11.8),
        C(58.5, -15.5, 61.3, -19.7, 63.5, -24.6),
        C(65.6, -29.4, 66.6, -34.5, 66.6, -40.0),
        C(66.6, -45.5, 65.6, -50.6, 63.5, -55.4),
        Z,
    ],
};

const GLYPH_A: AnuratiGlyph = AnuratiGlyph {
    advance: 89.9,
    visual_width: 85.0,
    ops: &[
        M(33.2, -26.6),
        L(45.0, -50.1),
        L(56.7, -26.6),
        L(71.7, -26.6),
        L(45.0, -80.0),
        L(5.0, 0.0),
        L(19.8, 0.0),
        L(26.6, -13.4),
        Z,
        M(78.3, -13.3),
        L(63.5, -13.3),
        L(70.1, 0.0),
        L(85.0, 0.0),
        Z,
    ],
};

const GLYPH_Y: AnuratiGlyph = AnuratiGlyph {
    advance: 88.8,
    visual_width: 79.5,
    ops: &[
        M(48.3, -40.0),
        L(79.5, -80.0),
        L(62.6, -80.0),
        L(39.8, -50.8),
        L(17.1, -80.0),
        L(0.0, -80.0),
        L(31.4, -40.0),
        L(0.0, 0.0),
        L(17.1, 0.0),
        L(39.8, -29.2),
        Z,
    ],
};

const GLYPH_M: AnuratiGlyph = AnuratiGlyph {
    advance: 95.4,
    visual_width: 80.1,
    ops: &[
        M(0.0, -61.4),
        L(40.1, -21.1),
        L(66.9, -47.8),
        L(66.9, 0.0),
        L(80.1, 0.0),
        L(80.1, -80.0),
        L(40.1, -40.0),
        L(0.0, -80.0),
        Z,
    ],
};

const GLYPH_O: AnuratiGlyph = AnuratiGlyph {
    advance: 96.5,
    visual_width: 80.1,
    ops: &[
        M(40.0, -80.0),
        C(28.9, -80.0, 19.5, -76.0, 11.8, -68.2),
        C(4.0, -60.4, 0.0, -51.0, 0.0, -40.0),
        C(0.0, -28.9, 4.0, -19.6, 11.8, -11.8),
        C(19.6, -4.0, 28.9, 0.0, 40.0, 0.0),
        C(51.1, 0.0, 60.4, -4.0, 68.2, -11.8),
        C(76.1, -19.5, 80.1, -28.9, 80.1, -40.0),
        C(80.1, -51.0, 76.1, -60.4, 68.2, -68.2),
        C(60.4, -76.0, 51.1, -80.0, 40.0, -80.0),
        Z,
        M(40.0, -66.6),
        C(47.3, -66.6, 53.6, -64.0, 58.8, -58.8),
        C(64.0, -53.6, 66.6, -47.3, 66.6, -40.0),
        C(66.6, -32.7, 64.1, -26.4, 58.8, -21.1),
        C(53.6, -15.9, 47.3, -13.4, 40.0, -13.4),
        C(32.7, -13.4, 26.3, -15.9, 21.1, -21.1),
        C(15.9, -26.3, 13.4, -32.7, 13.4, -40.0),
        C(13.4, -47.3, 15.9, -53.6, 21.1, -58.8),
        C(26.4, -64.1, 32.7, -66.6, 40.0, -66.6),
        Z,
    ],
};

const GLYPH_T: AnuratiGlyph = AnuratiGlyph {
    advance: 51.0,
    visual_width: 40.0,
    ops: &[
        M(0.0, -80.0),
        L(0.0, -66.6),
        L(0.0, 0.0),
        L(13.4, 0.0),
        L(13.4, -66.6),
        L(40.0, -66.6),
        L(40.0, -80.0),
        Z,
    ],
};

const GLYPH_H: AnuratiGlyph = AnuratiGlyph {
    advance: 81.9,
    visual_width: 66.6,
    ops: &[
        M(53.3, -80.0),
        L(53.3, -46.7),
        L(13.3, -46.7),
        L(0.0, -33.3),
        L(0.0, 0.0),
        L(13.3, 0.0),
        L(13.3, -33.3),
        L(53.3, -33.3),
        L(53.3, 0.0),
        L(66.6, 0.0),
        L(66.6, -80.0),
        Z,
    ],
};

const GLYPH_E: AnuratiGlyph = AnuratiGlyph {
    advance: 63.6,
    visual_width: 53.3,
    ops: &[
        M(0.0, -1.1),
        L(0.0, 0.0),
        L(53.3, 0.0),
        L(53.3, -13.4),
        L(13.3, -13.4),
        Z,
        M(0.0, -80.0),
        L(13.3, -66.6),
        L(53.3, -66.6),
        L(53.3, -80.0),
        Z,
        M(0.0, -46.6),
        L(0.0, -33.3),
        L(53.3, -33.3),
        L(53.3, -46.6),
        Z,
    ],
};

const GLYPH_R: AnuratiGlyph = AnuratiGlyph {
    advance: 76.1,
    visual_width: 61.7,
    ops: &[
        M(57.0, -38.3),
        C(60.2, -42.8, 61.7, -47.8, 61.7, -53.4),
        C(61.7, -60.7, 59.1, -66.9, 53.9, -72.2),
        C(48.7, -77.4, 42.4, -80.0, 35.1, -80.0),
        L(0.0, -80.0),
        L(6.9, -68.1),
        L(8.4, -66.6),
        L(35.1, -66.6),
        C(38.7, -66.6, 41.8, -65.3, 44.4, -62.7),
        C(47.1, -60.1, 48.4, -57.0, 48.4, -53.4),
        C(48.4, -49.7, 47.1, -46.5, 44.4, -43.9),
        C(41.8, -41.3, 38.7, -40.0, 35.1, -40.0),
        L(23.2, -40.1),
        L(46.4, 0.0),
        L(61.7, 0.0),
        L(45.3, -28.7),
        C(50.0, -30.6, 53.9, -33.8, 57.0, -38.3),
        Z,
    ],
};

const GLYPH_F: AnuratiGlyph = AnuratiGlyph {
    advance: 63.7,
    visual_width: 53.3,
    ops: &[
        M(0.0, -40.1),
        L(0.0, 0.0),
        L(13.3, 0.0),
        L(13.3, -26.6),
        L(53.3, -26.6),
        L(53.3, -40.1),
        L(13.3, -40.1),
        Z,
        M(0.0, -80.0),
        L(13.3, -66.6),
        L(53.3, -66.6),
        L(53.3, -80.0),
        Z,
    ],
};

const GLYPH_W: AnuratiGlyph = AnuratiGlyph {
    advance: 141.7,
    visual_width: 130.3,
    ops: &[
        M(14.9, -80.0),
        L(0.0, -80.0),
        L(32.6, -14.9),
        L(40.1, -29.9),
        Z,
        M(115.4, -80.0),
        L(90.3, -29.9),
        L(72.6, -65.1),
        L(65.1, -50.1),
        L(90.3, 0.0),
        L(130.3, -80.0),
        Z,
    ],
};

const GLYPH_I: AnuratiGlyph = AnuratiGlyph {
    advance: 14.0,
    visual_width: 13.3,
    ops: &[
        M(13.3, -66.8),
        L(0.0, -80.0),
        L(0.0, 0.0),
        L(13.3, -13.4),
        Z,
    ],
};

fn anurati_glyph(ch: char) -> Option<&'static AnuratiGlyph> {
    match ch {
        'A' => Some(&GLYPH_A),
        'D' => Some(&GLYPH_D),
        'E' => Some(&GLYPH_E),
        'F' => Some(&GLYPH_F),
        'H' => Some(&GLYPH_H),
        'I' => Some(&GLYPH_I),
        'M' => Some(&GLYPH_M),
        'N' => Some(&GLYPH_N),
        'O' => Some(&GLYPH_O),
        'R' => Some(&GLYPH_R),
        'S' => Some(&GLYPH_S),
        'T' => Some(&GLYPH_T),
        'U' => Some(&GLYPH_U),
        'W' => Some(&GLYPH_W),
        'Y' => Some(&GLYPH_Y),
        _ => None,
    }
}

const ANURATI_TRACKING_UNITS: f32 = 18.0;
const ANURATI_CAP_HEIGHT_UNITS: f32 = 80.0;

fn measure_anurati_word_units(word: &str) -> f32 {
    let mut glyphs = word.chars().filter_map(anurati_glyph).peekable();
    let mut width = 0.0f32;
    while let Some(g) = glyphs.next() {
        if glyphs.peek().is_some() {
            width += g.advance + ANURATI_TRACKING_UNITS;
        } else {
            width += g.visual_width;
        }
    }
    width
}

pub fn widget_pixel_size(day_str: &str, size_pct: u32, dpi: i32) -> (i32, i32) {
    let scale = (size_pct.clamp(50, 200) as f32 / 100.0) * (dpi.max(96) as f32 / 96.0);
    let cap_h = 50.0 * scale;
    let em_scale = cap_h / ANURATI_CAP_HEIGHT_UNITS;
    let day_px = measure_anurati_word_units(day_str) * em_scale;
    let width = (day_px + 56.0 * scale).ceil().max(360.0 * scale) as i32;
    let height = (176.0 * scale).ceil() as i32;
    (width.clamp(240, 2400), height.clamp(120, 900))
}

fn target_monitor_info(
    hwnd: HWND,
    owner: HWND,
    pos: Option<(i32, i32)>,
    approx_size: (i32, i32),
) -> (RECT, i32) {
    let mon = if let Some((x, y)) = pos {
        let (rw, rh) = approx_size;
        let rc = RECT {
            left: x,
            top: y,
            right: x.saturating_add(rw.max(1)),
            bottom: y.saturating_add(rh.max(1)),
        };
        unsafe { MonitorFromRect(&rc, MONITOR_DEFAULTTONEAREST) }
    } else if !owner.0.is_null() && unsafe { IsWindow(owner).as_bool() } {
        unsafe { MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST) }
    } else {
        unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }
    };
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let work = if unsafe { GetMonitorInfoW(mon, &mut mi) }.as_bool() {
        mi.rcWork
    } else {
        RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }
    };
    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    let dpi = if unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }.is_ok()
    {
        dpi_x as i32
    } else if !owner.0.is_null() {
        unsafe { GetDpiForWindow(owner) }.max(96) as i32
    } else {
        96
    };
    (work, dpi.max(96))
}

fn resolve_screen_position(pos: Option<(i32, i32)>, work: RECT, w: i32, h: i32) -> (i32, i32) {
    let (raw_x, raw_y) = match pos {
        Some((x, y)) => (x, y),
        None => {
            let cx = work.left + (work.right - work.left - w) / 2;
            let cy = work.top + ((work.bottom - work.top) / 4 - h / 2).max(24);
            (cx, cy)
        }
    };
    let max_x = (work.right - w).max(work.left);
    let max_y = (work.bottom - h).max(work.top);
    (raw_x.clamp(work.left, max_x), raw_y.clamp(work.top, max_y))
}

#[inline(always)]
fn div255_round(v: u32) -> u32 {
    let q = v + 127;
    (q + 1 + (q >> 8)) >> 8
}

#[inline(always)]
fn premul_blend_pixel(dst: &mut u32, r: u32, g: u32, b: u32, a: u8) {
    if a == 0 {
        return;
    }
    let a_u = u32::from(a);
    let src_r = div255_round(r * a_u);
    let src_g = div255_round(g * a_u);
    let src_b = div255_round(b * a_u);
    let inv = 255 - a_u;
    let cur = *dst;
    let cur_a = (cur >> 24) & 255;
    let cur_r = (cur >> 16) & 255;
    let cur_g = (cur >> 8) & 255;
    let cur_b = cur & 255;
    let out_a = (a_u + div255_round(cur_a * inv)).min(255);
    let out_r = (src_r + div255_round(cur_r * inv)).min(out_a);
    let out_g = (src_g + div255_round(cur_g * inv)).min(out_a);
    let out_b = (src_b + div255_round(cur_b * inv)).min(out_a);
    *dst = (out_a << 24) | (out_r << 16) | (out_g << 8) | out_b;
}

fn collect_glyph_edges(
    ops: &[AnuratiSeg],
    origin_x: f32,
    origin_y: f32,
    em_scale: f32,
    edges: &mut Vec<(f32, f32, f32, f32)>,
) {
    edges.clear();
    let map_pt = |gx: f32, gy: f32| -> (f32, f32) {
        (
            origin_x + gx * em_scale,
            origin_y + (gy + ANURATI_CAP_HEIGHT_UNITS) * em_scale,
        )
    };
    let push_edge = |edges: &mut Vec<(f32, f32, f32, f32)>, a: (f32, f32), b: (f32, f32)| {
        if (a.1 - b.1).abs() > 1e-5 {
            edges.push((a.0, a.1, b.0, b.1));
        }
    };
    let mut start = (0.0f32, 0.0f32);
    let mut cur = (0.0f32, 0.0f32);
    let mut has_open = false;

    for &op in ops {
        match op {
            M(x, y) => {
                if has_open {
                    push_edge(edges, cur, start);
                }
                start = map_pt(x, y);
                cur = start;
                has_open = true;
            }
            L(x, y) => {
                let next = map_pt(x, y);
                push_edge(edges, cur, next);
                cur = next;
            }
            C(cx1, cy1, cx2, cy2, x, y) => {
                let p0 = cur;
                let p1 = map_pt(cx1, cy1);
                let p2 = map_pt(cx2, cy2);
                let p3 = map_pt(x, y);
                let steps = 8;
                let mut prev = p0;
                for step in 1..=steps {
                    let t = step as f32 / steps as f32;
                    let mt = 1.0 - t;
                    let mt2 = mt * mt;
                    let t2 = t * t;
                    let nx = mt2 * mt * p0.0
                        + 3.0 * mt2 * t * p1.0
                        + 3.0 * mt * t2 * p2.0
                        + t2 * t * p3.0;
                    let ny = mt2 * mt * p0.1
                        + 3.0 * mt2 * t * p1.1
                        + 3.0 * mt * t2 * p2.1
                        + t2 * t * p3.1;
                    push_edge(edges, prev, (nx, ny));
                    prev = (nx, ny);
                }
                cur = p3;
            }
            Z => {
                if has_open {
                    push_edge(edges, cur, start);
                    cur = start;
                    has_open = false;
                }
            }
        }
    }
    if has_open {
        push_edge(edges, cur, start);
    }
}

pub struct GlyphCanvas<'a> {
    pub buf: &'a mut [u32],
    pub width: i32,
    pub height: i32,
}

#[inline(always)]
fn blend_alpha_channel(dst: &mut u8, src_a: u8) {
    if src_a == 0 {
        return;
    }
    let s = u32::from(src_a);
    let d = u32::from(*dst);
    *dst = (s + div255_round(d * (255 - s))).min(255) as u8;
}

pub fn render_anurati_word(
    canvas: &mut GlyphCanvas<'_>,
    text: &str,
    top_y: f32,
    cap_height_px: f32,
    center_x: f32,
    color: u32,
) {
    let stride_px = canvas.width.max(0) as usize;
    let buf_h = canvas.height;
    if stride_px == 0 || buf_h <= 0 || cap_height_px <= 0.0 {
        return;
    }
    let total_units = measure_anurati_word_units(text);
    if total_units <= 0.0 {
        return;
    }
    let r = (color >> 16) & 255;
    let g = (color >> 8) & 255;
    let b = color & 255;

    ANURATI_WORD_CACHE.with(|slot| {
        let mut cache = slot.borrow_mut();
        let hit = cache.key.as_ref().is_some_and(|k| {
            k.width == canvas.width
                && k.height == buf_h
                && k.top_bits == top_y.to_bits()
                && k.cap_bits == cap_height_px.to_bits()
                && k.cx_bits == center_x.to_bits()
                && k.text == text
        });
        if !hit {
            let em_scale = cap_height_px / ANURATI_CAP_HEIGHT_UNITS;
            let sh_dx = (1.5 * (cap_height_px / 50.0)).round().clamp(1.0, 3.0) as i32;
            let sh_dy = (2.0 * (cap_height_px / 50.0)).round().clamp(1.0, 4.0) as i32;

            // Measure exact rendered vertical band so cache buffers hold only active rows.
            let mut pen_x = center_x - total_units * em_scale * 0.5;
            let mut word_y0 = buf_h;
            let mut word_y1 = 0i32;
            for ch in text.chars() {
                let Some(glyph) = anurati_glyph(ch) else {
                    continue;
                };
                collect_glyph_edges(glyph.ops, pen_x, top_y, em_scale, &mut cache.edges);
                pen_x += (glyph.advance + ANURATI_TRACKING_UNITS) * em_scale;
                if cache.edges.is_empty() {
                    continue;
                }
                let mut min_y = f32::MAX;
                let mut max_y = f32::MIN;
                for &(_, ay, _, by) in &cache.edges {
                    min_y = min_y.min(ay).min(by);
                    max_y = max_y.max(ay).max(by);
                }
                word_y0 = word_y0.min((min_y.floor() as i32 - 1).max(0));
                word_y1 = word_y1.max((max_y.ceil() as i32 + 1).min(buf_h));
            }
            if word_y0 >= word_y1 {
                cache.key = Some(AnuratiCacheKey {
                    text: text.to_string(),
                    width: canvas.width,
                    height: buf_h,
                    top_bits: top_y.to_bits(),
                    cap_bits: cap_height_px.to_bits(),
                    cx_bits: center_x.to_bits(),
                });
                cache.band_y0 = 0;
                cache.band_y1 = 0;
                cache.sh_buf.clear();
                cache.fg_buf.clear();
                return;
            }

            let band_y0 = word_y0.clamp(0, buf_h);
            let band_y1 = (word_y1 + sh_dy).clamp(band_y0, buf_h);
            let band_rows = (band_y1 - band_y0) as usize;
            let band_len = band_rows * stride_px;
            cache.band_y0 = band_y0;
            cache.band_y1 = band_y1;
            cache.sh_buf.resize(band_len, 0);
            cache.sh_buf.fill(0);
            cache.fg_buf.resize(band_len, 0);
            cache.fg_buf.fill(0);

            let c = &mut *cache;
            pen_x = center_x - total_units * em_scale * 0.5;
            for ch in text.chars() {
                let Some(glyph) = anurati_glyph(ch) else {
                    continue;
                };
                collect_glyph_edges(glyph.ops, pen_x, top_y, em_scale, &mut c.edges);
                pen_x += (glyph.advance + ANURATI_TRACKING_UNITS) * em_scale;
                if c.edges.is_empty() {
                    continue;
                }

                let mut min_x = f32::MAX;
                let mut max_x = f32::MIN;
                let mut min_y = f32::MAX;
                let mut max_y = f32::MIN;
                for &(ax, ay, bx, by) in &c.edges {
                    min_x = min_x.min(ax).min(bx);
                    max_x = max_x.max(ax).max(bx);
                    min_y = min_y.min(ay).min(by);
                    max_y = max_y.max(ay).max(by);
                }

                let x0 = (min_x.floor() as i32 - 1).max(0);
                let x1 = (max_x.ceil() as i32 + 1).min(stride_px as i32);
                let y0 = (min_y.floor() as i32 - 1).max(0);
                let y1 = (max_y.ceil() as i32 + 1).min(buf_h);
                if x0 >= x1 || y0 >= y1 {
                    continue;
                }
                let bw = (x1 - x0) as usize;
                let bh = (y1 - y0) as usize;
                c.cov_grid.clear();
                c.cov_grid.resize(bw * bh, 0);
                c.hits.clear();
                c.hits.resize(bw, 0u8);

                for row_idx in 0..bh {
                    let py = y0 + row_idx as i32;
                    c.hits.fill(0);
                    for sub_y in 0..4 {
                        let sy = py as f32 + (sub_y as f32 + 0.5) * 0.25;
                        c.crossings.clear();
                        for &(ax, ay, bx, by) in &c.edges {
                            let (lo, hi) = if ay < by { (ay, by) } else { (by, ay) };
                            if lo <= sy && sy < hi {
                                let ix = ax + (sy - ay) * (bx - ax) / (by - ay);
                                c.crossings.push(ix);
                            }
                        }
                        c.crossings.sort_unstable_by(f32::total_cmp);
                        for &[xa, xb] in c.crossings.as_chunks::<2>().0 {
                            let cx_start = ((xa - 0.125).floor() as i32).max(x0);
                            let cx_end = ((xb + 0.125).ceil() as i32).min(x1);
                            for px in cx_start..cx_end {
                                let col = (px - x0) as usize;
                                let mut h_count = 0u8;
                                for sub_x in 0..4 {
                                    let sx = px as f32 + (sub_x as f32 + 0.5) * 0.25;
                                    if sx >= xa && sx < xb {
                                        h_count += 1;
                                    }
                                }
                                c.hits[col] = c.hits[col].saturating_add(h_count);
                            }
                        }
                    }
                    let out_row = &mut c.cov_grid[row_idx * bw..(row_idx + 1) * bw];
                    for (out_px, &hit) in out_row.iter_mut().zip(&c.hits) {
                        if hit > 0 {
                            *out_px = ((u16::from(hit) * 255 + 8) / 16).min(255) as u8;
                        }
                    }
                }

                for row_idx in 0..bh {
                    let py = y0 + row_idx as i32 + sh_dy;
                    if py < band_y0 || py >= band_y1 {
                        continue;
                    }
                    let dst_row = (py - band_y0) as usize * stride_px;
                    let src_row = &c.cov_grid[row_idx * bw..(row_idx + 1) * bw];
                    for (col, &cov) in src_row.iter().enumerate().take(bw) {
                        if cov == 0 {
                            continue;
                        }
                        let px = x0 + col as i32 + sh_dx;
                        if px >= 0 && (px as usize) < stride_px {
                            let sh_a = ((u16::from(cov) * 76) / 255) as u8;
                            blend_alpha_channel(&mut c.sh_buf[dst_row + px as usize], sh_a);
                        }
                    }
                }
                for row_idx in 0..bh {
                    let py = y0 + row_idx as i32;
                    if py < band_y0 || py >= band_y1 {
                        continue;
                    }
                    let dst_row = (py - band_y0) as usize * stride_px;
                    let src_row = &c.cov_grid[row_idx * bw..(row_idx + 1) * bw];
                    for (col, &cov) in src_row.iter().enumerate().take(bw) {
                        if cov == 0 {
                            continue;
                        }
                        let px = x0 + col as i32;
                        if px >= 0 && (px as usize) < stride_px {
                            blend_alpha_channel(&mut c.fg_buf[dst_row + px as usize], cov);
                        }
                    }
                }
            }

            c.key = Some(AnuratiCacheKey {
                text: text.to_string(),
                width: canvas.width,
                height: buf_h,
                top_bits: top_y.to_bits(),
                cap_bits: cap_height_px.to_bits(),
                cx_bits: center_x.to_bits(),
            });
            // Day word only rasterizes once per day/resize; drop temporary geometry buffers immediately.
            c.release_build_scratch();
        }

        let band_y0 = cache.band_y0;
        let band_y1 = cache.band_y1;
        for py in band_y0..band_y1 {
            let src_row = (py - band_y0) as usize * stride_px;
            let dst_row = py as usize * stride_px;
            let sh_slice = &cache.sh_buf[src_row..src_row + stride_px];
            let fg_slice = &cache.fg_buf[src_row..src_row + stride_px];
            let dst_slice = &mut canvas.buf[dst_row..dst_row + stride_px];
            for ((dst_px, &sa), &fa) in dst_slice.iter_mut().zip(sh_slice).zip(fg_slice) {
                if sa != 0 {
                    premul_blend_pixel(dst_px, 0, 0, 0, sa);
                }
                if fa != 0 {
                    premul_blend_pixel(dst_px, r, g, b, fa);
                }
            }
        }
    });
}

fn draw_quicksand_line(
    canvas: &mut GlyphCanvas<'_>,
    text: &str,
    area: RECT,
    font_px: i32,
    char_extra: i32,
    color: u32,
) {
    let buf_w = canvas.width;
    let buf_h = canvas.height;
    let rw = (area.right - area.left).max(0);
    let rh = (area.bottom - area.top).max(0);
    if rw <= 0 || rh <= 0 || buf_w <= 0 || buf_h <= 0 || text.is_empty() {
        return;
    }
    let mut stack = [0u16; 64];
    let mut heap = Vec::new();
    let wide_slice: &mut [u16] = {
        let mut len = 0usize;
        let mut overflow = false;
        for unit in text.encode_utf16() {
            if len == stack.len() {
                overflow = true;
                break;
            }
            stack[len] = unit;
            len += 1;
        }
        if overflow {
            heap.extend_from_slice(&stack);
            heap.extend(text.encode_utf16().skip(stack.len()));
            &mut heap
        } else {
            &mut stack[..len]
        }
    };
    if wide_slice.is_empty() {
        return;
    }

    let font_raw = CLOCK_FONT_CACHE.with(|fc| fc.borrow_mut().get_or_create(font_px));

    CLOCK_TEXT_SCRATCH.with(|slot| {
        let mut scratch = slot.borrow_mut();
        let Some((dc, bits_ptr)) = scratch.ensure(rw, rh) else {
            return;
        };
        if bits_ptr.is_null() {
            return;
        }
        let Some(total) = (rw as usize)
            .checked_mul(rh as usize)
            .filter(|&n| n <= isize::MAX as usize / std::mem::size_of::<u32>())
        else {
            return;
        };
        // SAFETY: scratch DIB has verified non-null rw * rh 32-bit pixels owned by this thread.
        unsafe {
            std::slice::from_raw_parts_mut(bits_ptr, total).fill(0);
            let prev_font =
                (font_raw != 0).then(|| SelectObject(dc, HGDIOBJ(font_raw as *mut c_void)));
            let prev_extra = SetTextCharacterExtra(dc, char_extra.max(0));
            let _ = SetBkMode(dc, TRANSPARENT);
            let _ = SetTextColor(dc, COLORREF(0x00FF_FFFF));
            let mut local_rect = RECT {
                left: 0,
                top: 0,
                right: rw,
                bottom: rh,
            };
            let _ = DrawTextW(
                dc,
                wide_slice,
                &mut local_rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            let _ = GdiFlush();
            SetTextCharacterExtra(dc, prev_extra);
            if let Some(prev) = prev_font {
                let _ = SelectObject(dc, prev);
            }
        }

        // SAFETY: scratch DIB pixels remain valid after GdiFlush.
        let src = unsafe { std::slice::from_raw_parts(bits_ptr, total) };
        let r = (color >> 16) & 255;
        let g = (color >> 8) & 255;
        let b = color & 255;
        let stride = buf_w as usize;
        let rw_u = rw as usize;

        for sy in 0..rh {
            let dy = area.top + sy + 2;
            if dy < 0 || dy >= buf_h {
                continue;
            }
            let src_row = &src[sy as usize * rw_u..(sy as usize + 1) * rw_u];
            let dx0 = (area.left + 1).max(0);
            let dx1 = (area.left + 1 + rw).min(buf_w);
            if dx0 >= dx1 {
                continue;
            }
            let sx0 = (dx0 - (area.left + 1)) as usize;
            let count = (dx1 - dx0) as usize;
            let dst_off = dy as usize * stride + dx0 as usize;
            for (&spx, dpx) in src_row[sx0..sx0 + count]
                .iter()
                .zip(&mut canvas.buf[dst_off..dst_off + count])
            {
                let cov = (spx >> 16) & 255;
                if cov != 0 {
                    let sh_a = ((cov * 84) / 255) as u8;
                    premul_blend_pixel(dpx, 0, 0, 0, sh_a);
                }
            }
        }
        for sy in 0..rh {
            let dy = area.top + sy;
            if dy < 0 || dy >= buf_h {
                continue;
            }
            let src_row = &src[sy as usize * rw_u..(sy as usize + 1) * rw_u];
            let dx0 = area.left.max(0);
            let dx1 = (area.left + rw).min(buf_w);
            if dx0 >= dx1 {
                continue;
            }
            let sx0 = (dx0 - area.left) as usize;
            let count = (dx1 - dx0) as usize;
            let dst_off = dy as usize * stride + dx0 as usize;
            for (&spx, dpx) in src_row[sx0..sx0 + count]
                .iter()
                .zip(&mut canvas.buf[dst_off..dst_off + count])
            {
                let cov = (spx >> 16) & 255;
                if cov != 0 {
                    premul_blend_pixel(dpx, r, g, b, cov as u8);
                }
            }
        }
    });
}

fn draw_corner_brackets(buf: &mut [u32], stride: i32, r: RECT, corner: i32, col: u32) {
    let rw = r.right - r.left;
    let rh = r.bottom - r.top;
    if rw <= 8 || rh <= 8 {
        return;
    }
    let c = corner.min(rw / 4).min(rh / 4);
    let wu = stride as usize;
    for t in 0..2 {
        for x in 0..c {
            buf[(r.top + t) as usize * wu + (r.left + x) as usize] = col;
            buf[(r.top + t) as usize * wu + (r.right - 1 - x) as usize] = col;
            buf[(r.bottom - 1 - t) as usize * wu + (r.left + x) as usize] = col;
            buf[(r.bottom - 1 - t) as usize * wu + (r.right - 1 - x) as usize] = col;
        }
        for y in 0..c {
            buf[(r.top + y) as usize * wu + (r.left + t) as usize] = col;
            buf[(r.top + y) as usize * wu + (r.right - 1 - t) as usize] = col;
            buf[(r.bottom - 1 - y) as usize * wu + (r.left + t) as usize] = col;
            buf[(r.bottom - 1 - y) as usize * wu + (r.right - 1 - t) as usize] = col;
        }
    }
}

fn draw_reposition_guide(buf: &mut [u32], w: i32, h: i32) {
    if w <= 8 || h <= 8 {
        return;
    }
    // Outer bounds brackets + subtle center grab-handle hint matching drag_handle_rect.
    draw_corner_brackets(
        buf,
        w,
        RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        },
        18,
        0xD8F3_EEE3,
    );
    draw_corner_brackets(buf, w, drag_handle_rect(w, h), 12, 0x90D9_541E);
}

fn apply_opacity_in_place(buf: &mut [u32], opacity_pct: u32) {
    let pct = opacity_pct.clamp(20, 100);
    if pct >= 100 {
        return;
    }
    let scale = (pct * 255 + 50) / 100;
    for px in buf {
        let cur = *px;
        let a = (cur >> 24) & 255;
        if a == 0 {
            continue;
        }
        let r = (cur >> 16) & 255;
        let g = (cur >> 8) & 255;
        let b = cur & 255;
        let na = div255_round(a * scale);
        let nr = div255_round(r * scale).min(na);
        let ng = div255_round(g * scale).min(na);
        let nb = div255_round(b * scale).min(na);
        *px = (na << 24) | (nr << 16) | (ng << 8) | nb;
    }
}

pub fn apply_hit_alpha_in_place(buf: &mut [u32], interactive: bool) {
    if !interactive {
        return;
    }
    // Rainmeter SolidColor=0,0,0,1 equivalent: invisible alpha=1 makes center grab & menu reliable.
    for px in buf {
        if (*px >> 24) == 0 {
            *px = 0x0100_0000;
        }
    }
}

fn update_layered_clock_surface(hwnd: HWND, dst: POINT, size: SIZE, mem_dc: HDC) -> bool {
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    // SAFETY: flushes finished DIB pixels and commits them atomically to the WS_EX_LAYERED HWND.
    unsafe {
        let _ = GdiFlush();
        let screen_dc = GetDC(None);
        let res = UpdateLayeredWindow(
            hwnd,
            screen_dc,
            Some(&dst),
            Some(&size),
            mem_dc,
            Some(&src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        if !screen_dc.0.is_null() {
            let _ = ReleaseDC(None, screen_dc);
        }
        res.is_ok()
    }
}

fn render_and_place(hwnd: HWND) {
    render_and_place_mode(hwnd, false);
}

fn render_and_place_mode(hwnd: HWND, live_preview: bool) {
    let snapshot = CLOCK_RUNTIME.with(|slot| {
        slot.borrow().as_ref().map(|r| {
            (
                r.owner,
                r.cfg,
                r.behind_attached,
                r.cached_layer,
                r.repositioning,
                r.last_bounds,
                r.dcomp.is_some(),
                r.width,
                r.height,
                r.input.is_some(),
            )
        })
    });
    let Some((
        owner_raw,
        cfg,
        behind_attached,
        cached_layer,
        repositioning,
        last_bounds,
        has_dcomp,
        prev_w,
        prev_h,
        has_input,
    )) = snapshot
    else {
        return;
    };
    let owner = HWND(owner_raw as *mut c_void);
    let now = unsafe { local_time() };
    let (day_str, date_str, time_str) = format_clock_lines(
        now.wYear,
        now.wMonth,
        now.wDayOfWeek,
        now.wDay,
        now.wHour,
        now.wMinute,
        cfg.use_24h,
    );
    let (est_w, est_h) = widget_pixel_size(&day_str, cfg.size_pct, 96);
    let (work, dpi) =
        target_monitor_info(hwnd, owner, cfg.pos, (prev_w.max(est_w), prev_h.max(est_h)));
    let (w, h) = widget_pixel_size(&day_str, cfg.size_pct, dpi);
    let (sx, sy) = resolve_screen_position(cfg.pos, work, w, h);

    let mut bounds_changed = false;
    // SAFETY: reuses thread-local offscreen DC and 32-bit DIBSection strictly for this synchronous frame update.
    CLOCK_CANVAS_SCRATCH.with(|canvas_slot| {
        let mut scratch = canvas_slot.borrow_mut();
        let Some((mem_dc, bits)) = scratch.ensure(w, h) else {
            return;
        };
        if bits.is_null() || w <= 0 || h <= 0 {
            return;
        }
        let Some(total) = (w as usize)
            .checked_mul(h as usize)
            .filter(|&n| n <= isize::MAX as usize / std::mem::size_of::<u32>())
        else {
            return;
        };
        unsafe {
            let buf = std::slice::from_raw_parts_mut(bits, total);
            buf.fill(0);

            let scale = (cfg.size_pct.clamp(50, 200) as f32 / 100.0) * (dpi as f32 / 96.0);
            {
                let mut canvas = GlyphCanvas {
                    buf,
                    width: w,
                    height: h,
                };
                render_anurati_word(
                    &mut canvas,
                    &day_str,
                    16.0 * scale,
                    50.0 * scale,
                    w as f32 * 0.5,
                    cfg.color,
                );
                draw_quicksand_line(
                    &mut canvas,
                    &date_str,
                    RECT {
                        left: 8,
                        top: (84.0 * scale).round() as i32,
                        right: w - 8,
                        bottom: (118.0 * scale).round() as i32,
                    },
                    (21.0 * scale).round() as i32,
                    (1.5 * scale).round() as i32,
                    cfg.color,
                );
                draw_quicksand_line(
                    &mut canvas,
                    &time_str,
                    RECT {
                        left: 8,
                        top: (126.0 * scale).round() as i32,
                        right: w - 8,
                        bottom: (160.0 * scale).round() as i32,
                    },
                    (20.0 * scale).round() as i32,
                    (1.2 * scale).round() as i32,
                    cfg.color,
                );
            }
            apply_opacity_in_place(buf, cfg.opacity_pct);
            apply_hit_alpha_in_place(
                buf,
                !behind_attached && !is_click_through(cfg, repositioning),
            );
            if repositioning {
                draw_reposition_guide(buf, w, h);
            }

            let mut dst = POINT { x: sx, y: sy };
            if behind_attached && let Some(l) = cached_layer.filter(|&l| layer_still_valid(l)) {
                let mut pts = [dst];
                MapWindowPoints(None, l.parent, &mut pts);
                dst = pts[0];
            }

            let next_bounds = (dst.x, dst.y, w, h, repositioning, behind_attached);
            let place_needed = last_bounds != Some(next_bounds) || !IsWindowVisible(hwnd).as_bool();
            bounds_changed = place_needed;
            if has_dcomp {
                if place_needed {
                    if behind_attached
                        && let Some(l) = cached_layer.filter(|&l| layer_still_valid(l))
                    {
                        let after = if l.raised { l.icons } else { HWND_TOP };
                        let _z_guard = ZOrderSyncGuard::enter();
                        let _ = SetWindowPos(
                            hwnd,
                            after,
                            dst.x,
                            dst.y,
                            w,
                            h,
                            SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        );
                    } else {
                        place_popup_on_desktop(hwnd, dst.x, dst.y, w, h, true, repositioning);
                    }
                }
                let ok = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow_mut()
                        .as_mut()
                        .and_then(|rt| rt.dcomp.as_mut())
                        .is_some_and(|dc| dc.present_bgra(buf, w as u32, h as u32))
                });
                if !ok {
                    // Drop the old IDCompositionTarget and commit before recreating on the same HWND
                    // so CreateTargetForHwnd does not fail with DCOMPOSITION_ERROR_WINDOW_ALREADY_COMPOSED (0x88980801).
                    let old_dcomp = CLOCK_RUNTIME
                        .with(|slot| slot.borrow_mut().as_mut().and_then(|rt| rt.dcomp.take()));
                    drop(old_dcomp);
                    let rebuilt = DCompPresenter::new(hwnd, w as u32, h as u32).map(|mut dc| {
                        let _ = dc.present_bgra(buf, w as u32, h as u32);
                        dc
                    });
                    CLOCK_RUNTIME.with(|slot| {
                        if let Some(rt) = slot.borrow_mut().as_mut() {
                            rt.dcomp = rebuilt;
                        }
                    });
                }
            } else {
                let was_visible = IsWindowVisible(hwnd).as_bool();
                let size = SIZE { cx: w, cy: h };
                let first_ok = update_layered_clock_surface(hwnd, dst, size, mem_dc);
                if place_needed {
                    if behind_attached
                        && let Some(l) = cached_layer.filter(|&l| layer_still_valid(l))
                    {
                        let after = if l.raised { l.icons } else { HWND_TOP };
                        let _z_guard = ZOrderSyncGuard::enter();
                        let _ = SetWindowPos(
                            hwnd,
                            after,
                            dst.x,
                            dst.y,
                            w,
                            h,
                            SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        );
                    } else {
                        place_popup_on_desktop(hwnd, dst.x, dst.y, w, h, !first_ok, repositioning);
                    }
                    if !was_visible || !first_ok {
                        let _ = update_layered_clock_surface(hwnd, dst, size, mem_dc);
                    }
                }
            }

            CLOCK_RUNTIME.with(|slot| {
                if let Some(state) = slot.borrow_mut().as_mut() {
                    state.width = w;
                    state.height = h;
                    state.last_bounds = Some(next_bounds);
                    state.last_stamp = (
                        now.wYear,
                        now.wMonth,
                        now.wDayOfWeek,
                        now.wDay,
                        now.wHour,
                        now.wMinute,
                    );
                }
            });
            let _ = SetTimer(
                hwnd,
                CLOCK_TIMER_ID,
                minute_timer_delay_ms(now.wSecond),
                None,
            );
        }
    });
    let want_input = behind_attached && !cfg.locked && !repositioning;
    let input_mismatch = want_input != has_input;
    if bounds_changed || input_mismatch {
        sync_clock_input_mode(!live_preview);
    }
    // Between minute ticks (60s idle), release offscreen DIB scratch + trim DXGI temporary memory.
    if !live_preview {
        trim_transient_caches();
    }
}

fn build_context_menu_items(cfg: ClockConfig, repositioning: bool) -> Vec<(usize, &'static str)> {
    let lock_label = if cfg.locked {
        "Lock position (On)"
    } else {
        "Lock position (Off)"
    };
    let icons_label = if cfg.behind_icons {
        "Behind desktop shortcuts (On)"
    } else {
        "Behind desktop shortcuts (Off)"
    };
    let fmt_label = if cfg.use_24h {
        "24-hour clock (On)"
    } else {
        "12-hour clock (AM/PM)"
    };
    let mut items: Vec<(usize, &str)> = vec![
        (CTX_LOCAL_SIZE_UP, "Bigger (Scroll up)"),
        (CTX_LOCAL_SIZE_DOWN, "Smaller (Scroll down)"),
        (CLOCK_CTX_PICK_COLOR, "Pick colour from screen..."),
        (CLOCK_CTX_24H, fmt_label),
        (CLOCK_CTX_LOCK, lock_label),
        (CLOCK_CTX_BEHIND_ICONS, icons_label),
    ];
    if repositioning {
        items.push((CTX_LOCAL_DONE_REPOS, "Done positioning"));
    }
    items.push((CLOCK_CTX_OPEN_SETTINGS, "Widget settings"));
    items.push((CLOCK_CTX_REMOVE, "Remove from desktop"));
    items
}

fn show_context_menu(hwnd: HWND, screen_pt: POINT) {
    let (owner, action_msg, cfg, repositioning) = match CLOCK_RUNTIME.with(|s| {
        s.borrow().as_ref().map(|r| {
            (
                HWND(r.owner as *mut c_void),
                r.action_msg,
                r.cfg,
                r.repositioning,
            )
        })
    }) {
        Some(v) => v,
        None => return,
    };
    let items = build_context_menu_items(cfg, repositioning);

    let target_owner = if !owner.0.is_null()
        && unsafe { IsWindow(owner).as_bool() && IsWindowVisible(owner).as_bool() }
    {
        owner
    } else {
        hwnd
    };
    if let Ok(choice) = tray::popup_at(target_owner, screen_pt, &items, None)
        && choice != 0
    {
        if choice == CTX_LOCAL_DONE_REPOS {
            if !owner.0.is_null() {
                unsafe {
                    let _ = PostMessageW(
                        owner,
                        action_msg,
                        WPARAM(CLOCK_REPOSITION),
                        LPARAM(crate::app::internal_action_token()),
                    );
                }
            } else {
                end_reposition(owner, action_msg, cfg);
            }
            return;
        }
        if choice == CLOCK_CTX_PICK_COLOR {
            begin_color_pick(owner, action_msg, cfg.color);
            return;
        }
        if owner.0.is_null() {
            return;
        }
        if choice == CTX_LOCAL_SIZE_UP {
            CLOCK_WHEEL_STEPS.with(|s| *s.borrow_mut() += 2);
            unsafe {
                let _ = PostMessageW(
                    owner,
                    action_msg,
                    WPARAM(CLOCK_CTX_SIZE_DELTA),
                    LPARAM(crate::app::internal_action_token()),
                );
            }
        } else if choice == CTX_LOCAL_SIZE_DOWN {
            CLOCK_WHEEL_STEPS.with(|s| *s.borrow_mut() -= 2);
            unsafe {
                let _ = PostMessageW(
                    owner,
                    action_msg,
                    WPARAM(CLOCK_CTX_SIZE_DELTA),
                    LPARAM(crate::app::internal_action_token()),
                );
            }
        } else {
            unsafe {
                let _ = PostMessageW(
                    owner,
                    action_msg,
                    WPARAM(choice),
                    LPARAM(crate::app::internal_action_token()),
                );
            }
        }
    }
}

unsafe extern "system" fn clock_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: interacts strictly with thread-local CLOCK_RUNTIME on the UI thread.
    unsafe {
        match msg {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_QUERYOPEN => LRESULT(1),
            WM_SYSCOMMAND => {
                let cmd = (wparam.0 & 0xFFF0) as u32;
                if cmd == SC_MINIMIZE || cmd == SC_MAXIMIZE {
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_SIZE if wparam.0 == SIZE_MINIMIZED as usize => {
                restore_if_iconic(hwnd);
                ensure_desktop_visible();
                LRESULT(0)
            }
            WM_WINDOWPOSCHANGING => {
                if lparam.0 != 0 {
                    let pos = &mut *(lparam.0 as *mut WINDOWPOS);
                    if !ALLOW_INTERNAL_HIDE.with(Cell::get) {
                        pos.flags.0 &= !SWP_HIDEWINDOW.0;
                    }
                    if !in_zorder_sync() {
                        // Never allow minimize-park offscreen coordinates (-32000, -32000).
                        if (pos.flags.0 & SWP_NOMOVE.0 == 0) && (pos.x <= -30000 || pos.y <= -30000)
                        {
                            pos.flags |= SWP_NOMOVE | SWP_NOSIZE;
                        }
                        if (pos.flags.0 & SWP_NOZORDER.0 == 0)
                            && !is_repositioning()
                            && !CLOCK_RUNTIME
                                .with(|s| s.borrow().as_ref().is_some_and(|r| r.behind_attached))
                        {
                            // Exact Rainmeter Skin::OnWindowPosChanging rule: keep OS/DWM from bumping z-order.
                            pos.flags |= SWP_NOZORDER;
                        }
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_NCHITTEST => {
                let sx = (lparam.0 as i32) << 16 >> 16;
                let sy = (lparam.0 as i32) >> 16;
                let mut pt = POINT { x: sx, y: sy };
                let _ = ScreenToClient(hwnd, &mut pt);
                let hit = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow().as_ref().is_some_and(|rt| {
                        if rt.behind_attached || is_click_through(rt.cfg, rt.repositioning) {
                            return false;
                        }
                        if rt.repositioning {
                            return true;
                        }
                        // Ignore extreme outer blank margin so adjacent icons stay untouched.
                        let mx = rt.width * 12 / 100;
                        let my = rt.height * 12 / 100;
                        pt.x >= mx && pt.x < rt.width - mx && pt.y >= my && pt.y < rt.height - my
                    })
                });
                if hit {
                    LRESULT(HTCLIENT as isize)
                } else {
                    LRESULT(HTTRANSPARENT as isize)
                }
            }
            WM_SETCURSOR => {
                let mut cur = POINT::default();
                let _ = GetCursorPos(&mut cur);
                let _ = ScreenToClient(hwnd, &mut cur);
                let can_move = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow().as_ref().is_some_and(|rt| {
                        (!rt.cfg.locked || rt.repositioning)
                            && in_drag_handle(cur.x, cur.y, rt.width, rt.height)
                    })
                });
                let cursor = if can_move { IDC_SIZEALL } else { IDC_ARROW };
                if let Ok(h) = LoadCursorW(None, cursor) {
                    SetCursor(h);
                }
                LRESULT(1)
            }
            WM_TIMER if wparam.0 == CLOCK_ZORDER_SETTLE_TIMER_ID => {
                let _ = KillTimer(hwnd, CLOCK_ZORDER_SETTLE_TIMER_ID);
                ensure_desktop_visible();
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == CLOCK_ZORDER_LATE_TIMER_ID => {
                let _ = KillTimer(hwnd, CLOCK_ZORDER_LATE_TIMER_ID);
                ensure_desktop_visible();
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == CLOCK_TIMER_ID => {
                let now = local_time();
                let changed = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow().as_ref().is_some_and(|rt| {
                        rt.last_stamp
                            != (
                                now.wYear,
                                now.wMonth,
                                now.wDayOfWeek,
                                now.wDay,
                                now.wHour,
                                now.wMinute,
                            )
                    })
                });
                if changed {
                    render_and_place(hwnd);
                } else {
                    let _ = SetTimer(
                        hwnd,
                        CLOCK_TIMER_ID,
                        minute_timer_delay_ms(now.wSecond),
                        None,
                    );
                }
                LRESULT(0)
            }
            WM_NCDESTROY => {
                let old = CLOCK_RUNTIME.with(|slot| {
                    let mut borrow = slot.borrow_mut();
                    if borrow.as_ref().is_some_and(|rt| rt.hwnd == hwnd.0 as isize) {
                        borrow.take()
                    } else {
                        None
                    }
                });
                if let Some(rt) = old {
                    unhook_events(rt.hooks);
                }
                CLOCK_TEXT_SCRATCH.with(|s| s.borrow_mut().free());
                CLOCK_CANVAS_SCRATCH.with(|s| s.borrow_mut().free());
                CLOCK_FONT_CACHE.with(|s| s.borrow_mut().free());
                ANURATI_WORD_CACHE.with(|s| s.borrow_mut().free());
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_LBUTTONDOWN => {
                let mut cur = POINT::default();
                let _ = GetCursorPos(&mut cur);
                let mut client_pt = cur;
                let _ = ScreenToClient(hwnd, &mut client_pt);
                let allow_drag = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow().as_ref().is_some_and(|rt| {
                        (!rt.cfg.locked || rt.repositioning)
                            && in_drag_handle(client_pt.x, client_pt.y, rt.width, rt.height)
                    })
                });
                if !allow_drag {
                    return LRESULT(0);
                }
                let mut wr = RECT::default();
                let _ = GetWindowRect(hwnd, &mut wr);
                CLOCK_RUNTIME.with(|slot| {
                    if let Some(rt) = slot.borrow_mut().as_mut() {
                        rt.dragging = true;
                        rt.drag_moved = false;
                        rt.drag_start_x = cur.x;
                        rt.drag_start_y = cur.y;
                        rt.drag_offset_x = cur.x - wr.left;
                        rt.drag_offset_y = cur.y - wr.top;
                    }
                });
                if !CLOCK_RUNTIME.with(|s| s.borrow().as_ref().is_some_and(|r| r.behind_attached)) {
                    let _ = SetCapture(hwnd);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let drag = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow().as_ref().filter(|rt| rt.dragging).map(|rt| {
                        (
                            rt.drag_start_x,
                            rt.drag_start_y,
                            rt.drag_offset_x,
                            rt.drag_offset_y,
                        )
                    })
                });
                if let Some((sx, sy, ox, oy)) = drag {
                    let mut cur = POINT::default();
                    let _ = GetCursorPos(&mut cur);
                    if (cur.x - sx).abs() > 2 || (cur.y - sy).abs() > 2 {
                        CLOCK_RUNTIME.with(|slot| {
                            if let Some(rt) = slot.borrow_mut().as_mut() {
                                rt.drag_moved = true;
                            }
                        });
                    }
                    let mut origin = [POINT {
                        x: cur.x - ox,
                        y: cur.y - oy,
                    }];
                    if GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & WS_CHILD.0 != 0
                        && let Ok(parent) = GetParent(hwnd)
                    {
                        MapWindowPoints(None, parent, &mut origin);
                    }
                    let _ = SetWindowPos(
                        hwnd,
                        HWND::default(),
                        origin[0].x,
                        origin[0].y,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (moved, owner, action_msg) = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow_mut().as_mut().map_or((false, 0, 0), |rt| {
                        let m = rt.dragging && rt.drag_moved;
                        rt.dragging = false;
                        rt.drag_moved = false;
                        (m, rt.owner, rt.action_msg)
                    })
                });
                if GetCapture() == hwnd {
                    let _ = ReleaseCapture();
                }
                if moved {
                    let mut wr = RECT::default();
                    if GetWindowRect(hwnd, &mut wr).is_ok() {
                        CLOCK_RUNTIME.with(|s| {
                            if let Some(rt) = s.borrow_mut().as_mut() {
                                rt.cfg.pos = Some((wr.left, wr.top));
                                rt.last_bounds = None;
                            }
                        });
                        CLOCK_SAVED_POS.with(|s| *s.borrow_mut() = Some((wr.left, wr.top)));
                        if owner != 0 {
                            let _ = PostMessageW(
                                HWND(owner as *mut c_void),
                                action_msg,
                                WPARAM(CLOCK_CTX_POS_SAVED),
                                LPARAM(crate::app::internal_action_token()),
                            );
                        }
                    }
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam.0 >> 16) as i16) as i32;
                let (allowed, owner, action_msg) = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow_mut().as_mut().map_or((false, 0, 0), |rt| {
                        // Lock position disables accidental drag & scroll-resizing; repositioning unlocks both.
                        let ok = !rt.cfg.locked || rt.repositioning;
                        if ok {
                            rt.pending_wheel_delta += delta;
                            let steps = rt.pending_wheel_delta / 120;
                            rt.pending_wheel_delta %= 120;
                            if steps != 0 {
                                CLOCK_WHEEL_STEPS.with(|s| *s.borrow_mut() += steps);
                                return (true, rt.owner, rt.action_msg);
                            }
                        }
                        (false, 0, 0)
                    })
                });
                if allowed && owner != 0 {
                    let _ = PostMessageW(
                        HWND(owner as *mut c_void),
                        action_msg,
                        WPARAM(CLOCK_CTX_SIZE_DELTA),
                        LPARAM(crate::app::internal_action_token()),
                    );
                }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                let mut cur = POINT::default();
                let _ = GetCursorPos(&mut cur);
                show_context_menu(hwnd, cur);
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let vk = wparam.0 as u16;
                let info = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .filter(|rt| rt.repositioning)
                        .map(|rt| (rt.owner, rt.action_msg, rt.cfg))
                });
                if let Some((owner, action_msg, cfg)) = info {
                    if vk == VK_ESCAPE.0 || vk == VK_RETURN.0 {
                        if owner != 0 {
                            let _ = PostMessageW(
                                HWND(owner as *mut c_void),
                                action_msg,
                                WPARAM(CLOCK_REPOSITION),
                                LPARAM(crate::app::internal_action_token()),
                            );
                        } else {
                            end_reposition(HWND::default(), action_msg, cfg);
                        }
                        return LRESULT(0);
                    }
                    let step = if GetKeyState(VK_SHIFT.0 as i32) < 0 {
                        1
                    } else {
                        10
                    };
                    let (dx, dy) = match vk {
                        x if x == VK_LEFT.0 => (-step, 0),
                        x if x == VK_RIGHT.0 => (step, 0),
                        x if x == VK_UP.0 => (0, -step),
                        x if x == VK_DOWN.0 => (0, step),
                        _ => (0, 0),
                    };
                    if dx != 0 || dy != 0 {
                        let mut wr = RECT::default();
                        if GetWindowRect(hwnd, &mut wr).is_ok() {
                            let nx = wr.left + dx;
                            let ny = wr.top + dy;
                            let _ = SetWindowPos(
                                hwnd,
                                HWND::default(),
                                nx,
                                ny,
                                0,
                                0,
                                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                            );
                            CLOCK_RUNTIME.with(|s| {
                                if let Some(rt) = s.borrow_mut().as_mut() {
                                    rt.cfg.pos = Some((nx, ny));
                                    rt.last_bounds = None;
                                }
                            });
                            CLOCK_SAVED_POS.with(|s| *s.borrow_mut() = Some((nx, ny)));
                            if owner != 0 {
                                let _ = PostMessageW(
                                    HWND(owner as *mut c_void),
                                    action_msg,
                                    WPARAM(CLOCK_CTX_POS_SAVED),
                                    LPARAM(crate::app::internal_action_token()),
                                );
                            }
                        }
                        return LRESULT(0);
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

// --- Screen-wide Eyedropper Color Picker (Pick colour from anywhere on screen) ---

#[derive(Default)]
struct ColorPickState {
    picker_hwnd: isize,
    loupe_hwnd: isize,
    owner: isize,
    action_msg: u32,
    original_color: u32,
    last_color: u32,
    sample_dc: isize,
    sample_bmp: isize,
    sample_bits: usize,
    loupe_dc: isize,
    loupe_bmp: isize,
    loupe_bits: usize,
    loupe_dpi: i32,
    loupe_w: i32,
    loupe_h: i32,
    closing: bool,
}

const SAMPLE_SIDE: i32 = 9;
const LOUPE_BASE_W: i32 = 152;
const LOUPE_BASE_H: i32 = 62;

fn monitor_dpi_for_point(pt: POINT) -> i32 {
    // SAFETY: read-only monitor DPI lookup for High-DPI loupe scaling.
    unsafe {
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut dx = 96u32;
        let mut dy = 96u32;
        if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_ok() && dx >= 96 {
            (dx as i32).clamp(96, 384)
        } else {
            96
        }
    }
}

fn loupe_dims_for_dpi(dpi: i32) -> (i32, i32) {
    let d = dpi.clamp(96, 384);
    (
        (LOUPE_BASE_W * d / 96).max(LOUPE_BASE_W),
        (LOUPE_BASE_H * d / 96).max(LOUPE_BASE_H),
    )
}

unsafe fn create_loupe_dib(
    w: i32,
    h: i32,
) -> Option<(HDC, windows::Win32::Graphics::Gdi::HBITMAP, *mut c_void)> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.0.is_null() {
            return None;
        }
        let mut bits = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(
            dc,
            &BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
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
        ) else {
            let _ = DeleteDC(dc);
            return None;
        };
        if bits.is_null() {
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(dc);
            return None;
        }
        let _ = SelectObject(dc, HGDIOBJ(bmp.0));
        Some((dc, bmp, bits))
    }
}

unsafe extern "system" fn loupe_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: delegates loupe messages directly to default Win32 handler.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

pub fn begin_color_pick(owner: HWND, action_msg: u32, current_color: u32) {
    cancel_color_pick();
    let black_brush = unsafe { GetStockObject(BLACK_BRUSH) };
    if ensure_class(PICKER_CLASS, Some(picker_proc), black_brush, IDC_CROSS).is_err()
        || ensure_class(LOUPE_CLASS, Some(loupe_proc), HGDIOBJ::default(), IDC_CROSS).is_err()
    {
        return;
    }
    ensure_quicksand_loaded();
    let Ok(instance) = (unsafe { GetModuleHandleW(None) }) else {
        return;
    };
    let mut init_pt = POINT::default();
    let _ = unsafe { GetCursorPos(&mut init_pt) };
    let init_dpi = monitor_dpi_for_point(init_pt);
    let (init_lw, init_lh) = loupe_dims_for_dpi(init_dpi);
    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
        )
    };
    // SAFETY: lightweight LWA_ALPHA=1 overlay consumes 0 KB backing bitmap while intercepting clicks across all monitors.
    unsafe {
        let Ok(picker) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            PICKER_CLASS,
            w!("WallpaperLive Color Picker"),
            WS_POPUP,
            vx,
            vy,
            vw,
            vh,
            None,
            None,
            instance,
            None,
        ) else {
            return;
        };
        let _ = SetLayeredWindowAttributes(picker, COLORREF(0), 1, LWA_ALPHA);

        let Ok(loupe) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            LOUPE_CLASS,
            w!("WallpaperLive Color Loupe"),
            WS_POPUP,
            0,
            0,
            init_lw,
            init_lh,
            None,
            None,
            instance,
            None,
        ) else {
            let _ = DestroyWindow(picker);
            return;
        };

        let sample_dc = CreateCompatibleDC(None);
        let mut s_bits = std::ptr::null_mut();
        let sample_bmp = if !sample_dc.0.is_null() {
            CreateDIBSection(
                sample_dc,
                &BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: SAMPLE_SIDE,
                        biHeight: -SAMPLE_SIDE,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                DIB_RGB_COLORS,
                &mut s_bits,
                None,
                0,
            )
            .ok()
        } else {
            None
        };
        let Some(sb) = sample_bmp else {
            if !sample_dc.0.is_null() {
                let _ = DeleteDC(sample_dc);
            }
            let _ = DestroyWindow(loupe);
            let _ = DestroyWindow(picker);
            return;
        };
        if s_bits.is_null() {
            let _ = DeleteObject(sb);
            let _ = DeleteDC(sample_dc);
            let _ = DestroyWindow(loupe);
            let _ = DestroyWindow(picker);
            return;
        }
        let _ = SelectObject(sample_dc, HGDIOBJ(sb.0));

        let Some((loupe_dc, lb, l_bits)) = create_loupe_dib(init_lw, init_lh) else {
            let _ = DeleteDC(sample_dc);
            let _ = DeleteObject(HGDIOBJ(sb.0));
            let _ = DestroyWindow(loupe);
            let _ = DestroyWindow(picker);
            return;
        };

        COLOR_PICK_SESSION.with(|slot| {
            *slot.borrow_mut() = Some(ColorPickState {
                picker_hwnd: picker.0 as isize,
                loupe_hwnd: loupe.0 as isize,
                owner: owner.0 as isize,
                action_msg,
                original_color: current_color & 0x00FF_FFFF,
                last_color: current_color & 0x00FF_FFFF,
                sample_dc: sample_dc.0 as isize,
                sample_bmp: sb.0 as isize,
                sample_bits: s_bits as usize,
                loupe_dc: loupe_dc.0 as isize,
                loupe_bmp: lb.0 as isize,
                loupe_bits: l_bits as usize,
                loupe_dpi: init_dpi,
                loupe_w: init_lw,
                loupe_h: init_lh,
                closing: false,
            });
        });

        let _ = ShowWindow(picker, SW_SHOWNA);
        let _ = SetWindowPos(
            picker,
            HWND_TOPMOST,
            vx,
            vy,
            vw,
            vh,
            SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
        let _ = SetForegroundWindow(picker);
        let _ = SetFocus(picker);
        let _ = SetCapture(picker);

        update_picker_loupe(init_pt);
    }
}

pub fn cancel_color_pick() {
    let old = COLOR_PICK_SESSION.with(|slot| {
        let mut borrow = slot.borrow_mut();
        if let Some(st) = borrow.as_mut() {
            if st.closing {
                return None;
            }
            st.closing = true;
        }
        borrow.take()
    });
    let Some(st) = old else {
        return;
    };
    unsafe {
        if st.picker_hwnd != 0 {
            let p = HWND(st.picker_hwnd as *mut c_void);
            if GetCapture() == p {
                let _ = ReleaseCapture();
            }
            if IsWindow(p).as_bool() {
                let _ = DestroyWindow(p);
            }
        }
        if st.loupe_hwnd != 0 {
            let l = HWND(st.loupe_hwnd as *mut c_void);
            if IsWindow(l).as_bool() {
                let _ = DestroyWindow(l);
            }
        }
        if st.loupe_dc != 0 {
            let _ = DeleteDC(HDC(st.loupe_dc as *mut c_void));
        }
        if st.loupe_bmp != 0 {
            let _ = DeleteObject(HGDIOBJ(st.loupe_bmp as *mut c_void));
        }
        if st.sample_dc != 0 {
            let _ = DeleteDC(HDC(st.sample_dc as *mut c_void));
        }
        if st.sample_bmp != 0 {
            let _ = DeleteObject(HGDIOBJ(st.sample_bmp as *mut c_void));
        }
    }
}

fn preview_clock_color_live(color: u32) {
    let clock_raw = CLOCK_RUNTIME.with(|slot| {
        let mut borrow = slot.borrow_mut();
        borrow.as_mut().map(|rt| {
            rt.cfg.color = color & 0x00FF_FFFF;
            rt.hwnd
        })
    });
    if let Some(raw) = clock_raw
        && raw != 0
    {
        render_and_place_mode(HWND(raw as *mut c_void), true);
    }
}

fn finish_color_pick(commit: bool) {
    let info = COLOR_PICK_SESSION.with(|slot| {
        slot.borrow().as_ref().map(|st| {
            (
                st.picker_hwnd,
                st.loupe_hwnd,
                st.owner,
                st.action_msg,
                st.original_color,
                st.last_color,
            )
        })
    });
    let Some((picker_raw, loupe_raw, owner_raw, action_msg, original_color, mut picked_color)) =
        info
    else {
        return;
    };

    if commit {
        // Hide capture overlay + loupe first so final 1x1 pixel read is 100% untinted by alpha=1 veil.
        unsafe {
            if loupe_raw != 0 {
                let _ = ShowWindow(HWND(loupe_raw as *mut c_void), SW_HIDE);
            }
            if picker_raw != 0 {
                let p = HWND(picker_raw as *mut c_void);
                if GetCapture() == p {
                    let _ = ReleaseCapture();
                }
                let _ = ShowWindow(p, SW_HIDE);
            }
            let _ = DwmFlush();
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            if let Some(exact) = sample_exact_screen_pixel(pt) {
                picked_color = exact;
            }
        }
    }

    cancel_color_pick();

    if commit {
        preview_clock_color_live(picked_color);
        CLOCK_PICKED_COLOR.with(|c| c.set(Some(picked_color)));
        if owner_raw != 0 {
            unsafe {
                let _ = PostMessageW(
                    HWND(owner_raw as *mut c_void),
                    action_msg,
                    WPARAM(CLOCK_CTX_PICK_COLOR),
                    LPARAM(crate::app::internal_action_token()),
                );
            }
        }
    } else {
        preview_clock_color_live(original_color);
        if owner_raw != 0 {
            unsafe {
                let _ = SetForegroundWindow(HWND(owner_raw as *mut c_void));
            }
        }
    }
}

unsafe fn sample_exact_screen_pixel(pt: POINT) -> Option<u32> {
    let mut result = None;
    COLOR_PICK_SESSION.with(|slot| {
        if let Some(st) = slot.borrow().as_ref()
            && st.sample_dc != 0
            && st.sample_bits != 0
        {
            // SAFETY: reads 1x1 pixel at virtual screen coordinates into owned 9x9 scratch DIB.
            unsafe {
                let sdc = GetDC(None);
                if !sdc.0.is_null() {
                    let _ = BitBlt(
                        HDC(st.sample_dc as *mut c_void),
                        0,
                        0,
                        1,
                        1,
                        sdc,
                        pt.x,
                        pt.y,
                        SRCCOPY,
                    );
                    let _ = GdiFlush();
                    let _ = ReleaseDC(None, sdc);
                    let px = *(st.sample_bits as *const u32);
                    result = Some(px & 0x00FF_FFFF);
                }
            }
        }
    });
    result
}

unsafe fn update_picker_loupe(pt: POINT) {
    let target_dpi = monitor_dpi_for_point(pt);
    let (target_lw, target_lh) = loupe_dims_for_dpi(target_dpi);
    let need_resize = COLOR_PICK_SESSION.with(|slot| {
        slot.borrow().as_ref().is_some_and(|st| {
            st.loupe_dpi != target_dpi || st.loupe_w != target_lw || st.loupe_h != target_lh
        })
    });
    if need_resize
        && let Some((new_dc, new_bmp, new_bits)) = unsafe { create_loupe_dib(target_lw, target_lh) }
    {
        let old = COLOR_PICK_SESSION.with(|slot| {
            slot.borrow_mut().as_mut().map(|st| {
                let prev = (st.loupe_dc, st.loupe_bmp);
                st.loupe_dc = new_dc.0 as isize;
                st.loupe_bmp = new_bmp.0 as isize;
                st.loupe_bits = new_bits as usize;
                st.loupe_dpi = target_dpi;
                st.loupe_w = target_lw;
                st.loupe_h = target_lh;
                prev
            })
        });
        unsafe {
            if let Some((old_dc, old_bmp)) = old {
                if old_dc != 0 {
                    let _ = DeleteDC(HDC(old_dc as *mut c_void));
                }
                if old_bmp != 0 {
                    let _ = DeleteObject(HGDIOBJ(old_bmp as *mut c_void));
                }
            } else {
                let _ = DeleteDC(new_dc);
                let _ = DeleteObject(HGDIOBJ(new_bmp.0));
            }
        }
    }

    let meta = COLOR_PICK_SESSION.with(|slot| {
        slot.borrow().as_ref().map(|st| {
            (
                st.loupe_hwnd,
                st.sample_dc,
                st.sample_bits,
                st.loupe_dc,
                st.loupe_bits,
                st.loupe_dpi,
                st.loupe_w,
                st.loupe_h,
            )
        })
    });
    let Some((loupe_raw, sdc_raw, sbits_raw, ldc_raw, lbits_raw, dpi, lw, lh)) = meta else {
        return;
    };
    if loupe_raw == 0
        || sdc_raw == 0
        || sbits_raw == 0
        || ldc_raw == 0
        || lbits_raw == 0
        || lw <= 0
        || lh <= 0
    {
        return;
    }
    let Some(loupe_total) = (lw as usize)
        .checked_mul(lh as usize)
        .filter(|&n| n <= isize::MAX as usize / std::mem::size_of::<u32>())
    else {
        return;
    };
    let sc = |n: i32| (n * dpi.max(96) / 96).max(1);

    let mut samples = [0u32; (SAMPLE_SIDE * SAMPLE_SIDE) as usize];
    // SAFETY: reads 9x9 virtual screen pixels around cursor and updates DPI-scaled layered loupe preview.
    unsafe {
        let sdc = GetDC(None);
        if sdc.0.is_null() {
            return;
        }
        let sample_dc = HDC(sdc_raw as *mut c_void);
        let half = SAMPLE_SIDE / 2;
        let _ = BitBlt(
            sample_dc,
            0,
            0,
            SAMPLE_SIDE,
            SAMPLE_SIDE,
            sdc,
            pt.x - half,
            pt.y - half,
            SRCCOPY,
        );
        let _ = GdiFlush();
        let _ = ReleaseDC(None, sdc);

        let raw = std::slice::from_raw_parts(sbits_raw as *const u32, samples.len());
        for (i, &px) in raw.iter().enumerate() {
            // Undo tiny 1/255 alpha=1 black veil so live preview matches exact final pixel color.
            let r = (((px >> 16) & 255) * 255 + 127) / 254;
            let g = (((px >> 8) & 255) * 255 + 127) / 254;
            let b = ((px & 255) * 255 + 127) / 254;
            samples[i] = (r.min(255) << 16) | (g.min(255) << 8) | b.min(255);
        }
    }

    let center_col = samples[samples.len() / 2];
    COLOR_PICK_SESSION.with(|slot| {
        if let Some(st) = slot.borrow_mut().as_mut() {
            st.last_color = center_col;
        }
    });
    preview_clock_color_live(center_col);

    // Paint DPI-scaled dark glass loupe into persistent DIB with 9x9 zoom grid + color swatch + hex code.
    unsafe {
        let mem_dc = HDC(ldc_raw as *mut c_void);
        let buf = std::slice::from_raw_parts_mut(lbits_raw as *mut u32, loupe_total);
        // Dark rounded pill background (#181818 @ 235 alpha).
        for y in 0..lh {
            for x in 0..lw {
                let border = x == 0 || y == 0 || x == lw - 1 || y == lh - 1;
                buf[(y * lw + x) as usize] = if border { 0xFF42_4242 } else { 0xEB18_1818 };
            }
        }
        // Left 9x9 zoomed pixel square scaled cleanly to monitor DPI.
        let cell = sc(5).max(4);
        let zx0 = sc(8);
        let zy0 = sc(8);
        for sy in 0..SAMPLE_SIDE {
            for sx in 0..SAMPLE_SIDE {
                let col = samples[(sy * SAMPLE_SIDE + sx) as usize];
                let px_col = 0xFF00_0000 | col;
                for dy in 0..cell {
                    for dx in 0..cell {
                        let gx = zx0 + sx * cell + dx;
                        let gy = zy0 + sy * cell + dy;
                        if gx < 0 || gx >= lw || gy < 0 || gy >= lh {
                            continue;
                        }
                        let grid_edge = dx == 0 || dy == 0;
                        let final_px = if sx == 4
                            && sy == 4
                            && (dx == 0 || dy == 0 || dx == cell - 1 || dy == cell - 1)
                        {
                            0xFFFF_FFFF
                        } else if grid_edge {
                            0xFF28_2828
                        } else {
                            px_col
                        };
                        buf[(gy * lw + gx) as usize] = final_px;
                    }
                }
            }
        }
        // Color swatch square next to zoom grid.
        let sw_x0 = sc(62).min(lw - 2);
        let sw_x1 = sc(80).min(lw - 1);
        let sw_y0 = sc(12).min(lh - 2);
        let sw_y1 = sc(30).min(lh - 1);
        for y in sw_y0..sw_y1 {
            for x in sw_x0..sw_x1 {
                let edge = x == sw_x0 || x == sw_x1 - 1 || y == sw_y0 || y == sw_y1 - 1;
                buf[(y * lw + x) as usize] = if edge {
                    0xFFAA_AAAA
                } else {
                    0xFF00_0000 | center_col
                };
            }
        }

        let hex = format!("#{:06X}", center_col);
        let mut canvas = GlyphCanvas {
            buf,
            width: lw,
            height: lh,
        };
        draw_quicksand_line(
            &mut canvas,
            &hex,
            RECT {
                left: sc(84),
                top: sc(10),
                right: lw - sc(6),
                bottom: sc(32),
            },
            sc(14),
            0,
            0x00FF_FFFF,
        );
        draw_quicksand_line(
            &mut canvas,
            "Click pick / Esc cancel",
            RECT {
                left: sc(60),
                top: sc(34),
                right: lw - sc(6),
                bottom: sc(54),
            },
            sc(11),
            0,
            0x00B8_B8B8,
        );

        // Offset loupe away from cursor while staying inside current monitor work area.
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let mon_r = if GetMonitorInfoW(mon, &mut mi).as_bool() {
            mi.rcMonitor
        } else {
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            }
        };
        let off = sc(20);
        let back_off = sc(16);
        let lx = if pt.x + off + lw <= mon_r.right {
            pt.x + off
        } else {
            pt.x - lw - back_off
        };
        let ly = if pt.y + off + lh <= mon_r.bottom {
            pt.y + off
        } else {
            pt.y - lh - back_off
        };

        let loupe = HWND(loupe_raw as *mut c_void);
        let dst = POINT { x: lx, y: ly };
        let size = SIZE { cx: lw, cy: lh };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(
            loupe,
            None,
            Some(&dst),
            Some(&size),
            mem_dc,
            Some(&src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        let _ = SetWindowPos(
            loupe,
            HWND_TOPMOST,
            lx,
            ly,
            lw,
            lh,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

unsafe extern "system" fn picker_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: interacts strictly with thread-local COLOR_PICK_SESSION on UI thread.
    unsafe {
        match msg {
            WM_MOUSEMOVE => {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                update_picker_loupe(pt);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                finish_color_pick(true);
                LRESULT(0)
            }
            WM_RBUTTONUP | WM_RBUTTONDOWN => {
                finish_color_pick(false);
                LRESULT(0)
            }
            WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => {
                finish_color_pick(false);
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                let active = COLOR_PICK_SESSION.with(|s| {
                    s.borrow()
                        .as_ref()
                        .is_some_and(|st| !st.closing && st.picker_hwnd == hwnd.0 as isize)
                });
                if active {
                    finish_color_pick(false);
                }
                LRESULT(0)
            }
            WM_SETCURSOR => {
                if let Ok(h) = LoadCursorW(None, IDC_CROSS) {
                    SetCursor(h);
                }
                LRESULT(1)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ClockFixture {
        panel: HWND,
        clock: HWND,
    }
    impl ClockFixture {
        fn popup() -> Self {
            unsafe {
                ensure_class(CLOCK_CLASS, Some(clock_proc), HGDIOBJ::default(), IDC_ARROW).unwrap();
                let instance = GetModuleHandleW(None).unwrap();
                let panel = CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    w!("STATIC"),
                    w!("WallpaperLive clock regression"),
                    WS_POPUP,
                    40,
                    40,
                    320,
                    200,
                    None,
                    None,
                    instance,
                    None,
                )
                .unwrap();
                let clock = CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    CLOCK_CLASS,
                    w!("WallpaperLive clock regression"),
                    WS_POPUP,
                    80,
                    80,
                    400,
                    176,
                    None,
                    None,
                    instance,
                    None,
                )
                .unwrap();
                CLOCK_RUNTIME.with(|s| {
                    *s.borrow_mut() = Some(ClockRuntime {
                        hwnd: clock.0 as isize,
                        owner: panel.0 as isize,
                        cfg: ClockConfig {
                            enabled: true,
                            behind_icons: false,
                            ..Default::default()
                        },
                        width: 400,
                        height: 176,
                        ..Default::default()
                    })
                });
                Self { panel, clock }
            }
        }
    }
    impl Drop for ClockFixture {
        fn drop(&mut self) {
            hide_all();
            unsafe {
                let _ = DestroyWindow(self.panel);
            }
        }
    }

    #[test]
    fn clock_unlocked_behind_icons_accepts_drag_without_changing_layer() {
        let fixture = ClockFixture::popup();
        unsafe {
            let mut cursor = POINT::default();
            assert!(GetCursorPos(&mut cursor).is_ok());
            SetWindowPos(
                fixture.panel,
                None,
                cursor.x - 280,
                cursor.y - 168,
                600,
                400,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .unwrap();
            SetParent(fixture.clock, fixture.panel).unwrap();
            SetWindowLongPtrW(
                fixture.clock,
                GWL_STYLE,
                (WS_CHILD | WS_CLIPSIBLINGS).0 as isize,
            );
            SetWindowPos(
                fixture.clock,
                None,
                80,
                80,
                400,
                176,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .unwrap();
            CLOCK_RUNTIME.with(|s| {
                let mut slot = s.borrow_mut();
                let rt = slot.as_mut().unwrap();
                rt.behind_attached = true;
                rt.cfg.behind_icons = true;
                rt.cached_layer = Some(DesktopLayer {
                    parent: fixture.panel,
                    icons: fixture.panel,
                    raised: true,
                });
            });
            SendMessageW(fixture.clock, WM_LBUTTONDOWN, WPARAM(0), LPARAM(0));
            assert!(
                CLOCK_RUNTIME.with(|s| s.borrow().as_ref().unwrap().dragging),
                "Behind desktop shortcuts must not lock an unlocked clock"
            );
            let mut before = RECT::default();
            GetWindowRect(fixture.clock, &mut before).unwrap();
            CLOCK_RUNTIME.with(|s| {
                let mut slot = s.borrow_mut();
                let rt = slot.as_mut().unwrap();
                rt.drag_offset_x -= 30;
                rt.drag_offset_y -= 24;
            });
            SendMessageW(fixture.clock, WM_MOUSEMOVE, WPARAM(0), LPARAM(0));
            let mut after = RECT::default();
            GetWindowRect(fixture.clock, &mut after).unwrap();
            assert_eq!(
                (after.left - before.left, after.top - before.top),
                (30, 24),
                "child drag must translate screen coordinates to its desktop parent"
            );
            SendMessageW(fixture.clock, WM_LBUTTONUP, WPARAM(0), LPARAM(0));
            assert_eq!(GetParent(fixture.clock).unwrap(), fixture.panel);
            CLOCK_RUNTIME.with(|s| s.borrow_mut().as_mut().unwrap().cfg.locked = true);
            SendMessageW(fixture.clock, WM_LBUTTONDOWN, WPARAM(0), LPARAM(0));
            assert!(
                !CLOCK_RUNTIME.with(|s| s.borrow().as_ref().unwrap().dragging),
                "Lock must block dragging regardless of desktop layer"
            );
        }
    }

    #[test]
    fn clock_behind_icons_routes_mouse_through_a_desktop_child() {
        let fixture = ClockFixture::popup();
        unsafe {
            let instance = GetModuleHandleW(None).unwrap();
            let list = CreateWindowExW(
                Default::default(),
                w!("SysListView32"),
                w!(""),
                WS_CHILD | WS_VISIBLE,
                0,
                0,
                600,
                400,
                fixture.panel,
                None,
                instance,
                None,
            )
            .unwrap();
            SetParent(fixture.clock, fixture.panel).unwrap();
            SetWindowLongPtrW(
                fixture.clock,
                GWL_STYLE,
                (WS_CHILD | WS_CLIPSIBLINGS).0 as isize,
            );
            CLOCK_RUNTIME.with(|s| {
                let mut slot = s.borrow_mut();
                let rt = slot.as_mut().unwrap();
                rt.behind_attached = true;
                rt.cfg.behind_icons = true;
                rt.cached_layer = Some(DesktopLayer {
                    parent: fixture.panel,
                    icons: fixture.panel,
                    raised: true,
                });
            });
            render_and_place(fixture.clock);
            let input = CLOCK_RUNTIME.with(|s| {
                s.borrow()
                    .as_ref()
                    .and_then(|r| r.input.as_ref().map(|i| i.hwnd))
            });
            assert!(
                input.is_some(),
                "behind-icons clock needs an input handle above the desktop"
            );
            let input = input.unwrap();
            assert_eq!(
                GetWindowLongPtrW(input, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0,
                0
            );
            let mut cursor = POINT::default();
            GetCursorPos(&mut cursor).unwrap();
            let mut input_rect = RECT::default();
            let mut widget_rect = RECT::default();
            let (width, height) = CLOCK_RUNTIME.with(|s| {
                let slot = s.borrow();
                let rt = slot.as_ref().unwrap();
                (rt.width, rt.height)
            });
            SetWindowPos(
                fixture.panel,
                None,
                cursor.x - width / 2 - 80,
                cursor.y - height / 2 - 80,
                2000,
                1400,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .unwrap();
            SetWindowPos(
                fixture.clock,
                None,
                80,
                80,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .unwrap();
            sync_clock_input();
            SendMessageW(input, WM_LBUTTONDOWN, WPARAM(0), LPARAM(0));
            assert_eq!(GetCapture(), input);
            GetWindowRect(fixture.clock, &mut widget_rect).unwrap();
            let mut client_pt = cursor;
            let _ = ScreenToClient(fixture.clock, &mut client_pt);
            assert!(
                CLOCK_RUNTIME.with(|s| s.borrow().as_ref().unwrap().dragging),
                "forwarded down: cursor={cursor:?} widget={widget_rect:?} client={client_pt:?}, userdata={} rt={:?}",
                GetWindowLongPtrW(input, GWLP_USERDATA),
                CLOCK_RUNTIME.with(|s| s.borrow().as_ref().map(|r| (
                    r.hwnd,
                    r.width,
                    r.height,
                    r.cfg.locked,
                    r.dragging
                )))
            );
            CLOCK_RUNTIME.with(|s| {
                let mut slot = s.borrow_mut();
                let rt = slot.as_mut().unwrap();
                rt.drag_offset_x -= 30;
                rt.drag_offset_y -= 24;
                rt.drag_start_x -= 30;
                rt.drag_start_y -= 24;
            });
            SendMessageW(input, WM_MOUSEMOVE, WPARAM(0), LPARAM(0));
            GetWindowRect(input, &mut input_rect).unwrap();
            GetWindowRect(fixture.clock, &mut widget_rect).unwrap();
            assert_eq!(
                (input_rect.left, input_rect.top),
                (widget_rect.left, widget_rect.top),
                "input child must follow the visual child throughout a background drag"
            );
            SendMessageW(input, WM_LBUTTONUP, WPARAM(0), LPARAM(0));
            assert_eq!(GetCapture(), HWND::default());
            assert_eq!(
                take_saved_position(),
                Some((widget_rect.left, widget_rect.top))
            );
            assert_eq!(
                CLOCK_RUNTIME.with(|s| s.borrow().as_ref().unwrap().cfg.pos),
                Some((widget_rect.left, widget_rect.top)),
                "minute redraw must keep the dragged position"
            );
            SendMessageW(input, WM_LBUTTONDOWN, WPARAM(0), LPARAM(0));
            let _ = ReleaseCapture();
            assert!(
                !CLOCK_RUNTIME.with(|s| s.borrow().as_ref().unwrap().dragging),
                "losing input capture must not leave the clock dragging"
            );
            CLOCK_RUNTIME.with(|s| s.borrow_mut().as_mut().unwrap().cfg.locked = true);
            render_and_place(fixture.clock);
            assert!(
                !IsWindow(input).as_bool(),
                "Lock removes the drag input child"
            );
            CLOCK_RUNTIME.with(|s| s.borrow_mut().as_mut().unwrap().cfg.locked = false);
            sync_clock_input();
            let input =
                CLOCK_RUNTIME.with(|s| s.borrow().as_ref().unwrap().input.as_ref().unwrap().hwnd);
            use windows::Win32::UI::Controls::{
                LVIF_TEXT, LVITEMW, LVM_INSERTITEMW, LVM_SETITEMPOSITION,
            };
            let mut label: Vec<u16> = "Desktop shortcut".encode_utf16().chain(Some(0)).collect();
            let item = LVITEMW {
                mask: LVIF_TEXT,
                pszText: windows::core::PWSTR(label.as_mut_ptr()),
                ..Default::default()
            };
            assert_eq!(
                SendMessageW(
                    list,
                    LVM_INSERTITEMW,
                    WPARAM(0),
                    LPARAM((&item as *const LVITEMW) as isize)
                )
                .0,
                0
            );
            SendMessageW(
                list,
                LVM_SETITEMPOSITION,
                WPARAM(0),
                LPARAM(100 | (100 << 16)),
            );
            let icons = desktop_shortcut_rects(list);
            assert_eq!(icons.len(), 1);
            let center = POINT {
                x: (icons[0].left + icons[0].right) / 2,
                y: (icons[0].top + icons[0].bottom) / 2,
            };
            let mut widget = RECT::default();
            GetWindowRect(fixture.clock, &mut widget).unwrap();
            let x = center.x - (widget.right - widget.left) / 2;
            let y = center.y - (widget.bottom - widget.top) / 2;
            let mut origin = [POINT { x, y }];
            MapWindowPoints(None, fixture.panel, &mut origin);
            SetWindowPos(
                fixture.clock,
                None,
                origin[0].x,
                origin[0].y,
                widget.right - widget.left,
                widget.bottom - widget.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .unwrap();
            sync_clock_input();
            let region = windows::Win32::Graphics::Gdi::CreateRectRgn(0, 0, 0, 0);
            assert_ne!(
                windows::Win32::Graphics::Gdi::GetWindowRgn(input, region).0,
                0
            );
            assert!(
                !windows::Win32::Graphics::Gdi::PtInRegion(region, center.x - x, center.y - y)
                    .as_bool(),
                "input child must leave shortcut icon and label clickable"
            );
            let _ = DeleteObject(HGDIOBJ(region.0));
            // Repeated sync with unchanged holes skips rebuilding HRGN yet preserves the icon hole.
            sync_clock_input();
            let region2 = windows::Win32::Graphics::Gdi::CreateRectRgn(0, 0, 0, 0);
            assert_ne!(
                windows::Win32::Graphics::Gdi::GetWindowRgn(input, region2).0,
                0
            );
            assert!(
                !windows::Win32::Graphics::Gdi::PtInRegion(region2, center.x - x, center.y - y)
                    .as_bool()
            );
            let _ = DeleteObject(HGDIOBJ(region2.0));
            hide_all();
            assert!(!IsWindow(input).as_bool());
            let _ = DestroyWindow(list);
        }
    }

    #[test]
    fn clock_foreground_hook_observes_wallpaperlive_panel() {
        let fixture = ClockFixture::popup();
        unsafe {
            let hooks = install_events();
            CLOCK_RUNTIME.with(|s| s.borrow_mut().as_mut().unwrap().hooks = hooks);
            assert!(hooks[0] != 0);
            windows::Win32::UI::Accessibility::NotifyWinEvent(
                EVENT_SYSTEM_FOREGROUND,
                fixture.panel,
                0,
                0,
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
            let mut tick = MSG::default();
            let mut pending = false;
            while std::time::Instant::now() < deadline {
                pending = PeekMessageW(&mut tick, fixture.clock, WM_TIMER, WM_TIMER, PM_NOREMOVE)
                    .as_bool();
                if pending {
                    break;
                }
                std::thread::yield_now();
            }
            assert!(
                pending,
                "own-panel foreground must schedule clock z-order repair"
            );
        }
    }

    #[test]
    fn mond_clock_formatting_anurati_raster_and_hit_policy() {
        let (day, date, t24) = format_clock_lines(2026, 11, 3, 18, 13, 9, true);
        assert_eq!(day, "WEDNESDAY");
        assert_eq!(date, "18  NOVEMBER,  2026.");
        assert_eq!(t24, "- 13:09 -");

        let (_, date_pad, t12) = format_clock_lines(2026, 6, 0, 5, 0, 5, false);
        assert_eq!(date_pad, "05  JUNE,  2026.");
        assert_eq!(t12, "- 12:05 AM -");
        let (_, _, t12_pm) = format_clock_lines(2026, 6, 5, 5, 15, 45, false);
        assert_eq!(t12_pm, "- 3:45 PM -");

        // Every weekday letter has an exact Anurati vector contour and positive advance.
        for dow in 0..=6 {
            let name = weekday_name(dow);
            assert!(
                name.chars()
                    .all(|c| anurati_glyph(c).is_some_and(|g| !g.ops.is_empty() && g.advance > 0.0))
            );
        }

        // Exact Mond Anurati rasterization produces solid anti-aliased foreground + shadow.
        let (w, h) = widget_pixel_size("WEDNESDAY", 100, 96);
        let mut buf = vec![0u32; (w * h) as usize];
        {
            let mut canvas = GlyphCanvas {
                buf: &mut buf,
                width: w,
                height: h,
            };
            render_anurati_word(
                &mut canvas,
                "WEDNESDAY",
                16.0,
                50.0,
                w as f32 * 0.5,
                0x00FF_FFFF,
            );
        }
        assert!(buf.iter().any(|&px| (px >> 24) == 255));
        // Warm Anurati cache produces bit-identical output across repeated draws and fresh re-rasters.
        let mut buf_cached = vec![0u32; (w * h) as usize];
        {
            let mut canvas = GlyphCanvas {
                buf: &mut buf_cached,
                width: w,
                height: h,
            };
            render_anurati_word(
                &mut canvas,
                "WEDNESDAY",
                16.0,
                50.0,
                w as f32 * 0.5,
                0x00FF_FFFF,
            );
        }
        assert_eq!(
            buf, buf_cached,
            "cached Anurati frame must match cold raster"
        );

        let mut orange_warm = vec![0u32; (w * h) as usize];
        {
            let mut canvas = GlyphCanvas {
                buf: &mut orange_warm,
                width: w,
                height: h,
            };
            render_anurati_word(
                &mut canvas,
                "WEDNESDAY",
                16.0,
                50.0,
                w as f32 * 0.5,
                0x00D9_541E,
            );
        }
        ANURATI_WORD_CACHE.with(|c| c.borrow_mut().free());
        let mut orange_cold = vec![0u32; (w * h) as usize];
        {
            let mut canvas = GlyphCanvas {
                buf: &mut orange_cold,
                width: w,
                height: h,
            };
            render_anurati_word(
                &mut canvas,
                "WEDNESDAY",
                16.0,
                50.0,
                w as f32 * 0.5,
                0x00D9_541E,
            );
        }
        assert_eq!(
            orange_warm, orange_cold,
            "color switch hitting warm Anurati cache must match cold rasterization"
        );
        ANURATI_WORD_CACHE.with(|c| c.borrow_mut().free());

        apply_opacity_in_place(&mut buf, 50);
        let max_alpha = buf.iter().map(|&px| px >> 24).max().unwrap_or(0);
        assert!((120..=135).contains(&max_alpha));

        // Lock position is strictly distinct from Behind desktop shortcuts: locking does NOT activate WS_EX_TRANSPARENT.
        assert!(!is_click_through(
            ClockConfig {
                enabled: true,
                locked: true,
                behind_icons: false,
                ..Default::default()
            },
            false,
        ));
        assert!(is_click_through(
            ClockConfig {
                enabled: true,
                locked: false,
                behind_icons: true,
                ..Default::default()
            },
            false,
        ));
        assert!(!is_click_through(
            ClockConfig {
                enabled: true,
                locked: true,
                behind_icons: true,
                ..Default::default()
            },
            true,
        ));

        // Inner center drag handle rejects outer border touches and accepts dead-center clicks.
        assert!(in_drag_handle(w / 2, h / 2, w, h));
        assert!(!in_drag_handle(4, h / 2, w, h));
        assert!(!in_drag_handle(w - 4, h / 2, w, h));
        assert!(!in_drag_handle(w / 2, 4, w, h));
        assert!(!in_drag_handle(w / 2, h - 4, w, h));

        assert_eq!(minute_timer_delay_ms(0), 60_050);
        assert_eq!(minute_timer_delay_ms(59), 1_050);
        assert_eq!(minute_timer_delay_ms(65), 1_050);

        // Multi-monitor negative/secondary work areas keep positions within that monitor cleanly.
        let left_monitor = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1040,
        };
        assert_eq!(
            resolve_screen_position(Some((-1400, 120)), left_monitor, 400, 180),
            (-1400, 120)
        );
        assert_eq!(
            resolve_screen_position(Some((-2100, -50)), left_monitor, 400, 180),
            (-1920, 0)
        );
        assert_eq!(
            resolve_screen_position(Some((200, 950)), left_monitor, 400, 180),
            (-400, 860)
        );
        assert_eq!(
            resolve_screen_position(None, left_monitor, 400, 180),
            (-1160, 170)
        );
    }

    #[test]
    fn context_menu_ids_are_unique_and_formatted_without_tabs() {
        for repositioning in [false, true] {
            let items = build_context_menu_items(ClockConfig::default(), repositioning);
            let mut ids: Vec<usize> = items.iter().map(|(id, _)| *id).collect();
            let count = ids.len();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(
                ids.len(),
                count,
                "context menu command IDs must never collide (repositioning={repositioning})"
            );
            for (id, label) in &items {
                assert!(*id != 0, "menu ID must be non-zero for {label}");
                assert!(
                    !label.contains('\t'),
                    "menu label `{label}` must not contain unexpanded tabs"
                );
            }
            assert!(
                items
                    .iter()
                    .any(|&(id, l)| id == CLOCK_CTX_24H && l.contains("24-hour"))
            );
            assert!(
                items
                    .iter()
                    .any(|&(id, l)| id == CLOCK_CTX_LOCK && l.contains("Lock position"))
            );
            assert!(
                items.iter().any(|&(id, _)| id == CTX_LOCAL_SIZE_UP)
                    && items.iter().any(|&(id, _)| id == CTX_LOCAL_SIZE_DOWN)
            );
        }
    }

    #[test]
    fn zorder_helper_detects_relative_window_order() {
        // SAFETY: isolated hidden WS_POPUP windows strictly created and destroyed on this test thread.
        unsafe {
            let instance = GetModuleHandleW(None).unwrap();
            let lower = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("WallpaperLive ZOrder Lower"),
                WS_POPUP,
                10,
                10,
                100,
                100,
                None,
                None,
                instance,
                None,
            )
            .unwrap();
            let upper = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("WallpaperLive ZOrder Upper"),
                WS_POPUP,
                20,
                20,
                100,
                100,
                None,
                None,
                instance,
                None,
            )
            .unwrap();
            let _ = SetWindowPos(
                lower,
                HWND_BOTTOM,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            let _ = SetWindowPos(
                upper,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            assert!(is_above_in_zorder(upper, lower));
            assert!(!is_above_in_zorder(lower, upper));
            assert!(!is_above_in_zorder(upper, upper));
            let _ = DestroyWindow(upper);
            let _ = DestroyWindow(lower);
        }
    }

    #[test]
    fn popup_placement_verified_requires_all_three_conditions() {
        assert!(popup_placement_verified(true, true, true));
        // UIPI SetWindowPos failure leaves window hidden above shell; must not short-circuit.
        assert!(!popup_placement_verified(false, false, true));
        assert!(!popup_placement_verified(false, true, true));
        assert!(!popup_placement_verified(true, false, true));
        assert!(!popup_placement_verified(true, true, false));
    }

    #[test]
    fn zorder_sync_guard_tracks_nested_scope() {
        assert!(!in_zorder_sync());
        {
            let _outer = ZOrderSyncGuard::enter();
            assert!(in_zorder_sync());
            {
                let _inner = ZOrderSyncGuard::enter();
                assert!(in_zorder_sync());
            }
            assert!(
                in_zorder_sync(),
                "dropping inner guard must keep outer zorder sync active"
            );
        }
        assert!(!in_zorder_sync());
    }

    #[test]
    fn layered_popup_reveal_updates_surface_and_becomes_visible() {
        // SAFETY: isolated WS_POPUP layered window strictly created and destroyed on this test thread.
        unsafe {
            ensure_class(CLOCK_CLASS, Some(clock_proc), HGDIOBJ::default(), IDC_ARROW).unwrap();
            let instance = GetModuleHandleW(None).unwrap();
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLOCK_CLASS,
                w!("WallpaperLive Layered Reveal Test"),
                WS_POPUP,
                0,
                0,
                100,
                100,
                None,
                None,
                instance,
                None,
            )
            .unwrap();
            let mut scratch = ClockTextScratch::default();
            let (w, h) = (360, 176);
            let (mem_dc, bits) = scratch.ensure(w, h).unwrap();
            std::slice::from_raw_parts_mut(bits, (w * h) as usize).fill(0x80FF_FFFF);
            let dst = POINT { x: 48, y: 64 };
            let size = SIZE { cx: w, cy: h };
            let first_ok = update_layered_clock_surface(hwnd, dst, size, mem_dc);
            assert!(first_ok, "UpdateLayeredWindow must commit initial frame");
            place_popup_on_desktop(hwnd, dst.x, dst.y, w, h, !first_ok, false);
            if !first_ok {
                assert!(update_layered_clock_surface(hwnd, dst, size, mem_dc));
            }
            assert!(
                IsWindowVisible(hwnd).as_bool(),
                "layered popup must become visible after reveal"
            );
            let mut rc = RECT::default();
            GetWindowRect(hwnd, &mut rc).unwrap();
            assert_eq!(
                (rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top),
                (48, 64, w, h)
            );
            scratch.free();
            let _ = DestroyWindow(hwnd);
        }
    }
}
