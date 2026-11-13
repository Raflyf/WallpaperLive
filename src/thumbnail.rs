use crate::core::validate_media;
use std::{
    io::Read,
    os::windows::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use windows::{
    Win32::{Foundation::SIZE, Graphics::Gdi::*, System::Com::*, UI::Shell::*},
    core::PCWSTR,
};

const WIDTH: i32 = 480;
const HEIGHT: i32 = 270;
const LIMIT: u64 = (WIDTH * HEIGHT * 4 + 8) as u64;

struct Worker(std::process::Child);
impl Drop for Worker {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

pub fn capture(path: &Path, cancelled: &std::sync::atomic::AtomicBool) -> Result<Vec<u8>, String> {
    if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("Thumbnail cancelled".into());
    }
    // Shell codecs stay in one short-lived child, not in the wallpaper process.
    let child = Worker(
        Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
            .arg("--thumbnail")
            .arg(path)
            .creation_flags(0x08004000)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Could not start thumbnail worker: {e}"))?,
    );
    capture_output(child, cancelled)
}
fn capture_output(
    mut child: Worker,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Vec<u8>, String> {
    let stdout = child
        .0
        .stdout
        .take()
        .ok_or("Thumbnail worker has no output")?;
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout
            .take(LIMIT + 1)
            .read_to_end(&mut output)
            .map(|_| output)
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if cancelled.load(std::sync::atomic::Ordering::Relaxed)
            || start.elapsed() >= Duration::from_secs(10)
        {
            child
                .0
                .kill()
                .map_err(|e| format!("Could not stop thumbnail worker: {e}"))?;
            child
                .0
                .wait()
                .map_err(|e| format!("Could not reap thumbnail worker: {e}"))?;
            let _ = reader.join();
            return Err(if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                "Thumbnail cancelled"
            } else {
                "Thumbnail worker timed out"
            }
            .into());
        }
        std::thread::park_timeout(Duration::from_millis(50));
    };
    let output = reader
        .join()
        .map_err(|_| "Thumbnail output reader failed")?
        .map_err(|e| e.to_string())?;
    if !status.success() {
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
            let bitmap = factory
                .GetImage(
                    SIZE {
                        cx: WIDTH,
                        cy: HEIGHT,
                    },
                    SIIGBF_THUMBNAILONLY | SIIGBF_RESIZETOFIT,
                )
                .map_err(|e| e.to_string())?;
            let result = (|| {
                let mut object = BITMAP::default();
                if GetObjectW(
                    bitmap,
                    std::mem::size_of::<BITMAP>() as i32,
                    Some((&mut object as *mut BITMAP).cast()),
                ) == 0
                {
                    return Err("Could not read thumbnail dimensions".into());
                }
                let (width, height) = (object.bmWidth, object.bmHeight);
                if !(1..=WIDTH).contains(&width) || !(1..=HEIGHT).contains(&height) {
                    return Err("Windows returned an oversized thumbnail".into());
                }
                let info = bitmap_info(width, height);
                let dc = CreateCompatibleDC(None);
                if dc.0.is_null() {
                    return Err("Could not create thumbnail DC".into());
                }
                let mut bytes = vec![0; (width * height * 4 + 8) as usize];
                bytes[..4].copy_from_slice(&width.to_le_bytes());
                bytes[4..8].copy_from_slice(&height.to_le_bytes());
                let rows = GetDIBits(
                    dc,
                    bitmap,
                    0,
                    height as u32,
                    Some(bytes[8..].as_mut_ptr().cast()),
                    &info as *const _ as *mut _,
                    DIB_RGB_COLORS,
                );
                let _ = DeleteDC(dc);
                if rows != height {
                    return Err("Could not read thumbnail pixels".into());
                }
                Ok(bytes)
            })();
            let _ = DeleteObject(bitmap);
            result
        })();
        CoUninitialize();
        result
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
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows::Win32::{Foundation::*, System::Threading::*};
        let command = std::env::var_os("ComSpec").expect("Windows command processor");
        let child = Command::new(command)
            .args(["/D", "/Q", "/C", "set /p wallpaperlive_test_wait="])
            .creation_flags(0x08004000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut duplicate = HANDLE::default();
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                HANDLE(child.as_raw_handle()),
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
            capture_output(Worker(child), &cancelled)
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
}
