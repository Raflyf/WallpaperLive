use crate::core::validate_media;
use std::{
    ffi::OsStr,
    fs::OpenOptions,
    io::Read,
    mem::{size_of, size_of_val},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SIZE, SetHandleInformation, WAIT_OBJECT_0,
            WAIT_TIMEOUT,
        },
        Graphics::Gdi::*,
        System::{
            Com::*,
            Threading::{
                CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
                GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
                LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTF_FORCEOFFFEEDBACK,
                STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW, STARTUPINFOW_FLAGS,
                TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
            },
        },
        UI::Shell::*,
    },
    core::{PCWSTR, PWSTR},
};

const WIDTH: i32 = 480;
const HEIGHT: i32 = 270;
const LIMIT: u64 = (WIDTH * HEIGHT * 4 + 8) as u64;
const MAX_SOURCE_DIMENSION: i32 = 8192;
// Suppresses Windows' background-launch hourglass cursor (IDC_APPSTARTING) for short GUI workers.
const SILENT_STARTUP_FLAGS: STARTUPINFOW_FLAGS =
    STARTUPINFOW_FLAGS(STARTF_USESTDHANDLES.0 | STARTF_FORCEOFFFEEDBACK.0);

static SPAWN_LOCK: Mutex<()> = Mutex::new(());

fn append_quoted_win32_arg(out: &mut Vec<u16>, arg: &OsStr, force_quote: bool) {
    let units: Vec<u16> = arg.encode_wide().collect();
    let needs_quotes = force_quote
        || units.is_empty()
        || units
            .iter()
            .any(|&u| u == b' ' as u16 || u == b'\t' as u16 || u == b'"' as u16);
    if !needs_quotes {
        out.extend_from_slice(&units);
        return;
    }
    out.push(b'"' as u16);
    let mut backslashes = 0usize;
    for &u in &units {
        if u == b'\\' as u16 {
            backslashes += 1;
        } else if u == b'"' as u16 {
            out.resize(out.len() + backslashes * 2 + 1, b'\\' as u16);
            out.push(b'"' as u16);
            backslashes = 0;
        } else {
            out.resize(out.len() + backslashes, b'\\' as u16);
            out.push(u);
            backslashes = 0;
        }
    }
    out.resize(out.len() + backslashes * 2, b'\\' as u16);
    out.push(b'"' as u16);
}

fn build_windows_command_line(exe: &Path, args: &[&OsStr]) -> Vec<u16> {
    let mut out = Vec::new();
    append_quoted_win32_arg(&mut out, exe.as_os_str(), true);
    for arg in args {
        out.push(b' ' as u16);
        append_quoted_win32_arg(&mut out, arg, false);
    }
    out.push(0);
    out
}

struct ProcAttrList {
    _buf: Vec<u64>,
    ptr: LPPROC_THREAD_ATTRIBUTE_LIST,
}

impl Drop for ProcAttrList {
    fn drop(&mut self) {
        if !self.ptr.0.is_null() {
            // SAFETY: initialized by InitializeProcThreadAttributeList while `_buf` stays alive.
            unsafe {
                DeleteProcThreadAttributeList(self.ptr);
            }
        }
    }
}

pub(crate) struct SilentChild {
    process: OwnedHandle,
    pid: u32,
    stdout: Option<std::io::PipeReader>,
    #[cfg(test)]
    _stdin: Option<std::io::PipeWriter>,
}

impl SilentChild {
    pub(crate) fn spawn(exe: &Path, args: &[&OsStr], creation_flags: u32) -> Result<Self, String> {
        Self::spawn_inner(exe, args, creation_flags, false)
    }

    #[cfg(test)]
    pub(crate) fn spawn_with_piped_stdin(
        exe: &Path,
        args: &[&OsStr],
        creation_flags: u32,
    ) -> Result<Self, String> {
        Self::spawn_inner(exe, args, creation_flags, true)
    }

