use crate::core::validate_path_syntax;
use std::{
    ffi::c_void,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    ptr,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows::{
    Data::Json::{IJsonObjectStatics, JsonObject, JsonValueType},
    Win32::{
        Graphics::Imaging::*,
        Networking::WinHttp::*,
        Storage::FileSystem::GetDiskFreeSpaceExW,
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        },
        UI::Shell::SHCreateMemStream,
    },
    core::{HSTRING, Interface, PCWSTR, w},
};

const PIXABAY_KEY_1: &str = env!("PIXABAY_KEY_1");
const PIXABAY_KEY_2: &str = env!("PIXABAY_KEY_2");

const TOTAL_TIMEOUT: Duration = Duration::from_secs(12);

struct Handle(*mut c_void);
impl Handle {
    fn new(raw: *mut c_void) -> Result<Self, String> {
        if raw.is_null() {
            Err(windows::core::Error::from_win32().to_string())
        } else {
            Ok(Self(raw))
        }
    }
    fn timeout(&self, cancelled: &AtomicBool, deadline: Instant) -> Result<(), String> {
        let timeout = checkpoint(cancelled, deadline)?;
        // SAFETY: this thread exclusively owns the open synchronous handle.
        unsafe { WinHttpSetTimeouts(self.0, timeout, timeout, timeout, timeout) }
            .map_err(|e| e.to_string())
    }
    fn option(&self, option: u32, value: u32) -> Result<(), String> {
        // SAFETY: native DWORD options copy the four-byte value during this call.
        unsafe { WinHttpSetOption(Some(self.0), option, Some(&value.to_ne_bytes())) }
            .map_err(|e| e.to_string())
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: all synchronous calls have returned; the handle is closed exactly once.
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

fn checkpoint(cancelled: &AtomicBool, deadline: Instant) -> Result<i32, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Operation cancelled".into());
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("Request timed out".into());
    }
    Ok(remaining.min(Duration::from_secs(5)).as_millis().max(1) as i32)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OnlineVideo {
    pub id: u64,
    pub duration: u32,
    pub user: String,
    pub tags: String,
    pub thumbnail_url: String,
    pub video_url: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchPage {
    pub total_hits: u32,
    pub current_page: u32,
    pub items: Vec<OnlineVideo>,
}

pub fn url_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 3);
    for b in input.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            b' ' => encoded.push('+'),
            _ => {
                encoded.push_str(&format!("%{b:02X}"));
            }
        }
    }
    encoded
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}

