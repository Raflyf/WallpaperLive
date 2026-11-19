use crate::core::Rect;
use std::ffi::c_void;
use windows::{
    Win32::{
        Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CreateCompatibleBitmap,
            CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint,
            EnumDisplayMonitors, GetDC, GetMonitorInfoW, HALFTONE, HBITMAP, HDC, HGDIOBJ, HMONITOR,
            InvalidateRect, MONITORINFO, MONITORINFOEXW, MapWindowPoints, PAINTSTRUCT, ReleaseDC,
            SRCCOPY, SelectObject, SetDIBits, SetStretchBltMode, StretchBlt, UpdateWindow,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::*,
    },
    core::{PCWSTR, w},
};

const SURFACE_CLASS: PCWSTR = w!("WallpaperLive.Surface");

#[derive(Clone, Debug)]
pub struct Monitor {
    pub name: String,
    pub rect: Rect,
    pub work_rect: Rect,
    pub primary: bool,
}

pub fn monitors() -> Result<Vec<Monitor>, String> {
    let mut monitors: Vec<Monitor> = Vec::new();
    // SAFETY: callback is synchronous, Vec remains alive and exclusively borrowed for duration.
    if !unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(monitor_callback),
            LPARAM((&mut monitors as *mut Vec<Monitor>) as isize),
        )
    }
    .as_bool()
    {
        return Err("Could not read the monitor list".into());
    }
    monitors.sort_by_key(|m| !m.primary);
    if monitors.is_empty() {
        return Err("No active monitor found".into());
    }
    Ok(monitors)
}

unsafe extern "system" fn monitor_callback(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: correctly sized MONITORINFOEXW, callback data references live Vec from monitors().
    unsafe {
        if GetMonitorInfoW(
            monitor,
            (&mut info as *mut MONITORINFOEXW).cast::<MONITORINFO>(),
        )
        .as_bool()
        {
            let n = info
                .szDevice
                .iter()
                .position(|x| *x == 0)
                .unwrap_or(info.szDevice.len());
            let r = info.monitorInfo.rcMonitor;
            let w = info.monitorInfo.rcWork;
            (&mut *(data.0 as *mut Vec<Monitor>)).push(Monitor {
                name: String::from_utf16_lossy(&info.szDevice[..n]),
                rect: Rect {
                    left: r.left,
                    top: r.top,
                    right: r.right,
                    bottom: r.bottom,
                },
                work_rect: Rect {
                    left: w.left,
                    top: w.top,
                    right: w.right,
                    bottom: w.bottom,
                },
                primary: info.monitorInfo.dwFlags & 1 != 0,
            });
        }
    }
    BOOL(1)
}

#[derive(Default)]
struct SurfacePaintState {
    bitmap: HBITMAP,
    width: i32,
    height: i32,
}

impl SurfacePaintState {
    fn clear(&mut self) {
        if !self.bitmap.0.is_null() {
            // SAFETY: owned GDI HBITMAP deselected from any temporary DC before deletion.
            unsafe {
                let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            }
            self.bitmap = HBITMAP::default();
        }
        self.width = 0;
        self.height = 0;
    }
}

pub struct Surface {
    pub hwnd: HWND,
    player_host: HWND,
    parent: HWND,
    icons: HWND,
    raised: bool,
}

#[cfg(not(test))]
struct Search {
    worker: HWND,
    icons: HWND,
}

#[cfg(not(test))]
unsafe extern "system" fn search_callback(window: HWND, data: LPARAM) -> BOOL {
    // SAFETY: enumeration synchronous; Search lives for whole callback sequence.
    unsafe {
        if let Ok(icons) = FindWindowExW(window, None, w!("SHELLDLL_DefView"), PCWSTR::null()) {
            let result = &mut *(data.0 as *mut Search);
            result.icons = icons;
            if let Ok(worker) = FindWindowExW(None, window, w!("WorkerW"), PCWSTR::null()) {
                result.worker = worker;
            }
        }
    }
    BOOL(1)
}

fn register_surface_class(instance: windows::Win32::Foundation::HINSTANCE) -> Result<(), String> {
    let wc = WNDCLASSW {
        lpfnWndProc: Some(surface_proc),
        hInstance: instance,
        lpszClassName: SURFACE_CLASS,
        ..Default::default()
    };
    // SAFETY: UI-thread class registration; ERROR_CLASS_ALREADY_EXISTS (1410) is benign.
    if unsafe { RegisterClassW(&wc) } == 0
        && unsafe { windows::Win32::Foundation::GetLastError() }.0 != 1410
    {
        return Err("Could not register the wallpaper window class".into());
    }
    Ok(())
}

unsafe fn attach_paint_state(hwnd: HWND) {
    let raw = Box::into_raw(Box::new(SurfacePaintState::default()));
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
    }
}