    fn spawn_inner(
        exe: &Path,
        args: &[&OsStr],
        creation_flags: u32,
        pipe_stdin: bool,
    ) -> Result<Self, String> {
        let (stdout_reader, stdout_writer) =
            std::io::pipe().map_err(|e| format!("Could not create worker pipe: {e}"))?;
        let nul_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(r"\\.\NUL")
            .map_err(|e| format!("Could not open NUL device: {e}"))?;
        #[cfg(test)]
        let (stdin_reader, stdin_writer) = if pipe_stdin {
            let (r, w) =
                std::io::pipe().map_err(|e| format!("Could not create worker stdin pipe: {e}"))?;
            (Some(r), Some(w))
        } else {
            (None, None)
        };
        #[cfg(not(test))]
        let stdin_reader = {
            let _ = pipe_stdin;
            None::<std::io::PipeReader>
        };

        let app_wide: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut cmd_wide = build_windows_command_line(exe, args);

        let pi = {
            let _lock = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            // Keep inheritable handles inner-scoped so Rust drops them strictly before `_lock`.
            let _nul_guard = nul_file;
            let _stdout_guard = stdout_writer;
            let _stdin_guard = stdin_reader;
            let nul_handle = HANDLE(_nul_guard.as_raw_handle());
            let stdout_write_handle = HANDLE(_stdout_guard.as_raw_handle());
            let stdin_handle = _stdin_guard
                .as_ref()
                .map_or(nul_handle, |r| HANDLE(r.as_raw_handle()));
            let inherit_storage = [nul_handle, stdout_write_handle, stdin_handle];
            let inherit_handles = if pipe_stdin {
                &inherit_storage[..3]
            } else {
                &inherit_storage[..2]
            };

            // SAFETY: `attr_list._buf`, `inherit_handles`, `app_wide`, and `cmd_wide` stay alive across CreateProcessW.
            unsafe {
                for &h in inherit_handles {
                    SetHandleInformation(h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT)
                        .map_err(|e| format!("Could not mark worker handle inheritable: {e}"))?;
                }
                let mut attr_size = 0usize;
                let _ = InitializeProcThreadAttributeList(
                    LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut()),
                    1,
                    0,
                    &mut attr_size,
                );
                if attr_size == 0 {
                    return Err("Could not size worker attribute list".into());
                }
                let mut attr_list = ProcAttrList {
                    _buf: vec![0u64; attr_size.div_ceil(size_of::<u64>())],
                    ptr: LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut()),
                };
                let list_ptr = LPPROC_THREAD_ATTRIBUTE_LIST(attr_list._buf.as_mut_ptr().cast());
                InitializeProcThreadAttributeList(list_ptr, 1, 0, &mut attr_size)
                    .map_err(|e| format!("Could not initialize worker attribute list: {e}"))?;
                attr_list.ptr = list_ptr;
                UpdateProcThreadAttribute(
                    attr_list.ptr,
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                    Some(inherit_handles.as_ptr().cast()),
                    size_of_val(inherit_handles),
                    None,
                    None,
                )
                .map_err(|e| format!("Could not scope worker inherited handles: {e}"))?;

                let startup = STARTUPINFOEXW {
                    StartupInfo: STARTUPINFOW {
                        cb: size_of::<STARTUPINFOEXW>() as u32,
                        dwFlags: SILENT_STARTUP_FLAGS,
                        hStdInput: stdin_handle,
                        hStdOutput: stdout_write_handle,
                        hStdError: nul_handle,
                        ..Default::default()
                    },
                    lpAttributeList: attr_list.ptr,
                };
                let mut pi = PROCESS_INFORMATION::default();
                CreateProcessW(
                    PCWSTR(app_wide.as_ptr()),
                    PWSTR(cmd_wide.as_mut_ptr()),
                    None,
                    None,
                    true,
                    PROCESS_CREATION_FLAGS(creation_flags | EXTENDED_STARTUPINFO_PRESENT.0),
                    None,
                    PCWSTR::null(),
                    &startup.StartupInfo,
                    &mut pi,
                )
                .map_err(|e| format!("Could not start worker process: {e}"))?;
                if !pi.hThread.is_invalid() {
                    let _ = CloseHandle(pi.hThread);
                }
                let _keep_alive = &inherit_storage;
                pi
            }
        };

        if pi.hProcess.is_invalid() {
            return Err("Worker process returned an invalid handle".into());
        }
        // SAFETY: `pi.hProcess` is a verified valid unique HANDLE returned by CreateProcessW.
        let process = unsafe { OwnedHandle::from_raw_handle(pi.hProcess.0) };
        Ok(Self {
            process,
            pid: pi.dwProcessId,
            stdout: Some(stdout_reader),
            #[cfg(test)]
            _stdin: stdin_writer,
        })
    }

    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    pub(crate) fn take_stdout(&mut self) -> Option<std::io::PipeReader> {
        self.stdout.take()
    }

    #[cfg(test)]
    pub(crate) fn try_wait(&mut self) -> Result<Option<bool>, String> {
        self.wait_timeout(0)
    }

    pub(crate) fn wait_timeout(&mut self, timeout_ms: u32) -> Result<Option<bool>, String> {
        let h = HANDLE(self.process.as_raw_handle());
        // SAFETY: `self.process` owns a valid Windows process handle; blocks up to timeout_ms and wakes immediately on exit.
        let wait = unsafe { WaitForSingleObject(h, timeout_ms) };
        if wait == WAIT_TIMEOUT {
            return Ok(None);
        }
        if wait != WAIT_OBJECT_0 {
            return Err(format!("Could not poll worker process state ({})", wait.0));
        }
        let mut exit_code = 1u32;
        // SAFETY: process already signaled WAIT_OBJECT_0; writes 4-byte exit code.
        unsafe { GetExitCodeProcess(h, &mut exit_code) }.map_err(|e| e.to_string())?;
        Ok(Some(exit_code == 0))
    }

    pub(crate) fn kill_and_wait(&mut self) -> Result<(), String> {
        let h = HANDLE(self.process.as_raw_handle());
        // SAFETY: terminates and reaps the owned process handle synchronously.
        unsafe {
            let term = TerminateProcess(h, 1);
            let timeout = if term.is_ok() { INFINITE } else { 5000 };
            let wait = WaitForSingleObject(h, timeout);
            if wait != WAIT_OBJECT_0 {
                return Err(format!("Could not wait for stopped worker ({})", wait.0));
            }
            if term.is_err() {
                let mut exit_code = 0u32;
                if GetExitCodeProcess(h, &mut exit_code).is_err() {
                    term.map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn raw_process_handle(&self) -> HANDLE {
        HANDLE(self.process.as_raw_handle())
    }
}

impl Drop for SilentChild {
    fn drop(&mut self) {
        let h = HANDLE(self.process.as_raw_handle());
        // SAFETY: ensures spawned worker never outlives its handle owner.
        unsafe {
            if WaitForSingleObject(h, 0) != WAIT_OBJECT_0 {
                let _ = TerminateProcess(h, 1);
                let _ = WaitForSingleObject(h, 5000);
            }
        }
    }
}

pub fn capture(path: &Path, cancelled: &std::sync::atomic::AtomicBool) -> Result<Vec<u8>, String> {
    if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("Thumbnail cancelled".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Shell codecs stay in one short-lived child, not in the wallpaper process.
    let child = SilentChild::spawn(
        &exe,
        &[OsStr::new("--thumbnail"), path.as_os_str()],
        0x08004000,
    )
    .map_err(|e| format!("Could not start thumbnail worker: {e}"))?;
    capture_output(child, cancelled)
}
fn capture_output(
    mut child: SilentChild,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Vec<u8>, String> {
    let stdout = child
        .take_stdout()
        .ok_or("Thumbnail worker has no output")?;
    let reader = std::thread::spawn(move || {
        let mut output = Vec::with_capacity((WIDTH * HEIGHT * 4 + 8) as usize);
        stdout
            .take(LIMIT + 1)
            .read_to_end(&mut output)
            .map(|_| output)
    });
    let start = Instant::now();
    let wait_result = loop {
        if cancelled.load(std::sync::atomic::Ordering::Relaxed)
            || start.elapsed() >= Duration::from_secs(10)
        {
            break match child.kill_and_wait() {
                Ok(()) => Err(if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                    "Thumbnail cancelled"
                } else {
                    "Thumbnail worker timed out"
                }
                .into()),
                Err(e) => Err(format!("Could not stop thumbnail worker: {e}")),
            };
        }
        match child.wait_timeout(50) {
            Ok(Some(ok)) => break Ok(ok),
            Ok(None) => {}
            Err(e) => break Err(e),
        }
    };
    drop(child);
    let read_result = reader.join();
    let success = wait_result?;
    let output = read_result
        .map_err(|_| "Thumbnail output reader failed")?
        .map_err(|e| e.to_string())?;
    if !success {
        return Err("Windows could not create this thumbnail".into());
    }
    dimensions(&output)?;
    Ok(output)
}

pub fn extract(path: &Path) -> Result<Vec<u8>, String> {
    let path = validate_media(path)?;
    let text: Vec<_> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: worker owns its COM apartment and every native bitmap/DC; no windows are created.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        let result = (|| {
            let factory: IShellItemImageFactory =
                SHCreateItemFromParsingName(PCWSTR(text.as_ptr()), None)
                    .map_err(|e| e.to_string())?;
            let bitmap = OwnedBitmap(
                factory
                    .GetImage(
                        SIZE {
                            cx: WIDTH,
                            cy: HEIGHT,
                        },
                        SIIGBF_THUMBNAILONLY | SIIGBF_RESIZETOFIT,
                    )
                    .map_err(|e| e.to_string())?,
            );
            encode_bitmap(bitmap.0)
        })();
        CoUninitialize();
        result
    }
}

struct OwnedDc(HDC);
impl Drop for OwnedDc {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            // SAFETY: OwnedDc exclusively owns this memory DC created via CreateCompatibleDC.
            unsafe {
                let _ = DeleteDC(self.0);
            }
        }
    }
}

struct SelectedObject(HDC, HGDIOBJ);
impl Drop for SelectedObject {
    fn drop(&mut self) {
        if !self.0.0.is_null() && !self.1.0.is_null() {
            // SAFETY: restores the DC's prior GDI object before DC/bitmap teardown.
            unsafe {
                SelectObject(self.0, self.1);
            }
        }
    }
}

struct OwnedBitmap(HBITMAP);
impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            // SAFETY: OwnedBitmap exclusively owns this HBITMAP and drops after any SelectedObject guard.
            unsafe {
                let _ = DeleteObject(self.0);
            }
        }
    }
}