pub fn parse_pixabay_json(text: &str) -> Result<SearchPage, String> {
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|e| e.to_string())?;
    let _apartment = Apartment;
    let factory =
        windows::core::factory::<JsonObject, IJsonObjectStatics>().map_err(|e| e.to_string())?;
    let input = HSTRING::from(text);
    let mut raw = ptr::null_mut();
    unsafe {
        (factory.vtable().Parse)(factory.as_raw(), std::mem::transmute_copy(&input), &mut raw)
    }
    .ok()
    .map_err(|e| format!("Invalid JSON: {e}"))?;
    if raw.is_null() {
        return Err("JSON parser returned no root object".into());
    }
    let root = unsafe { JsonObject::from_raw(raw) };
    let total_hits = root
        .GetNamedNumber(&HSTRING::from("totalHits"))
        .unwrap_or(0.0) as u32;

    let hits = root
        .GetNamedArray(&HSTRING::from("hits"))
        .map_err(|_| "Response missing hits array")?;

    let size = hits.Size().unwrap_or(0);
    let mut items = Vec::with_capacity(size as usize);

    for i in 0..size {
        let val = hits.GetAt(i).map_err(|e| e.to_string())?;
        if val.ValueType().unwrap_or(JsonValueType::Null) != JsonValueType::Object {
            continue;
        }
        let obj = val.GetObject().map_err(|e| e.to_string())?;
        let id = obj.GetNamedNumber(&HSTRING::from("id")).unwrap_or(0.0) as u64;
        let duration = obj
            .GetNamedNumber(&HSTRING::from("duration"))
            .unwrap_or(0.0) as u32;
        let user = obj
            .GetNamedString(&HSTRING::from("user"))
            .map(|h| h.to_string_lossy())
            .unwrap_or_default();
        let tags = obj
            .GetNamedString(&HSTRING::from("tags"))
            .map(|h| h.to_string_lossy())
            .unwrap_or_default();

        let Ok(videos) = obj.GetNamedObject(&HSTRING::from("videos")) else {
            continue;
        };

        // Extract thumbnail URL (prefer tiny, then small, then medium)
        let mut thumbnail_url = String::new();
        for key in ["tiny", "small", "medium", "large"] {
            if let Ok(v_obj) = videos.GetNamedObject(&HSTRING::from(key))
                && let Ok(thumb) = v_obj.GetNamedString(&HSTRING::from("thumbnail"))
            {
                let s = thumb.to_string_lossy();
                if !s.is_empty() {
                    thumbnail_url = s;
                    break;
                }
            }
        }

        // Extract video file URL and dimensions (prefer large, then medium)
        let mut video_url = String::new();
        let mut width = 0u32;
        let mut height = 0u32;
        let mut size_bytes = 0u64;

        for key in ["large", "medium", "small"] {
            if let Ok(v_obj) = videos.GetNamedObject(&HSTRING::from(key))
                && let Ok(v_url) = v_obj.GetNamedString(&HSTRING::from("url"))
            {
                let s = v_url.to_string_lossy();
                if !s.is_empty() {
                    video_url = s;
                    width = v_obj.GetNamedNumber(&HSTRING::from("width")).unwrap_or(0.0) as u32;
                    height = v_obj
                        .GetNamedNumber(&HSTRING::from("height"))
                        .unwrap_or(0.0) as u32;
                    size_bytes = v_obj.GetNamedNumber(&HSTRING::from("size")).unwrap_or(0.0) as u64;
                    break;
                }
            }
        }

        if !video_url.is_empty() {
            items.push(OnlineVideo {
                id,
                duration,
                user,
                tags,
                thumbnail_url,
                video_url,
                width,
                height,
                size_bytes,
            });
        }
    }

    Ok(SearchPage {
        total_hits,
        current_page: 1,
        items,
    })
}

fn fetch_http_bytes(
    url: &str,
    cancelled: &AtomicBool,
    deadline: Instant,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    checkpoint(cancelled, deadline)?;
    let parsed = url.strip_prefix("https://").ok_or("URL must be HTTPS")?;
    let (host, path) = parsed.split_once('/').ok_or("Invalid URL path")?;
    let path = format!("/{path}");

    let agent: Vec<u16> = format!("WallpaperLive/{}", env!("CARGO_PKG_VERSION"))
        .encode_utf16()
        .chain(Some(0))
        .collect();

    let session = Handle::new(unsafe {
        WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        )
    })?;
    session.timeout(cancelled, deadline)?;

    let host_wide: Vec<u16> = host.encode_utf16().chain(Some(0)).collect();
    let connection =
        Handle::new(unsafe { WinHttpConnect(session.0, PCWSTR(host_wide.as_ptr()), 443, 0) })?;
    checkpoint(cancelled, deadline)?;

    let path_wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let request = Handle::new(unsafe {
        WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            PCWSTR(path_wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            ptr::null(),
            WINHTTP_FLAG_SECURE,
        )
    })?;
    request.option(
        WINHTTP_OPTION_DISABLE_FEATURE,
        WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION,
    )?;
    request.timeout(cancelled, deadline)?;

    let headers: Vec<u16> =
        "Accept: */*\r\nUser-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64)\r\n"
            .encode_utf16()
            .collect();
    unsafe { WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0) }
        .map_err(|e| e.to_string())?;

    request.timeout(cancelled, deadline)?;
    unsafe { WinHttpReceiveResponse(request.0, ptr::null_mut()) }.map_err(|e| e.to_string())?;
    checkpoint(cancelled, deadline)?;

    let mut status = 0u32;
    let mut length = std::mem::size_of_val(&status) as u32;
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status as *mut u32).cast()),
            &mut length,
            ptr::null_mut(),
        )
    }
    .map_err(|e| e.to_string())?;

    if status != 200 {
        return Err(format!("Server returned HTTP {status}"));
    }

    let mut body = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        request.timeout(cancelled, deadline)?;
        let mut read = 0u32;
        unsafe {
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                &mut read,
            )
        }
        .map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read as usize]);
        if body.len() > max_bytes {
            return Err("Response exceeded size limit".into());
        }
    }
    Ok(body)
}