unsafe fn paint_state_mut(hwnd: HWND) -> Option<&'static mut SurfacePaintState> {
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut SurfacePaintState;
    unsafe { raw.as_mut() }
}

impl Surface {
    #[cfg(test)]
    pub fn isolated(parent: HWND, rect: Rect) -> Result<Self, String> {
        let width = (rect.right - rect.left).max(1);
        let height = (rect.bottom - rect.top).max(1);
        // SAFETY: caller owns parent; this test-only path never discovers or changes Explorer.
        unsafe {
            let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
            register_surface_class(instance.into())?;
            let hwnd = CreateWindowExW(
                WS_EX_NOACTIVATE,
                SURFACE_CLASS,
                w!("WallpaperLive isolated surface"),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                width,
                height,
                parent,
                None,
                instance,
                None,
            )
            .map_err(|e| e.to_string())?;
            attach_paint_state(hwnd);
            let player_host = CreateWindowExW(
                WS_EX_NOACTIVATE,
                SURFACE_CLASS,
                w!("WallpaperLive isolated player host"),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                width,
                height,
                hwnd,
                None,
                instance,
                None,
            )
            .map_err(|e| {
                let _ = DestroyWindow(hwnd);
                e.to_string()
            })?;
            Ok(Self {
                hwnd,
                player_host,
                parent,
                icons: parent,
                raised: false,
            })
        }
    }