fn fit_dimensions(width: i32, height: i32) -> Result<(i32, i32), String> {
    if !(1..=MAX_SOURCE_DIMENSION).contains(&width) || !(1..=MAX_SOURCE_DIMENSION).contains(&height)
    {
        return Err("Windows returned an oversized thumbnail".into());
    }
    if width <= WIDTH && height <= HEIGHT {
        return Ok((width, height));
    }
    let scale = (f64::from(WIDTH) / f64::from(width)).min(f64::from(HEIGHT) / f64::from(height));
    let target_width = ((f64::from(width) * scale).round() as i32).clamp(1, WIDTH);
    let target_height = ((f64::from(height) * scale).round() as i32).clamp(1, HEIGHT);
    Ok((target_width, target_height))
}

/// Encodes a Shell thumbnail bitmap into bounded top-down 32bpp BGRA bytes,
/// aspect-fitting oversized bitmaps into `WIDTH x HEIGHT`.
unsafe fn encode_bitmap(bitmap: HBITMAP) -> Result<Vec<u8>, String> {
    // SAFETY: caller provides a live HBITMAP; owned DCs, DIBs, and selections are released via RAII guards.
    unsafe {
        let mut object = BITMAP::default();
        if GetObjectW(
            bitmap,
            std::mem::size_of::<BITMAP>() as i32,
            Some((&mut object as *mut BITMAP).cast()),
        ) == 0
        {
            return Err("Could not read thumbnail dimensions".into());
        }
        let (src_width, src_height) = (object.bmWidth, object.bmHeight);
        let (dst_width, dst_height) = fit_dimensions(src_width, src_height)?;
        let pixel_bytes = (dst_width * dst_height * 4) as usize;
        let mut bytes = vec![0u8; pixel_bytes + 8];
        bytes[..4].copy_from_slice(&dst_width.to_le_bytes());
        bytes[4..8].copy_from_slice(&dst_height.to_le_bytes());

        if (src_width, src_height) == (dst_width, dst_height) {
            let dc = OwnedDc(CreateCompatibleDC(None));
            if dc.0.0.is_null() {
                return Err("Could not create thumbnail DC".into());
            }
            let info = bitmap_info(dst_width, dst_height);
            let rows = GetDIBits(
                dc.0,
                bitmap,
                0,
                dst_height as u32,
                Some(bytes[8..].as_mut_ptr().cast()),
                &info as *const _ as *mut _,
                DIB_RGB_COLORS,
            );
            if rows != dst_height {
                return Err("Could not read thumbnail pixels".into());
            }
            return Ok(bytes);
        }

        let src_dc = OwnedDc(CreateCompatibleDC(None));
        if src_dc.0.0.is_null() {
            return Err("Could not create thumbnail DC".into());
        }
        let old_src = SelectObject(src_dc.0, bitmap);
        if old_src.0.is_null() || old_src.0 == (-1isize as *mut _) {
            return Err("Could not select source thumbnail".into());
        }
        let _src_guard = SelectedObject(src_dc.0, old_src);

        let mut pixels = std::ptr::null_mut();
        let dst_bitmap = OwnedBitmap(
            CreateDIBSection(
                None,
                &bitmap_info(dst_width, dst_height),
                DIB_RGB_COLORS,
                &mut pixels,
                None,
                0,
            )
            .map_err(|e| e.to_string())?,
        );
        if pixels.is_null() {
            return Err("Thumbnail allocation failed".into());
        }
        let dst_dc = OwnedDc(CreateCompatibleDC(None));
        if dst_dc.0.0.is_null() {
            return Err("Could not create thumbnail DC".into());
        }
        let old_dst = SelectObject(dst_dc.0, dst_bitmap.0);
        if old_dst.0.is_null() || old_dst.0 == (-1isize as *mut _) {
            return Err("Could not select target thumbnail".into());
        }
        let _dst_guard = SelectedObject(dst_dc.0, old_dst);

        if SetStretchBltMode(dst_dc.0, HALFTONE) == 0 {
            return Err("Could not set thumbnail scaling mode".into());
        }
        let _ = SetBrushOrgEx(dst_dc.0, 0, 0, None);
        if !StretchBlt(
            dst_dc.0, 0, 0, dst_width, dst_height, src_dc.0, 0, 0, src_width, src_height, SRCCOPY,
        )
        .as_bool()
        {
            return Err("Could not scale thumbnail".into());
        }
        let _ = GdiFlush();
        std::ptr::copy_nonoverlapping(pixels.cast::<u8>(), bytes[8..].as_mut_ptr(), pixel_bytes);
        Ok(bytes)
    }
}

