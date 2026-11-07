use crate::core::Rect;
#[cfg(not(test))]
use windows::Win32::Foundation::{LRESULT, WPARAM};
use windows::{
    Win32::{
        Foundation::{BOOL, COLORREF, HWND, LPARAM, POINT, RECT},
        Graphics::Gdi::{
            EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
            MapWindowPoints,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::*,
    },
    core::{PCWSTR, w},
};

#[derive(Clone, Debug)]
pub struct Monitor {
    pub name: String,
    pub rect: Rect,
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
            (&mut *(data.0 as *mut Vec<Monitor>)).push(Monitor {
                name: String::from_utf16_lossy(&info.szDevice[..n]),
                rect: Rect {
                    left: r.left,
                    top: r.top,
                    right: r.right,
                    bottom: r.bottom,
                },
                primary: info.monitorInfo.dwFlags & 1 != 0,
            });
        }
    }
    BOOL(1)
}

pub struct Surface {
    pub hwnd: HWND,
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

impl Surface {
    #[cfg(test)]
    pub fn isolated(parent: HWND, rect: Rect) -> Result<Self, String> {
        // SAFETY: caller owns parent; this test-only path never discovers or changes Explorer.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("WallpaperLive isolated surface"),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                rect.right - rect.left,
                rect.bottom - rect.top,
                parent,
                None,
                GetModuleHandleW(None).map_err(|e| e.to_string())?,
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        Ok(Self {
            hwnd,
            parent,
            icons: parent,
            raised: false,
        })
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
            let class = w!("WallpaperLive.Surface");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(surface_proc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 && windows::Win32::Foundation::GetLastError().0 != 1410 {
                return Err("Could not register the wallpaper window class".into());
            }
            let style = WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS;
            let ex = if raised {
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
            } else {
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE
            };
            let hwnd = CreateWindowExW(ex, class, w!("WallpaperLive wallpaper"), style, 0, 0, rect.right-rect.left, rect.bottom-rect.top, parent, None, instance, None).map_err(|e| format!("Could not attach to the desktop. Use a normal Windows session; isolated low-integrity processes cannot access Explorer. ({e})"))?;
            let surface = Self {
                hwnd,
                parent,
                icons,
                raised,
            };
            if raised {
                SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)
                    .map_err(|e| e.to_string())?;
            }
            surface.place(rect)?;
            Ok(surface)
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
            SetWindowPos(
                self.hwnd,
                if self.raised { self.icons } else { HWND_BOTTOM },
                origin[0].x,
                origin[0].y,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOACTIVATE,
            )
            .map_err(|e| e.to_string())?;
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
                && IsWindow(self.parent).as_bool()
                && IsWindow(self.icons).as_bool()
                && GetParent(self.hwnd).is_ok_and(|p| p == self.parent)
        }
    }
    pub fn prepare_renderer(&self) -> Result<(), String> {
        if !self.raised {
            return Ok(());
        }
        // SAFETY: enumerate only children of owned host, change only mpv's owned render child.
        unsafe {
            let child = FindWindowExW(self.hwnd, None, w!("mpv"), PCWSTR::null())
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
        // SAFETY: only our created HWND; decoder must be destroyed by caller before dropping surface.
        unsafe {
            if IsWindow(self.hwnd).as_bool() {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}
#[cfg(not(test))]
unsafe extern "system" fn surface_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // SAFETY: other messages use standard native default handling, no Rust references.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}