pub fn search_pixabay(
    query: &str,
    page: u32,
    cancelled: &AtomicBool,
) -> Result<SearchPage, String> {
    let keys = [PIXABAY_KEY_1, PIXABAY_KEY_2];
    let query_clean = if query.trim().is_empty() {
        "wallpaper"
    } else {
        query.trim()
    };
    let q = url_encode(query_clean);

    let mut last_err = String::new();
    for key in keys {
        if key.is_empty() {
            continue;
        }
        let url = format!(
            "https://pixabay.com/api/videos/?key={key}&q={q}&video_type=film&per_page=9&page={page}&safesearch=true"
        );
        let deadline = Instant::now() + TOTAL_TIMEOUT;
        match fetch_http_bytes(&url, cancelled, deadline, 1024 * 1024) {
            Ok(bytes) => {
                let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
                let mut page_result = parse_pixabay_json(&text)?;
                page_result.current_page = page;
                return Ok(page_result);
            }
            Err(e) => {
                last_err = e;
            }
        }
    }

    if last_err.is_empty() {
        Err("No Pixabay API key configured in build".into())
    } else {
        Err(format!("Pixabay search failed: {last_err}"))
    }
}

pub fn decode_jpeg_to_thumbnail_bytes(jpeg: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        let result = (|| {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| e.to_string())?;

            let stream = SHCreateMemStream(Some(jpeg)).ok_or("Could not create stream")?;
            let decoder = factory
                .CreateDecoderFromStream(&stream, ptr::null(), WICDecodeMetadataCacheOnDemand)
                .map_err(|e| e.to_string())?;

            let frame = decoder.GetFrame(0).map_err(|e| e.to_string())?;
            let mut width = 0u32;
            let mut height = 0u32;
            frame
                .GetSize(&mut width, &mut height)
                .map_err(|e| e.to_string())?;

            if width == 0 || height == 0 || width > 4096 || height > 4096 {
                return Err("Invalid image dimensions".into());
            }

            let converter = factory.CreateFormatConverter().map_err(|e| e.to_string())?;
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(|e| e.to_string())?;

            let stride = width * 4;
            let pixel_bytes = (stride * height) as usize;
            let mut bytes = vec![0u8; pixel_bytes + 8];
            bytes[..4].copy_from_slice(&(width as i32).to_le_bytes());
            bytes[4..8].copy_from_slice(&(height as i32).to_le_bytes());

            converter
                .CopyPixels(ptr::null(), stride, &mut bytes[8..])
                .map_err(|e| e.to_string())?;

            Ok(bytes)
        })();
        CoUninitialize();
        result
    }
}

pub fn fetch_and_decode_thumbnail(url: &str, cancelled: &AtomicBool) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + Duration::from_secs(8);
    let bytes = fetch_http_bytes(url, cancelled, deadline, 1024 * 1024)?;
    decode_jpeg_to_thumbnail_bytes(&bytes)
}

