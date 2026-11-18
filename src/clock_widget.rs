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
            BOOL, COLORREF, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, SYSTEMTIME, WPARAM,
        },
        Graphics::{
            Dwm::DwmFlush,
            Dxgi::{
                Common::{
                    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
                },
                DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
                DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice,
                IDXGIFactory2, IDXGISwapChain1,
            },
            Gdi::{
                AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, AddFontMemResourceEx, BI_RGB,
                BITMAPINFO, BITMAPINFOHEADER, BLACK_BRUSH, BLENDFUNCTION, BitBlt,
                CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW,
                DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CENTER, DT_NOPREFIX,
                DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FF_DONTCARE, FW_BOLD,
                GdiFlush, GetDC, GetMonitorInfoW, GetStockObject, HDC, HGDIOBJ,
                MONITOR_DEFAULTTONEAREST, MONITORINFO, MapWindowPoints, MonitorFromPoint,
                MonitorFromWindow, OUT_DEFAULT_PRECIS, ReleaseDC, SRCCOPY, ScreenToClient,
                SelectObject, SetBkMode, SetTextCharacterExtra, SetTextColor, TRANSPARENT,
            },
        },
        System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW},
        UI::{
            Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
            HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI},
            Input::KeyboardAndMouse::{
                GetCapture, ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE,
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
        if is_workerw_class(window)
            && belong_to_pid(window, out.progman_pid)
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

struct DCompPresenter {
    _d3d_device: IUnknown,
    d3d_ctx: ComPtr,
    dcomp_device: ComPtr,
    dcomp_target: ComPtr,
    dcomp_visual: ComPtr,
    swap_chain: IDXGISwapChain1,
    w: u32,
    h: u32,
}

impl DCompPresenter {
    fn new(hwnd: HWND, w: u32, h: u32) -> Option<Self> {
        let w = w.max(1);
        let h = h.max(1);
        // SAFETY: dynamically loads OS system DLLs d3d11.dll/dcomp.dll and uses verified Win32 COM ABI slots.
        unsafe {
            let d3d_dll = LoadLibraryW(w!("d3d11.dll")).ok()?;
            let dcomp_dll = LoadLibraryW(w!("dcomp.dll")).ok()?;
            let d3d_proc = GetProcAddress(d3d_dll, s!("D3D11CreateDevice") as PCSTR)?;
            let dcomp_proc = GetProcAddress(dcomp_dll, s!("DCompositionCreateDevice") as PCSTR)?;
            let create_d3d: D3D11CreateDeviceFn = std::mem::transmute(d3d_proc);
            let create_dcomp: DCompositionCreateDeviceFn = std::mem::transmute(dcomp_proc);

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
    _: i32,
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
    let state = CLOCK_RUNTIME.with(|s| {
        s.borrow()
            .as_ref()
            .filter(|r| !r.dragging)
            .and_then(|r| Some((r.input.as_ref()?.hwnd, r.cached_layer?.icons)))
    });
    if let Some((input, icons)) = state {
        unsafe {
            if event_hwnd == icons || IsChild(icons, event_hwnd).as_bool() {
                let _ = SetTimer(input, CLOCK_INPUT_REFRESH as usize, 80, None);
            }
        }
    }
}

fn sync_clock_input() {
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
        let view = FindWindowExW(layer.icons, None, w!("SysListView32"), PCWSTR::null())
            .unwrap_or_default();
        let exclusions = desktop_shortcut_rects(view);
        let hwnd = if let Some(input) = input {
            input
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
                    rt.input = Some(ClockInput { hwnd, hook });
                }
            });
            hwnd
        };
        let mut area = RECT::default();
        if GetWindowRect(HWND(raw as *mut c_void), &mut area).is_err() {
            return;
        }
        let handle = drag_handle_rect(w, h);
        let region = windows::Win32::Graphics::Gdi::CreateRectRgnIndirect(&handle);
        if region.0.is_null() {
            return;
        }
        for icon in exclusions {
            let hole = windows::Win32::Graphics::Gdi::CreateRectRgn(
                icon.left - area.left,
                icon.top - area.top,
                icon.right - area.left,
                icon.bottom - area.top,
            );
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
                    if !IN_ZORDER_SYNC.with(Cell::get) {
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
            if self.bmp != 0 {
                let _ = DeleteObject(HGDIOBJ(self.bmp as *mut c_void));
            }
            if self.dc != 0 {
                let _ = DeleteDC(HDC(self.dc as *mut c_void));
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
    static ALLOW_INTERNAL_HIDE: Cell<bool> = const { Cell::new(false) };
    static IN_ZORDER_SYNC: Cell<bool> = const { Cell::new(false) };
    static COLOR_PICK_SESSION: RefCell<Option<ColorPickState>> = const { RefCell::new(None) };
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

unsafe extern "system" fn find_top_shell_cb(window: HWND, data: LPARAM) -> BOOL {
    // SAFETY: read-only EnumWindows top-to-bottom scan matching Explorer's desktop host windows.
    unsafe {
        if !IsWindowVisible(window).as_bool() {
            return BOOL(1);
        }
        let out = &mut *(data.0 as *mut DesktopTopSearch);
        if window == out.progman {
            out.top_shell = window;
            return BOOL(0);
        }
        if is_workerw_class(window)
            && belong_to_pid(window, out.progman_pid)
            && FindWindowExW(window, None, w!("SHELLDLL_DefView"), PCWSTR::null()).is_ok()
        {
            out.top_shell = window;
            return BOOL(0);
        }
    }
    BOOL(1)
}

fn window_belongs_to_shell(hwnd: HWND, shell_top: HWND, progman: HWND) -> bool {
    if hwnd.0.is_null() {
        return false;
    }
    if hwnd == shell_top || hwnd == progman {
        return true;
    }
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    root == shell_top || root == progman
}

#[derive(Clone, Copy)]
enum PopupZTarget {
    ShowDesktopTopmost(HWND),
    BehindApps(HWND),
}

fn detect_popup_z_target(clock_hwnd: HWND, repositioning: bool) -> PopupZTarget {
    if repositioning {
        return PopupZTarget::ShowDesktopTopmost(HWND::default());
    }
    // SAFETY: walks global top-level Z-order to mirror Rainmeter System::PrepareHelperWindow.
    unsafe {
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
            return PopupZTarget::BehindApps(HWND_BOTTOM);
        }

        let fg = GetForegroundWindow();
        let mut prev_non_topmost = HWND::default();
        let mut last_topmost = HWND::default();
        let mut cur = GetWindow(shell_top, GW_HWNDPREV).unwrap_or_default();
        let mut hops = 0usize;
        while !cur.0.is_null() && hops < 256 {
            hops += 1;
            if cur == clock_hwnd || !IsWindowVisible(cur).as_bool() {
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
            if prev_non_topmost.0.is_null() {
                prev_non_topmost = cur;
                break;
            }
            cur = GetWindow(cur, GW_HWNDPREV).unwrap_or_default();
        }

        // Exact Rainmeter ShowDesktop rule: if desktop is foreground or sits directly under TOPMOST band.
        let show_desktop =
            prev_non_topmost.0.is_null() || window_belongs_to_shell(fg, shell_top, progman);
        if show_desktop {
            PopupZTarget::ShowDesktopTopmost(last_topmost)
        } else {
            PopupZTarget::BehindApps(prev_non_topmost)
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
    IN_ZORDER_SYNC.with(|g| g.set(true));
    // SAFETY: owns clock HWND; IN_ZORDER_SYNC prevents WM_WINDOWPOSCHANGING recursion.
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
            }
            PopupZTarget::BehindApps(prev_app) => {
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
                let _ = SetWindowPos(hwnd, prev_app, x, y, w, h, base_flags);
            }
        }
    }
    IN_ZORDER_SYNC.with(|g| g.set(false));
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
            IN_ZORDER_SYNC.with(|g| g.set(true));
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
            IN_ZORDER_SYNC.with(|g| g.set(false));
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

pub fn hide_all() {
    cancel_color_pick();
    let old = CLOCK_RUNTIME.with(|slot| slot.borrow_mut().take());
    if let Some(mut rt) = old {
        drop(rt.input.take());
        destroy_clock_hwnd(rt.hwnd, rt.hooks);
    }
    CLOCK_TEXT_SCRATCH.with(|s| s.borrow_mut().free());
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
    let glyphs: Vec<&AnuratiGlyph> = word.chars().filter_map(anurati_glyph).collect();
    if glyphs.is_empty() {
        return 0.0;
    }
    let mut width = 0.0f32;
    for (i, g) in glyphs.iter().enumerate() {
        if i + 1 < glyphs.len() {
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

fn target_monitor_info(hwnd: HWND, owner: HWND, pos: Option<(i32, i32)>) -> (RECT, i32) {
    let mon = if let Some((x, y)) = pos {
        unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) }
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

fn premul_blend_pixel(dst: &mut u32, r: u32, g: u32, b: u32, a: u8) {
    if a == 0 {
        return;
    }
    let a_u = u32::from(a);
    let src_r = (r * a_u + 127) / 255;
    let src_g = (g * a_u + 127) / 255;
    let src_b = (b * a_u + 127) / 255;
    let inv = 255 - a_u;
    let cur_a = (*dst >> 24) & 255;
    let cur_r = (*dst >> 16) & 255;
    let cur_g = (*dst >> 8) & 255;
    let cur_b = *dst & 255;
    let out_a = (a_u + (cur_a * inv + 127) / 255).min(255);
    let out_r = (src_r + (cur_r * inv + 127) / 255).min(out_a);
    let out_g = (src_g + (cur_g * inv + 127) / 255).min(out_a);
    let out_b = (src_b + (cur_b * inv + 127) / 255).min(out_a);
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
    let em_scale = cap_height_px / ANURATI_CAP_HEIGHT_UNITS;
    let mut pen_x = center_x - total_units * em_scale * 0.5;
    let r = (color >> 16) & 255;
    let g = (color >> 8) & 255;
    let b = color & 255;
    let sh_dx = (1.5 * (cap_height_px / 50.0)).round().clamp(1.0, 3.0) as i32;
    let sh_dy = (2.0 * (cap_height_px / 50.0)).round().clamp(1.0, 4.0) as i32;

    let mut edges = Vec::with_capacity(64);
    let mut crossings = Vec::with_capacity(16);
    let mut cov_grid = Vec::new();
    let mut hits = Vec::new();

    for ch in text.chars() {
        let Some(glyph) = anurati_glyph(ch) else {
            continue;
        };
        collect_glyph_edges(glyph.ops, pen_x, top_y, em_scale, &mut edges);
        pen_x += (glyph.advance + ANURATI_TRACKING_UNITS) * em_scale;
        if edges.is_empty() {
            continue;
        }

        let mut min_x = f32::MAX;
        let mut max_x = f32::MIN;
        let mut min_y = f32::MAX;
        let mut max_y = f32::MIN;
        for &(ax, ay, bx, by) in &edges {
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
        cov_grid.clear();
        cov_grid.resize(bw * bh, 0);
        hits.clear();
        hits.resize(bw, 0u8);

        for row_idx in 0..bh {
            let py = y0 + row_idx as i32;
            hits.fill(0);
            for sub_y in 0..4 {
                let sy = py as f32 + (sub_y as f32 + 0.5) * 0.25;
                crossings.clear();
                for &(ax, ay, bx, by) in &edges {
                    let (lo, hi) = if ay < by { (ay, by) } else { (by, ay) };
                    if lo <= sy && sy < hi {
                        let ix = ax + (sy - ay) * (bx - ax) / (by - ay);
                        crossings.push(ix);
                    }
                }
                crossings.sort_unstable_by(f32::total_cmp);
                for &[xa, xb] in crossings.as_chunks::<2>().0 {
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
                        hits[col] = hits[col].saturating_add(h_count);
                    }
                }
            }
            let out_row = &mut cov_grid[row_idx * bw..(row_idx + 1) * bw];
            for col in 0..bw {
                if hits[col] > 0 {
                    out_row[col] = ((u16::from(hits[col]) * 255 + 8) / 16).min(255) as u8;
                }
            }
        }

        // First composite soft drop shadow, then foreground so no glyph casts shadow onto itself.
        for row_idx in 0..bh {
            let py = y0 + row_idx as i32 + sh_dy;
            if py < 0 || py >= buf_h {
                continue;
            }
            let dst_row = py as usize * stride_px;
            let src_row = &cov_grid[row_idx * bw..(row_idx + 1) * bw];
            for (col, &cov) in src_row.iter().enumerate().take(bw) {
                if cov == 0 {
                    continue;
                }
                let px = x0 + col as i32 + sh_dx;
                if px >= 0 && (px as usize) < stride_px {
                    let sh_a = ((u16::from(cov) * 76) / 255) as u8;
                    premul_blend_pixel(&mut canvas.buf[dst_row + px as usize], 0, 0, 0, sh_a);
                }
            }
        }
        for row_idx in 0..bh {
            let py = y0 + row_idx as i32;
            let dst_row = py as usize * stride_px;
            let src_row = &cov_grid[row_idx * bw..(row_idx + 1) * bw];
            for (col, &cov) in src_row.iter().enumerate().take(bw) {
                if cov == 0 {
                    continue;
                }
                let px = x0 + col as i32;
                premul_blend_pixel(&mut canvas.buf[dst_row + px as usize], r, g, b, cov);
            }
        }
    }
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
    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return;
    }

    CLOCK_TEXT_SCRATCH.with(|slot| {
        let mut scratch = slot.borrow_mut();
        let Some((dc, bits_ptr)) = scratch.ensure(rw, rh) else {
            return;
        };
        let total = rw as usize * rh as usize;
        // SAFETY: scratch DIB has rw * rh 32-bit pixels owned by this thread.
        unsafe {
            std::slice::from_raw_parts_mut(bits_ptr, total).fill(0);
            let font = CreateFontW(
                -font_px.max(10),
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
            );
            let prev_font = (!font.0.is_null()).then(|| SelectObject(dc, HGDIOBJ(font.0)));
            let prev_extra = SetTextCharacterExtra(dc, char_extra.max(0));
            let _ = SetBkMode(dc, TRANSPARENT);
            let _ = SetTextColor(dc, COLORREF(0x00FF_FFFF));
            let mut local_rect = RECT {
                left: 0,
                top: 0,
                right: rw,
                bottom: rh,
            };
            let mut wide_mut = wide;
            let _ = DrawTextW(
                dc,
                &mut wide_mut,
                &mut local_rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            let _ = GdiFlush();
            SetTextCharacterExtra(dc, prev_extra);
            if let Some(prev) = prev_font {
                let _ = SelectObject(dc, prev);
            }
            if !font.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(font.0));
            }
        }

        // SAFETY: scratch DIB pixels remain valid after GdiFlush.
        let src = unsafe { std::slice::from_raw_parts(bits_ptr, total) };
        let r = (color >> 16) & 255;
        let g = (color >> 8) & 255;
        let b = color & 255;
        let stride = buf_w as usize;

        for sy in 0..rh {
            let dy = area.top + sy + 2;
            if dy < 0 || dy >= buf_h {
                continue;
            }
            let src_row = sy as usize * rw as usize;
            let dst_row = dy as usize * stride;
            for sx in 0..rw {
                let cov = (src[src_row + sx as usize] >> 16) & 255;
                if cov == 0 {
                    continue;
                }
                let dx = area.left + sx + 1;
                if dx >= 0 && dx < buf_w {
                    let sh_a = ((cov * 84) / 255) as u8;
                    premul_blend_pixel(&mut canvas.buf[dst_row + dx as usize], 0, 0, 0, sh_a);
                }
            }
        }
        for sy in 0..rh {
            let dy = area.top + sy;
            if dy < 0 || dy >= buf_h {
                continue;
            }
            let src_row = sy as usize * rw as usize;
            let dst_row = dy as usize * stride;
            for sx in 0..rw {
                let cov = (src[src_row + sx as usize] >> 16) & 255;
                if cov == 0 {
                    continue;
                }
                let dx = area.left + sx;
                if dx >= 0 && dx < buf_w {
                    premul_blend_pixel(&mut canvas.buf[dst_row + dx as usize], r, g, b, cov as u8);
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
        let a = (*px >> 24) & 255;
        if a == 0 {
            continue;
        }
        let r = (*px >> 16) & 255;
        let g = (*px >> 8) & 255;
        let b = *px & 255;
        let na = (a * scale + 127) / 255;
        let nr = ((r * scale + 127) / 255).min(na);
        let ng = ((g * scale + 127) / 255).min(na);
        let nb = ((b * scale + 127) / 255).min(na);
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

fn render_and_place(hwnd: HWND) {
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
    )) = snapshot
    else {
        return;
    };
    let owner = HWND(owner_raw as *mut c_void);
    let (work, dpi) = target_monitor_info(hwnd, owner, cfg.pos);
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
    let (w, h) = widget_pixel_size(&day_str, cfg.size_pct, dpi);
    let (sx, sy) = resolve_screen_position(cfg.pos, work, w, h);

    // SAFETY: owns offscreen memory DC and 32-bit DIBSection strictly for this synchronous frame update.
    unsafe {
        let mem_dc = CreateCompatibleDC(None);
        if mem_dc.0.is_null() {
            return;
        }
        let mut bits = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(
            mem_dc,
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
            let _ = DeleteDC(mem_dc);
            return;
        };
        if !bits.is_null() {
            let prev_bmp = SelectObject(mem_dc, HGDIOBJ(bmp.0));
            let total = (w as usize).saturating_mul(h as usize);
            let buf = std::slice::from_raw_parts_mut(bits as *mut u32, total);
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
            if last_bounds != Some(next_bounds) || !IsWindowVisible(hwnd).as_bool() {
                if behind_attached && let Some(l) = cached_layer.filter(|&l| layer_still_valid(l)) {
                    let after = if l.raised { l.icons } else { HWND_TOP };
                    IN_ZORDER_SYNC.with(|g| g.set(true));
                    let _ = SetWindowPos(
                        hwnd,
                        after,
                        dst.x,
                        dst.y,
                        w,
                        h,
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                    IN_ZORDER_SYNC.with(|g| g.set(false));
                } else {
                    place_popup_on_desktop(hwnd, dst.x, dst.y, w, h, true, repositioning);
                }
            }

            if has_dcomp {
                let ok = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow_mut()
                        .as_mut()
                        .and_then(|rt| rt.dcomp.as_mut())
                        .is_some_and(|dc| dc.present_bgra(buf, w as u32, h as u32))
                });
                if !ok {
                    // Rebuild presenter cleanly on device removal/display change.
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
                let size = SIZE { cx: w, cy: h };
                let src = POINT { x: 0, y: 0 };
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
                let _ = UpdateLayeredWindow(
                    hwnd,
                    None,
                    Some(&dst),
                    Some(&size),
                    mem_dc,
                    Some(&src),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                );
            }

            SelectObject(mem_dc, prev_bmp);
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
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem_dc);
    }
    sync_clock_input();
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
        (CLOCK_CTX_SIZE_DELTA + 1, "Bigger\tScroll up"),
        (CLOCK_CTX_SIZE_DELTA + 2, "Smaller\tScroll down"),
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
                    let _ = PostMessageW(owner, action_msg, WPARAM(CLOCK_REPOSITION), LPARAM(0));
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
        if choice == CLOCK_CTX_SIZE_DELTA + 1 {
            CLOCK_WHEEL_STEPS.with(|s| *s.borrow_mut() += 2);
            unsafe {
                let _ = PostMessageW(owner, action_msg, WPARAM(CLOCK_CTX_SIZE_DELTA), LPARAM(0));
            }
        } else if choice == CLOCK_CTX_SIZE_DELTA + 2 {
            CLOCK_WHEEL_STEPS.with(|s| *s.borrow_mut() -= 2);
            unsafe {
                let _ = PostMessageW(owner, action_msg, WPARAM(CLOCK_CTX_SIZE_DELTA), LPARAM(0));
            }
        } else {
            unsafe {
                let _ = PostMessageW(owner, action_msg, WPARAM(choice), LPARAM(0));
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
                    if !IN_ZORDER_SYNC.with(Cell::get) {
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
                                LPARAM(0),
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
                        LPARAM(0),
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
            WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => {
                let info = CLOCK_RUNTIME.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .filter(|rt| rt.repositioning)
                        .map(|rt| (rt.owner, rt.action_msg, rt.cfg))
                });
                if let Some((owner, action_msg, cfg)) = info {
                    if owner != 0 {
                        let _ = PostMessageW(
                            HWND(owner as *mut c_void),
                            action_msg,
                            WPARAM(CLOCK_REPOSITION),
                            LPARAM(0),
                        );
                    } else {
                        end_reposition(HWND::default(), action_msg, cfg);
                    }
                }
                LRESULT(0)
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
    closing: bool,
}

const SAMPLE_SIDE: i32 = 9;
const LOUPE_W: i32 = 152;
const LOUPE_H: i32 = 62;

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
            LOUPE_W,
            LOUPE_H,
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

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        update_picker_loupe(pt);
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
        if st.sample_bmp != 0 {
            let _ = DeleteObject(HGDIOBJ(st.sample_bmp as *mut c_void));
        }
        if st.sample_dc != 0 {
            let _ = DeleteDC(HDC(st.sample_dc as *mut c_void));
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
        render_and_place(HWND(raw as *mut c_void));
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
                    LPARAM(0),
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
    let meta = COLOR_PICK_SESSION.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|st| (st.loupe_hwnd, st.sample_dc, st.sample_bits))
    });
    let Some((loupe_raw, sdc_raw, sbits_raw)) = meta else {
        return;
    };
    if loupe_raw == 0 || sdc_raw == 0 || sbits_raw == 0 {
        return;
    }

    let mut samples = [0u32; (SAMPLE_SIDE * SAMPLE_SIDE) as usize];
    // SAFETY: reads 9x9 virtual screen pixels around cursor and updates compact LOUPE_W x LOUPE_H layered preview.
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

    // Build compact dark glass loupe DIB with 9x9 zoom grid + color swatch + hex code.
    unsafe {
        let mem_dc = CreateCompatibleDC(None);
        if mem_dc.0.is_null() {
            return;
        }
        let mut bits = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(
            mem_dc,
            &BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: LOUPE_W,
                    biHeight: -LOUPE_H,
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
            let _ = DeleteDC(mem_dc);
            return;
        };
        if !bits.is_null() {
            let prev = SelectObject(mem_dc, HGDIOBJ(bmp.0));
            let buf =
                std::slice::from_raw_parts_mut(bits as *mut u32, (LOUPE_W * LOUPE_H) as usize);
            // Dark rounded pill background (#181818 @ 235 alpha).
            for y in 0..LOUPE_H {
                for x in 0..LOUPE_W {
                    let border = x == 0 || y == 0 || x == LOUPE_W - 1 || y == LOUPE_H - 1;
                    buf[(y * LOUPE_W + x) as usize] =
                        if border { 0xFF42_4242 } else { 0xEB18_1818 };
                }
            }
            // Left 9x9 zoomed pixel square (8, 8) at 5x5 per sample -> 45x45.
            let cell = 5i32;
            let zx0 = 8i32;
            let zy0 = 8i32;
            for sy in 0..SAMPLE_SIDE {
                for sx in 0..SAMPLE_SIDE {
                    let col = samples[(sy * SAMPLE_SIDE + sx) as usize];
                    let px_col = 0xFF00_0000 | col;
                    for dy in 0..cell {
                        for dx in 0..cell {
                            let gx = zx0 + sx * cell + dx;
                            let gy = zy0 + sy * cell + dy;
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
                            buf[(gy * LOUPE_W + gx) as usize] = final_px;
                        }
                    }
                }
            }
            // Color swatch circle/square next to zoom grid (x=62..80, y=12..30).
            for y in 12..30 {
                for x in 62..80 {
                    let edge = x == 62 || x == 79 || y == 12 || y == 29;
                    buf[(y * LOUPE_W + x) as usize] = if edge {
                        0xFFAA_AAAA
                    } else {
                        0xFF00_0000 | center_col
                    };
                }
            }

            let hex = format!("#{:06X}", center_col);
            let mut canvas = GlyphCanvas {
                buf,
                width: LOUPE_W,
                height: LOUPE_H,
            };
            draw_quicksand_line(
                &mut canvas,
                &hex,
                RECT {
                    left: 84,
                    top: 10,
                    right: LOUPE_W - 6,
                    bottom: 32,
                },
                14,
                0,
                0x00FF_FFFF,
            );
            draw_quicksand_line(
                &mut canvas,
                "Click pick / Esc cancel",
                RECT {
                    left: 60,
                    top: 34,
                    right: LOUPE_W - 6,
                    bottom: 54,
                },
                11,
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
            let lx = if pt.x + 20 + LOUPE_W <= mon_r.right {
                pt.x + 20
            } else {
                pt.x - LOUPE_W - 16
            };
            let ly = if pt.y + 20 + LOUPE_H <= mon_r.bottom {
                pt.y + 20
            } else {
                pt.y - LOUPE_H - 16
            };

            let loupe = HWND(loupe_raw as *mut c_void);
            let dst = POINT { x: lx, y: ly };
            let size = SIZE {
                cx: LOUPE_W,
                cy: LOUPE_H,
            };
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
                LOUPE_W,
                LOUPE_H,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            SelectObject(mem_dc, prev);
        }
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem_dc);
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
    }
}