fn dimensions(bytes: &[u8]) -> Result<(i32, i32), String> {
    let header = bytes.get(..8).ok_or("Thumbnail output is incomplete")?;
    let width = i32::from_le_bytes(
        header[..4]
            .try_into()
            .map_err(|_| "Invalid thumbnail width")?,
    );
    let height = i32::from_le_bytes(
        header[4..]
            .try_into()
            .map_err(|_| "Invalid thumbnail height")?,
    );
    if !(1..=WIDTH).contains(&width)
        || !(1..=HEIGHT).contains(&height)
        || bytes.len() != (width * height * 4 + 8) as usize
    {
        return Err("Invalid thumbnail size".into());
    }
    Ok((width, height))
}
fn bitmap_info(width: i32, height: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    }
}
pub fn bitmap(bytes: &[u8]) -> Result<HBITMAP, String> {
    let (width, height) = dimensions(bytes)?;
    let mut pixels = std::ptr::null_mut();
    // SAFETY: validated dimensions fix allocation/copy size; caller owns returned HBITMAP.
    unsafe {
        let bitmap = CreateDIBSection(
            None,
            &bitmap_info(width, height),
            DIB_RGB_COLORS,
            &mut pixels,
            None,
            0,
        )
        .map_err(|e| e.to_string())?;
        if pixels.is_null() {
            let _ = DeleteObject(bitmap);
            return Err("Thumbnail allocation failed".into());
        }
        std::ptr::copy_nonoverlapping(bytes[8..].as_ptr(), pixels.cast(), bytes.len() - 8);
        Ok(bitmap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_worker_is_killed_and_reaped() {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        use windows::Win32::{Foundation::*, System::Threading::*};
        let command = std::env::var_os("ComSpec").expect("Windows command processor");
        let child = SilentChild::spawn_with_piped_stdin(
            Path::new(&command),
            &[
                OsStr::new("/D"),
                OsStr::new("/Q"),
                OsStr::new("/C"),
                OsStr::new("set /p wallpaperlive_test_wait="),
            ],
            0x08004000,
        )
        .unwrap();
        let mut duplicate = HANDLE::default();
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                child.raw_process_handle(),
                GetCurrentProcess(),
                &mut duplicate,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
            .unwrap();
        }
        let process = unsafe { OwnedHandle::from_raw_handle(duplicate.0) };
        let cancelled = std::sync::atomic::AtomicBool::new(true);
        let start = Instant::now();
        assert!(
            capture_output(child, &cancelled)
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        unsafe {
            assert_eq!(
                WaitForSingleObject(HANDLE(process.as_raw_handle()), 0),
                WAIT_OBJECT_0
            );
        }
    }

    #[test]
    fn windows_command_line_quotes_spaces_quotes_and_trailing_backslashes() {
        let cmd = build_windows_command_line(
            Path::new(r#"C:\Program Files\WallpaperLive\WallpaperLive.exe"#),
            &[
                OsStr::new("--thumbnail"),
                OsStr::new(r#"C:\my wallpapers\clip 1\"#),
                OsStr::new(r#"a"b"#),
                OsStr::new(""),
            ],
        );
        assert_eq!(cmd.last().copied(), Some(0));
        let text = String::from_utf16_lossy(&cmd[..cmd.len() - 1]);
        assert_eq!(
            text,
            r#""C:\Program Files\WallpaperLive\WallpaperLive.exe" --thumbnail "C:\my wallpapers\clip 1\\" "a\"b" """#
        );
    }

    #[test]
    fn silent_child_sets_force_off_feedback_and_captures_stdout() {
        assert!(SILENT_STARTUP_FLAGS.contains(STARTF_FORCEOFFFEEDBACK));
        let command = std::env::var_os("ComSpec").expect("Windows command processor");
        let mut child = SilentChild::spawn(
            Path::new(&command),
            &[
                OsStr::new("/D"),
                OsStr::new("/Q"),
                OsStr::new("/C"),
                OsStr::new("echo wallpaperlive-silent-ok"),
            ],
            0x0800_4000,
        )
        .expect("spawn silent child");
        let mut stdout = child.take_stdout().expect("stdout pipe");
        let mut text = String::new();
        stdout.read_to_string(&mut text).expect("read stdout");
        assert!(
            text.contains("wallpaperlive-silent-ok"),
            "unexpected stdout: {text:?}"
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        let success = loop {
            if let Some(ok) = child.try_wait().expect("poll exit") {
                break ok;
            }
            assert!(Instant::now() < deadline, "silent child timed out");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(success);
    }

    #[test]
    fn thumbnail_output_is_bounded_and_validated_before_allocation() {
        for bytes in [
            vec![],
            vec![0; 8],
            [481i32.to_le_bytes(), 270i32.to_le_bytes()].concat(),
            [(-1i32).to_le_bytes(), 1i32.to_le_bytes()].concat(),
        ] {
            assert!(bitmap(&bytes).is_err());
        }
        let mut bytes = [480i32.to_le_bytes(), 270i32.to_le_bytes()].concat();
        bytes.resize(LIMIT as usize, 0);
        assert_eq!(dimensions(&bytes).unwrap(), (480, 270));
        let image = bitmap(&bytes).unwrap();
        unsafe {
            assert!(DeleteObject(image).as_bool());
        }
        bytes.push(0);
        assert!(bitmap(&bytes).is_err());
    }

    #[test]
    fn oversized_shell_thumbnails_are_downscaled_within_bounds() {
        assert!(fit_dimensions(0, 100).is_err());
        assert!(fit_dimensions(100, -1).is_err());
        assert!(fit_dimensions(MAX_SOURCE_DIMENSION + 1, 270).is_err());
        assert_eq!(fit_dimensions(256, 256).unwrap(), (256, 256));
        assert_eq!(fit_dimensions(480, 270).unwrap(), (480, 270));
        assert_eq!(fit_dimensions(8192, 1).unwrap(), (480, 1));

        for (src_w, src_h, dst_w, dst_h) in [
            (768, 432, 480, 270),
            (640, 480, 360, 270),
            (150, 422, 96, 270),
        ] {
            assert_eq!(fit_dimensions(src_w, src_h).unwrap(), (dst_w, dst_h));
            let mut pixels = std::ptr::null_mut();
            let src = unsafe {
                OwnedBitmap(
                    CreateDIBSection(
                        None,
                        &bitmap_info(src_w, src_h),
                        DIB_RGB_COLORS,
                        &mut pixels,
                        None,
                        0,
                    )
                    .unwrap(),
                )
            };
            assert!(!pixels.is_null());
            let count = (src_w * src_h) as usize;
            let slice = unsafe { std::slice::from_raw_parts_mut(pixels.cast::<u32>(), count) };
            slice.fill(0x0033_6699);

            let encoded = unsafe { encode_bitmap(src.0) }.unwrap();
            assert_eq!(dimensions(&encoded).unwrap(), (dst_w, dst_h));
            assert!(encoded.len() <= LIMIT as usize);
            let center = (((dst_h / 2) * dst_w + (dst_w / 2)) * 4 + 8) as usize;
            assert_eq!(&encoded[center..center + 3], &[0x99, 0x66, 0x33]);

            let decoded = OwnedBitmap(bitmap(&encoded).unwrap());
            let mut info = BITMAP::default();
            let read = unsafe {
                GetObjectW(
                    decoded.0,
                    std::mem::size_of::<BITMAP>() as i32,
                    Some((&mut info as *mut BITMAP).cast()),
                )
            };
            assert_ne!(read, 0);
            assert_eq!((info.bmWidth, info.bmHeight), (dst_w, dst_h));
        }
    }
}