pub fn download_video_stream(
    url: &str,
    destination: &Path,
    progress_tx: &std::sync::mpsc::Sender<(u64, u64)>,
    cancelled: &AtomicBool,
) -> Result<PathBuf, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Download cancelled".into());
    }
    validate_path_syntax(destination)?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        // Check disk free space
        let wide: Vec<u16> = parent
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut free = 0u64;
        unsafe {
            GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None)
                .map_err(|e| e.to_string())?;
        }
        if free < 200 * 1024 * 1024 {
            return Err(
                "Disk space low; at least 200 MB free required to download wallpaper".into(),
            );
        }
    }

    if destination.exists() {
        return Ok(destination.to_path_buf());
    }

    let parsed = url.strip_prefix("https://").ok_or("URL must be HTTPS")?;
    let (host, path) = parsed.split_once('/').ok_or("Invalid URL path")?;
    let path = format!("/{path}");

    let deadline = Instant::now() + Duration::from_secs(180);
    let agent: Vec<u16> = format!("WallpaperLive/{}", env!("CARGO_PKG_VERSION"))
        .encode_utf16()
        .chain(Some(0))
        .collect();

    let session = Handle::new(unsafe {
        WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        )
    })?;
    session.timeout(cancelled, deadline)?;

    let host_wide: Vec<u16> = host.encode_utf16().chain(Some(0)).collect();
    let connection =
        Handle::new(unsafe { WinHttpConnect(session.0, PCWSTR(host_wide.as_ptr()), 443, 0) })?;
    checkpoint(cancelled, deadline)?;

    let path_wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let request = Handle::new(unsafe {
        WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            PCWSTR(path_wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            ptr::null(),
            WINHTTP_FLAG_SECURE,
        )
    })?;
    request.option(
        WINHTTP_OPTION_DISABLE_FEATURE,
        WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION,
    )?;
    request.timeout(cancelled, deadline)?;

    let headers: Vec<u16> =
        "Accept: */*\r\nUser-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64)\r\n"
            .encode_utf16()
            .collect();
    unsafe { WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0) }
        .map_err(|e| e.to_string())?;

    request.timeout(cancelled, deadline)?;
    unsafe { WinHttpReceiveResponse(request.0, ptr::null_mut()) }.map_err(|e| e.to_string())?;

    let mut status = 0u32;
    let mut length = std::mem::size_of_val(&status) as u32;
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status as *mut u32).cast()),
            &mut length,
            ptr::null_mut(),
        )
    }
    .map_err(|e| e.to_string())?;

    if status != 200 {
        return Err(format!("Download failed with HTTP {status}"));
    }

    // Try reading Content-Length
    let mut content_length = 0u64;
    let mut cl_buf = [0u16; 32];
    let mut cl_len = (cl_buf.len() * 2) as u32;
    if unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_CONTENT_LENGTH,
            PCWSTR::null(),
            Some(cl_buf.as_mut_ptr().cast()),
            &mut cl_len,
            ptr::null_mut(),
        )
    }
    .is_ok()
    {
        let n = cl_len as usize / 2;
        let s = String::from_utf16_lossy(&cl_buf[..n]);
        content_length = s.trim().parse::<u64>().unwrap_or(0);
    }

    let part_path = destination.with_extension("mp4.part");
    if part_path.exists() {
        let _ = fs::remove_file(&part_path);
    }

    let mut file = File::create(&part_path).map_err(|e| e.to_string())?;
    let mut downloaded = 0u64;
    let mut chunk = [0u8; 16384];

    loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = fs::remove_file(&part_path);
            return Err("Download cancelled".into());
        }
        let mut read = 0u32;
        unsafe {
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                &mut read,
            )
        }
        .map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        file.write_all(&chunk[..read as usize])
            .map_err(|e| e.to_string())?;
        downloaded += read as u64;
        let _ = progress_tx.send((downloaded, content_length));
    }

    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    if downloaded == 0 {
        let _ = fs::remove_file(&part_path);
        return Err("Downloaded file was empty".into());
    }

    fs::rename(&part_path, destination).map_err(|e| format!("Could not finalize download: {e}"))?;
    Ok(destination.to_path_buf())
}