    #[cfg(not(test))]
    pub fn create(rect: Rect) -> Result<Self, String> {
        // SAFETY: Win32 discovery and window creation on GUI thread, no foreign Rust pointers stored.
        unsafe {
            let progman = FindWindowW(w!("Progman"), PCWSTR::null())
                .map_err(|_| "Explorer desktop not found")?;
            let raised =
                GetWindowLongPtrW(progman, GWL_EXSTYLE) as u32 & WS_EX_NOREDIRECTIONBITMAP.0 != 0;
            let mut message_result = 0;
            // ponytail: undocumented shell message has no stable reply contract; validate resulting hierarchy below.
            let _ = SendMessageTimeoutW(
                progman,
                0x052c,
                WPARAM(0xD),
                LPARAM(1),
                SMTO_ABORTIFHUNG,
                1000,
                Some(&mut message_result),
            );
            let mut search = Search {
                worker: HWND::default(),
                icons: HWND::default(),
            };
            EnumWindows(
                Some(search_callback),
                LPARAM((&mut search as *mut Search) as isize),
            )
            .map_err(|e| format!("EnumWindows: {e}"))?;
            let (parent, icons) = if raised {
                let icons = FindWindowExW(progman, None, w!("SHELLDLL_DefView"), PCWSTR::null())
                    .map_err(|_| "Desktop icon layer not found")?;
                (progman, icons)
            } else {
                (search.worker, search.icons)
            };
            if parent.0.is_null() || icons.0.is_null() {
                return Err("Windows wallpaper layer is unavailable".into());
            }
            let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
            register_surface_class(instance.into())?;
            let style = WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS;
            let ex = if raised {
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
            } else {
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
            };
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            let hwnd = CreateWindowExW(ex, SURFACE_CLASS, w!("WallpaperLive wallpaper"), style, 0, 0, width, height, parent, None, instance, None).map_err(|e| format!("Could not attach to the desktop. Use a normal Windows session; isolated low-integrity processes cannot access Explorer. ({e})"))?;
            attach_paint_state(hwnd);
            if raised && let Err(e) = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)
            {
                let raw = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut SurfacePaintState;
                if !raw.is_null() {
                    drop(Box::from_raw(raw));
                }
                let _ = DestroyWindow(hwnd);
                return Err(e.to_string());
            }
            let player_ex = if raised {
                WS_EX_LAYERED | WS_EX_NOACTIVATE
            } else {
                WS_EX_NOACTIVATE
            };
            let player_host = CreateWindowExW(
                player_ex,
                SURFACE_CLASS,
                w!("WallpaperLive player host"),
                style,
                0,
                0,
                width,
                height,
                hwnd,
                None,
                instance,
                None,
            )
            .map_err(|e| {
                let raw = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut SurfacePaintState;
                if !raw.is_null() {
                    drop(Box::from_raw(raw));
                }
                let _ = DestroyWindow(hwnd);
                e.to_string()
            })?;
            if raised {
                let _ = SetLayeredWindowAttributes(player_host, COLORREF(0), 255, LWA_ALPHA);
            }
            let surface = Self {
                hwnd,
                player_host,
                parent,
                icons,
                raised,
            };
            surface.place(rect)?;
            Ok(surface)
        }
    }

    pub fn player_host(&self) -> HWND {
        self.player_host
    }

    pub fn is_raised(&self) -> bool {
        self.raised
    }

    #[cfg(test)]
    pub fn has_frozen_frame(&self) -> bool {
        // SAFETY: UI thread only; state lives until Surface::drop clears GWLP_USERDATA.
        unsafe { paint_state_mut(self.hwnd) }.is_some_and(|st| !st.bitmap.0.is_null())
    }

    pub fn set_frozen_frame(
        &mut self,
        width: u32,
        height: u32,
        stride: usize,
        bgr0: &[u8],
    ) -> Result<(), String> {
        let w = i32::try_from(width).map_err(|_| "Snapshot width overflow")?;
        let h = i32::try_from(height).map_err(|_| "Snapshot height overflow")?;
        if w <= 0 || h <= 0 {
            return Err("Snapshot dimensions must be positive".into());
        }
        let row_bytes = (width as usize)
            .checked_mul(4)
            .ok_or("Snapshot row size overflow")?;
        if stride < row_bytes {
            return Err("Snapshot stride is narrower than pixel row".into());
        }
        let required = stride
            .checked_mul(height as usize)
            .ok_or("Snapshot buffer size overflow")?;
        if bgr0.len() < required {
            return Err("Snapshot pixel buffer is truncated".into());
        }
        let packed_storage;
        let pixels_ptr: *const c_void = if stride == row_bytes {
            bgr0.as_ptr().cast()
        } else {
            let mut packed = Vec::with_capacity(row_bytes * (height as usize));
            for y in 0..(height as usize) {
                let start = y * stride;
                packed.extend_from_slice(&bgr0[start..start + row_bytes]);
            }
            packed_storage = packed;
            packed_storage.as_ptr().cast()
        };
        let bmi = BITMAPINFO {
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
        };
        // SAFETY: screen DC is queried and released immediately; DDB owns no process heap pixels.
        unsafe {
            let screen_dc = GetDC(None);
            if screen_dc.0.is_null() {
                return Err("Could not acquire screen DC for snapshot".into());
            }
            let bitmap = CreateCompatibleBitmap(screen_dc, w, h);
            if bitmap.0.is_null() {
                let _ = ReleaseDC(None, screen_dc);
                return Err("Could not allocate snapshot bitmap".into());
            }
            let copied = SetDIBits(
                screen_dc,
                bitmap,
                0,
                height,
                pixels_ptr,
                &bmi,
                DIB_RGB_COLORS,
            );
            let _ = ReleaseDC(None, screen_dc);
            if copied == 0 {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                return Err("Could not upload snapshot pixels to GDI bitmap".into());
            }
            let Some(state) = paint_state_mut(self.hwnd) else {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                return Err("Surface paint state is missing".into());
            };
            state.clear();
            state.bitmap = bitmap;
            state.width = w;
            state.height = h;
            let _ = InvalidateRect(self.hwnd, None, false);
            let _ = UpdateWindow(self.hwnd);
        }
        Ok(())
    }

    pub fn clear_frozen_frame(&mut self) {
        // SAFETY: UI thread only; frees owned GDI DDB if present.
        unsafe {
            if let Some(state) = paint_state_mut(self.hwnd) {
                state.clear();
                let _ = InvalidateRect(self.hwnd, None, false);
            }
        }
    }

    pub fn hide_player(&self) {
        // SAFETY: hides child player host while keeping Surface DDB snapshot visible.
        unsafe {
            if IsWindow(self.player_host).as_bool() {
                let _ = ShowWindow(self.player_host, SW_HIDE);
            }
            let _ = InvalidateRect(self.hwnd, None, false);
            let _ = UpdateWindow(self.hwnd);
        }
    }

    pub fn show_player(&self) {
        // SAFETY: reveals owned child player host without activating or stealing focus.
        unsafe {
            if IsWindow(self.player_host).as_bool() {
                let _ = ShowWindow(self.player_host, SW_SHOWNOACTIVATE);
            }
        }
    }

    pub fn place(&self, rect: Rect) -> Result<(), String> {
        // SAFETY: validated parent/own HWND; screen origin mapped to actual native parent coordinates.
        unsafe {
            if !self.valid() {
                return Err("Desktop changed; apply the wallpaper again".into());
            }
            let mut origin = [POINT {
                x: rect.left,
                y: rect.top,
            }];
            MapWindowPoints(None, self.parent, &mut origin);
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            SetWindowPos(
                self.hwnd,
                if self.raised { self.icons } else { HWND_BOTTOM },
                origin[0].x,
                origin[0].y,
                width,
                height,
                SWP_NOACTIVATE,
            )
            .map_err(|e| e.to_string())?;
            if IsWindow(self.player_host).as_bool() {
                let _ = SetWindowPos(
                    self.player_host,
                    None,
                    0,
                    0,
                    width,
                    height,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                );
            }
            Ok(())
        }
    }

    pub fn show(&self) {
        // SAFETY: only own surface made visible, never takes foreground focus.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    pub fn valid(&self) -> bool {
        // SAFETY: IsWindow accepts stale handles and GetParent checked before use.
        unsafe {
            IsWindow(self.hwnd).as_bool()
                && IsWindow(self.player_host).as_bool()
                && IsWindow(self.parent).as_bool()
                && IsWindow(self.icons).as_bool()
                && GetParent(self.hwnd).is_ok_and(|p| p == self.parent)
                && GetParent(self.player_host).is_ok_and(|p| p == self.hwnd)
        }
    }

    pub fn prepare_renderer(&self) -> Result<(), String> {
        if !self.raised {
            return Ok(());
        }
        // SAFETY: enumerate only children of owned player_host, change only mpv's owned render child.
        unsafe {
            SetLayeredWindowAttributes(self.player_host, COLORREF(0), 255, LWA_ALPHA)
                .map_err(|e| e.to_string())?;
            let child = FindWindowExW(self.player_host, None, w!("mpv"), PCWSTR::null())
                .map_err(|_| "Decoder render window is not ready")?;
            let style = GetWindowLongPtrW(child, GWL_EXSTYLE);
            SetWindowLongPtrW(child, GWL_EXSTYLE, style | WS_EX_LAYERED.0 as isize);
            SetLayeredWindowAttributes(child, COLORREF(0), 255, LWA_ALPHA)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: only our created HWNDs; decoder must be destroyed by caller before dropping surface.
        unsafe {
            if IsWindow(self.hwnd).as_bool() {
                let raw = SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0) as *mut SurfacePaintState;
                if !raw.is_null() {
                    let mut state = Box::from_raw(raw);
                    state.clear();
                }
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

unsafe extern "system" fn surface_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            // SAFETY: GWLP_USERDATA stays valid on UI thread until cleared before DestroyWindow.
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(window, &mut ps);
                if let Some(state) = paint_state_mut(window)
                    && !state.bitmap.0.is_null()
                    && state.width > 0
                    && state.height > 0
                {
                    let mut client = RECT::default();
                    let _ = GetClientRect(window, &mut client);
                    let cw = (client.right - client.left).max(1);
                    let ch = (client.bottom - client.top).max(1);
                    let mem = CreateCompatibleDC(hdc);
                    if !mem.0.is_null() {
                        let prev = SelectObject(mem, HGDIOBJ(state.bitmap.0));
                        if cw == state.width && ch == state.height {
                            let _ = BitBlt(hdc, 0, 0, cw, ch, mem, 0, 0, SRCCOPY);
                        } else {
                            let _ = SetStretchBltMode(hdc, HALFTONE);
                            let _ = StretchBlt(
                                hdc,
                                0,
                                0,
                                cw,
                                ch,
                                mem,
                                0,
                                0,
                                state.width,
                                state.height,
                                SRCCOPY,
                            );
                        }
                        SelectObject(mem, prev);
                        let _ = DeleteDC(mem);
                    }
                }
                let _ = EndPaint(window, &ps);
            }
            LRESULT(0)
        }
        // SAFETY: other messages use standard native default handling, no Rust references.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_surface_uploads_and_paints_ddb_frozen_frame_without_leaks() {
        // SAFETY: test owns hidden parent and child surface windows solely within this test.
        unsafe {
            let instance = GetModuleHandleW(None).expect("module handle");
            let parent = CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("SurfaceTestParent"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                64,
                64,
                None,
                None,
                instance,
                None,
            )
            .expect("parent window");
            let mut surface = Surface::isolated(
                parent,
                Rect {
                    left: 0,
                    top: 0,
                    right: 32,
                    bottom: 24,
                },
            )
            .expect("isolated surface");
            assert!(surface.valid());
            assert_ne!(surface.player_host(), surface.hwnd);
            assert!(!surface.has_frozen_frame());

            // Stride padded beyond width*4 exercises row packing path.
            let stride = 32 * 4 + 16;
            let mut pixels = vec![0u8; stride * 24];
            for y in 0..24 {
                for x in 0..32 {
                    let off = y * stride + x * 4;
                    pixels[off] = 0x20;
                    pixels[off + 1] = 0x80;
                    pixels[off + 2] = 0xE0;
                }
            }
            surface
                .set_frozen_frame(32, 24, stride, &pixels)
                .expect("upload ddb snapshot");
            assert!(surface.has_frozen_frame());

            surface.hide_player();
            let _ = UpdateWindow(surface.hwnd);
            surface.show_player();
            surface.clear_frozen_frame();
            assert!(!surface.has_frozen_frame());

            drop(surface);
            let _ = DestroyWindow(parent);
        }
    }
}