pub fn export_library(source_dir: &Path, target_dir: &Path) -> Result<(usize, u64), String> {
    validate_path_syntax(source_dir)?;
    validate_path_syntax(target_dir)?;
    if !source_dir.is_dir() {
        return Ok((0, 0));
    }
    fs::create_dir_all(target_dir)
        .map_err(|e| format!("Could not create backup directory: {e}"))?;

    let mut count = 0;
    let mut total_bytes = 0u64;

    for entry in fs::read_dir(source_dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() {
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            if matches!(ext.as_str(), "mp4" | "gif" | "mkv" | "webm" | "mov" | "avi")
                && let Some(file_name) = path.file_name()
            {
                let dest = target_dir.join(file_name);
                if let Ok(meta) = entry.metadata() {
                    let len = meta.len();
                    if fs::copy(&path, &dest).is_ok() {
                        count += 1;
                        total_bytes += len;
                    }
                }
            }
        }
    }

    Ok((count, total_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_encode_handles_spaces_and_special_chars() {
        assert_eq!(url_encode("cyberpunk city"), "cyberpunk+city");
        assert_eq!(url_encode("anime & manga"), "anime+%26+manga");
        assert_eq!(url_encode("nature"), "nature");
    }

    #[test]
    fn parse_pixabay_json_handles_empty_and_valid_payload() {
        let sample = r#"{
            "totalHits": 150,
            "hits": [
                {
                    "id": 12345,
                    "duration": 15,
                    "user": "Alice",
                    "tags": "cyberpunk, neon",
                    "videos": {
                        "large": {
                            "url": "https://cdn.pixabay.com/video/sample_large.mp4",
                            "width": 1920,
                            "height": 1080,
                            "size": 15000000,
                            "thumbnail": "https://cdn.pixabay.com/video/sample_large.jpg"
                        },
                        "tiny": {
                            "url": "https://cdn.pixabay.com/video/sample_tiny.mp4",
                            "width": 640,
                            "height": 360,
                            "size": 2000000,
                            "thumbnail": "https://cdn.pixabay.com/video/sample_tiny.jpg"
                        }
                    }
                }
            ]
        }"#;

        let parsed = parse_pixabay_json(sample).unwrap();
        assert_eq!(parsed.total_hits, 150);
        assert_eq!(parsed.items.len(), 1);
        let item = &parsed.items[0];
        assert_eq!(item.id, 12345);
        assert_eq!(item.duration, 15);
        assert_eq!(item.user, "Alice");
        assert_eq!(item.width, 1920);
        assert_eq!(item.height, 1080);
        assert_eq!(item.size_bytes, 15000000);
        assert_eq!(
            item.video_url,
            "https://cdn.pixabay.com/video/sample_large.mp4"
        );
        assert_eq!(
            item.thumbnail_url,
            "https://cdn.pixabay.com/video/sample_tiny.jpg"
        );
    }

    #[test]
    fn export_library_copies_media_files() {
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scratch")
            .join(format!("test-export-src-{}", std::process::id()));
        let dst = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scratch")
            .join(format!("test-export-dst-{}", std::process::id()));

        fs::create_dir_all(&src).unwrap();
        let f1 = src.join("video1.mp4");
        let f2 = src.join("video2.gif");
        let f3 = src.join("ignore.txt");

        fs::write(&f1, b"mp4data").unwrap();
        fs::write(&f2, b"gifdata").unwrap();
        fs::write(&f3, b"txtdata").unwrap();

        let (count, bytes) = export_library(&src, &dst).unwrap();
        assert_eq!(count, 2);
        assert_eq!(bytes, 14);

        assert!(dst.join("video1.mp4").is_file());
        assert!(dst.join("video2.gif").is_file());
        assert!(!dst.join("ignore.txt").exists());

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }
}
