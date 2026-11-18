use std::{
    ffi::c_void,
    fs::{self, File},
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    ptr,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows::{
    Data::Json::{IJsonObjectStatics, JsonObject},
    Win32::{
        Networking::WinHttp::*,
        Security::Cryptography::{
            BCRYPT_ALG_HANDLE, BCRYPT_HASH_HANDLE, BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS,
            BCRYPT_SHA256_ALGORITHM, BCryptCloseAlgorithmProvider, BCryptCreateHash,
            BCryptDestroyHash, BCryptFinishHash, BCryptHashData, BCryptOpenAlgorithmProvider,
        },
        System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    },
    core::{HSTRING, Interface, PCWSTR, w},
};

const MAX_BODY: usize = 128 * 1024;
const MAX_CHECKSUMS_BODY: usize = 32 * 1024;
const MAX_INSTALLER_BYTES: u64 = 256 * 1024 * 1024;
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const PHASE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug)]
pub enum DownloadEvent {
    Progress(Option<u8>),
    Verifying,
    Ready(PathBuf),
    Failed(String),
}

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

fn checkpoint_capped(
    cancelled: &AtomicBool,
    deadline: Instant,
    max_phase: Duration,
) -> Result<i32, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Update check cancelled".into());
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("Update check timed out".into());
    }
    Ok(remaining.min(max_phase).as_millis().max(1) as i32)
}

fn checkpoint(cancelled: &AtomicBool, deadline: Instant) -> Result<i32, String> {
    checkpoint_capped(cancelled, deadline, Duration::from_secs(3))
}

pub fn normalize_tag(tag: &str) -> Result<String, String> {
    let parsed = tag
        .strip_prefix('v')
        .and_then(version)
        .ok_or("Release tag must be canonical vX.Y.Z")?;
    Ok(format!("v{}.{}.{}", parsed.0, parsed.1, parsed.2))
}

pub fn release_page_url(tag: Option<&str>) -> String {
    match tag.and_then(|t| normalize_tag(t).ok()) {
        Some(tag) => format!("https://github.com/Raflyf/WallpaperLive/releases/tag/{tag}"),
        None => "https://github.com/Raflyf/WallpaperLive/releases".into(),
    }
}

pub fn installer_filename(tag: &str) -> Result<String, String> {
    let canonical = normalize_tag(tag)?;
    Ok(format!("wallpaperlivesetup-{canonical}.exe"))
}

pub fn allowed_download_host(host: &str) -> bool {
    matches!(
        host.to_ascii_lowercase().as_str(),
        "github.com" | "objects.githubusercontent.com" | "release-assets.githubusercontent.com"
    )
}

pub fn split_https_url(url: &str) -> Result<(&str, &str), String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or("Untrusted update download scheme")?;
    let (authority, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    if authority.is_empty()
        || authority.contains('@')
        || authority.contains(':')
        || authority.contains(['\\', ' '])
        || !allowed_download_host(authority)
    {
        return Err("Untrusted update download host".into());
    }
    Ok((authority, path))
}

pub fn parse_sha256sums(text: &str, expected_file: &str) -> Result<[u8; 32], String> {
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((hex_part, file_part)) = line
            .split_once("  ")
            .or_else(|| line.split_once(" *"))
            .or_else(|| line.split_once('\t'))
        else {
            continue;
        };
        if file_part.trim() != expected_file {
            continue;
        }
        let hex = hex_part.trim();
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid SHA256SUMS digest entry".into());
        }
        let mut digest = [0u8; 32];
        for (i, slot) in digest.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|_| "Invalid SHA256SUMS digest entry")?;
        }
        return Ok(digest);
    }
    Err(format!("Missing checksum entry for {expected_file}"))
}

fn version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.split('.');
    let mut number = || {
        let part = parts.next()?;
        if part.is_empty()
            || !part.bytes().all(|c| c.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return None;
        }
        part.parse::<u32>().ok()
    };
    let value = (number()?, number()?, number()?);
    if parts.next().is_some() {
        None
    } else {
        Some(value)
    }
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: balances this thread's successful RoInitialize, after JSON objects are dropped.
        unsafe {
            RoUninitialize();
        }
    }
}

fn parse_release(text: &str) -> Result<Option<String>, String> {
    // SAFETY: check runs on an owned worker thread; initialization is balanced on every path.
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|e| e.to_string())?;
    let _apartment = Apartment;
    // Avoid the process-global activation cache retaining a factory after this worker's WinRT apartment is released.
    let factory =
        windows::core::factory::<JsonObject, IJsonObjectStatics>().map_err(|e| e.to_string())?;
    let input = HSTRING::from(text);
    let mut raw = ptr::null_mut();
    // SAFETY: exact generated Parse ABI; factory and input outlive the call, and result has one owner.
    unsafe {
        (factory.vtable().Parse)(factory.as_raw(), std::mem::transmute_copy(&input), &mut raw)
    }
    .ok()
    .map_err(|e| format!("Invalid release JSON: {e}"))?;
    if raw.is_null() {
        return Err("Release parser returned no object".into());
    }
    let object = unsafe { JsonObject::from_raw(raw) };
    let draft = object
        .GetNamedBoolean(&HSTRING::from("draft"))
        .map_err(|_| "Release draft field is missing or invalid")?;
    let prerelease = object
        .GetNamedBoolean(&HSTRING::from("prerelease"))
        .map_err(|_| "Release prerelease field is missing or invalid")?;
    let tag = object
        .GetNamedString(&HSTRING::from("tag_name"))
        .map_err(|_| "Release tag is missing or invalid")?
        .to_string_lossy();
    if draft || prerelease {
        return Ok(None);
    }
    let remote = tag
        .strip_prefix('v')
        .and_then(version)
        .ok_or("Release tag must be canonical vX.Y.Z")?;
    let local = version(env!("CARGO_PKG_VERSION")).ok_or("Application version is not X.Y.Z")?;
    Ok((remote > local).then(|| format!("v{}.{}.{}", remote.0, remote.1, remote.2)))
}

fn fetch(cancelled: &AtomicBool, deadline: Instant) -> Result<Vec<u8>, String> {
    checkpoint(cancelled, deadline)?;
    let agent: Vec<u16> = format!("WallpaperLive/{}", env!("CARGO_PKG_VERSION"))
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: fixed HTTPS target, owned handles and live NUL-terminated UTF-16 strings.
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
    let connection =
        Handle::new(unsafe { WinHttpConnect(session.0, w!("api.github.com"), 443, 0) })?;
    checkpoint(cancelled, deadline)?;
    let request = Handle::new(unsafe {
        WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            w!("/repos/Raflyf/WallpaperLive/releases/latest"),
            PCWSTR::null(),
            PCWSTR::null(),
            ptr::null(),
            WINHTTP_FLAG_SECURE,
        )
    })?;
    request.option(
        WINHTTP_OPTION_DISABLE_FEATURE,
        WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION,
    )?;
    request.option(
        WINHTTP_OPTION_AUTOLOGON_POLICY,
        WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH,
    )?;
    request.timeout(cancelled, deadline)?;
    let headers: Vec<u16> = "Accept: application/vnd.github+json\r\n"
        .encode_utf16()
        .collect();
    unsafe { WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0) }
        .map_err(|e| e.to_string())?;
    request.timeout(cancelled, deadline)?;
    request.option(
        WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT,
        checkpoint(cancelled, deadline)? as u32,
    )?;
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
    if status == 404 {
        return Err("No published stable release was found on GitHub".into());
    }
    if status != 200 {
        return Err(format!("GitHub update check returned HTTP {status}"));
    }
    let mut body = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        request.timeout(cancelled, deadline)?;
        let mut read = 0u32;
        // SAFETY: native API writes at most the supplied buffer size; count lives through the call.
        unsafe {
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                &mut read,
            )
        }
        .map_err(|e| e.to_string())?;
        checkpoint(cancelled, deadline)?;
        if read == 0 {
            break;
        }
        if read as usize > chunk.len() || body.len() + read as usize > MAX_BODY {
            return Err("GitHub release response exceeds 128 KiB".into());
        }
        body.extend_from_slice(&chunk[..read as usize]);
    }
    Ok(body)
}

pub fn message(result: &Result<Option<String>, String>) -> String {
    let current = env!("CARGO_PKG_VERSION");
    match result {
        Ok(Some(tag)) => format!(
            "A new version of WallpaperLive is available ({tag}).\n\nYou are using v{current}. Open the GitHub release page?\n\nNo automatic download or installation."
        ),
        Ok(None) => format!("You are using the latest version of WallpaperLive (v{current})."),
        Err(error) => format!(
            "Could not check for updates.\n\n{error}\n\nCheck your Internet connection and try again."
        ),
    }
}

pub fn check(cancelled: &AtomicBool) -> Result<Option<String>, String> {
    let deadline = Instant::now() + TOTAL_TIMEOUT;
    // ponytail: native synchronous calls have 3s phase limits; cancellation waits for the current call.
    let body = fetch(cancelled, deadline)?;
    checkpoint(cancelled, deadline)?;
    let result =
        parse_release(std::str::from_utf8(&body).map_err(|_| "Release JSON is not UTF-8")?);
    checkpoint(cancelled, deadline)?;
    result
}

struct AlgHandle(BCRYPT_ALG_HANDLE);
impl Drop for AlgHandle {
    fn drop(&mut self) {
        // SAFETY: owned CNG algorithm provider closed once after associated hashes drop.
        unsafe {
            let _ = BCryptCloseAlgorithmProvider(self.0, 0);
        }
    }
}

struct HashHandle(BCRYPT_HASH_HANDLE);
impl Drop for HashHandle {
    fn drop(&mut self) {
        // SAFETY: owned CNG hash handle destroyed once.
        unsafe {
            let _ = BCryptDestroyHash(self.0);
        }
    }
}

pub fn sha256_file(path: &Path) -> Result<[u8; 32], String> {
    let mut alg = BCRYPT_ALG_HANDLE::default();
    // SAFETY: valid CNG constants; handles owned and closed via RAII guards.
    unsafe {
        BCryptOpenAlgorithmProvider(
            &mut alg,
            BCRYPT_SHA256_ALGORITHM,
            None,
            BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
        )
    }
    .ok()
    .map_err(|e| format!("SHA-256 initialization failed: {e}"))?;
    let alg = AlgHandle(alg);

    let mut hash = BCRYPT_HASH_HANDLE::default();
    // SAFETY: alg handle is valid; hash buffer managed internally by Windows CNG.
    unsafe { BCryptCreateHash(alg.0, &mut hash, None, None, 0) }
        .ok()
        .map_err(|e| format!("SHA-256 hash creation failed: {e}"))?;
    let hash = HashHandle(hash);

    let mut file = File::open(path).map_err(|e| format!("Could not read downloaded file: {e}"))?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("Could not hash downloaded file: {e}"))?;
        if n == 0 {
            break;
        }
        // SAFETY: slice length is valid and initialized.
        unsafe { BCryptHashData(hash.0, &buf[..n], 0) }
            .ok()
            .map_err(|e| format!("SHA-256 update failed: {e}"))?;
    }
    let mut digest = [0u8; 32];
    // SAFETY: 32-byte digest matches BCRYPT_SHA256_ALGORITHM output size.
    unsafe { BCryptFinishHash(hash.0, &mut digest, 0) }
        .ok()
        .map_err(|e| format!("SHA-256 verification failed: {e}"))?;
    Ok(digest)
}

fn open_verified_https_stream(
    initial_url: &str,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<(Handle, Handle, Handle, Option<u64>), String> {
    let agent: Vec<u16> = format!("WallpaperLive/{}", env!("CARGO_PKG_VERSION"))
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: fixed HTTPS agent string; WinHTTP handles are owned via Handle Drop guards.
    let session = Handle::new(unsafe {
        WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        )
    })?;
    let timeout = checkpoint_capped(cancelled, deadline, PHASE_TIMEOUT)?;
    // SAFETY: owns session handle.
    unsafe { WinHttpSetTimeouts(session.0, timeout, timeout, timeout, timeout) }
        .map_err(|e| e.to_string())?;

    let mut current_url = initial_url.to_owned();
    for _hop in 0..=4 {
        let (host, path) = split_https_url(&current_url)?;
        let host_w: Vec<u16> = host.encode_utf16().chain(Some(0)).collect();
        let path_w: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let connection =
            Handle::new(unsafe { WinHttpConnect(session.0, PCWSTR(host_w.as_ptr()), 443, 0) })?;
        let request = Handle::new(unsafe {
            WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                PCWSTR(path_w.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        })?;
        request.option(
            WINHTTP_OPTION_DISABLE_FEATURE,
            WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION,
        )?;
        request.option(
            WINHTTP_OPTION_AUTOLOGON_POLICY,
            WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH,
        )?;
        let req_timeout = checkpoint_capped(cancelled, deadline, PHASE_TIMEOUT)?;
        unsafe {
            WinHttpSetTimeouts(
                request.0,
                req_timeout,
                req_timeout,
                req_timeout,
                req_timeout,
            )
        }
        .map_err(|e| e.to_string())?;
        unsafe { WinHttpSendRequest(request.0, None, None, 0, 0, 0) }.map_err(|e| e.to_string())?;
        let recv_timeout = checkpoint_capped(cancelled, deadline, PHASE_TIMEOUT)?;
        request.option(WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT, recv_timeout as u32)?;
        unsafe { WinHttpReceiveResponse(request.0, ptr::null_mut()) }.map_err(|e| e.to_string())?;
        checkpoint_capped(cancelled, deadline, PHASE_TIMEOUT)?;

        let mut status = 0u32;
        let mut status_len = std::mem::size_of_val(&status) as u32;
        unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some((&mut status as *mut u32).cast()),
                &mut status_len,
                ptr::null_mut(),
            )
        }
        .map_err(|e| e.to_string())?;

        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let mut loc_len = 0u32;
            let _ = unsafe {
                WinHttpQueryHeaders(
                    request.0,
                    WINHTTP_QUERY_LOCATION,
                    PCWSTR::null(),
                    None,
                    &mut loc_len,
                    ptr::null_mut(),
                )
            };
            if loc_len == 0 || loc_len > 16384 {
                return Err("Invalid update redirect header".into());
            }
            let mut buf = vec![0u16; (loc_len as usize) / 2 + 1];
            unsafe {
                WinHttpQueryHeaders(
                    request.0,
                    WINHTTP_QUERY_LOCATION,
                    PCWSTR::null(),
                    Some(buf.as_mut_ptr().cast()),
                    &mut loc_len,
                    ptr::null_mut(),
                )
            }
            .map_err(|e| e.to_string())?;
            let nul = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            current_url = String::from_utf16_lossy(&buf[..nul]);
            continue;
        }
        if status == 404 {
            return Err("Update package was not found on GitHub".into());
        }
        if status != 200 {
            return Err(format!("Update download returned HTTP {status}"));
        }

        let mut len_buf = [0u16; 32];
        let mut len_bytes = (len_buf.len() * 2) as u32;
        let content_length = unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_CONTENT_LENGTH,
                PCWSTR::null(),
                Some(len_buf.as_mut_ptr().cast()),
                &mut len_bytes,
                ptr::null_mut(),
            )
        }
        .ok()
        .and_then(|_| {
            let chars = (len_bytes as usize) / 2;
            String::from_utf16_lossy(&len_buf[..chars.min(len_buf.len())])
                .trim_matches('\0')
                .trim()
                .parse::<u64>()
                .ok()
        });
        return Ok((session, connection, request, content_length));
    }
    Err("Too many update download redirects".into())
}

fn fetch_verified_bytes(
    url: &str,
    max_bytes: usize,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<Vec<u8>, String> {
    let (_session, _conn, request, _) = open_verified_https_stream(url, cancelled, deadline)?;
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        checkpoint_capped(cancelled, deadline, PHASE_TIMEOUT)?;
        let mut read = 0u32;
        // SAFETY: writes at most chunk.len() bytes into local stack buffer.
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
        if out.len() + read as usize > max_bytes {
            return Err("Update metadata exceeds expected size".into());
        }
        out.extend_from_slice(&chunk[..read as usize]);
    }
    Ok(out)
}

pub fn download_verified_installer(
    tag: &str,
    dir: &Path,
    cancelled: &AtomicBool,
    mut on_progress: impl FnMut(Option<u8>),
) -> Result<PathBuf, String> {
    let canonical = normalize_tag(tag)?;
    let exe_name = installer_filename(&canonical)?;
    let sums_url = format!(
        "https://github.com/Raflyf/WallpaperLive/releases/download/{canonical}/SHA256SUMS.txt"
    );
    let exe_url =
        format!("https://github.com/Raflyf/WallpaperLive/releases/download/{canonical}/{exe_name}");
    let deadline = Instant::now() + DOWNLOAD_TIMEOUT;

    on_progress(Some(0));
    let sums_bytes = fetch_verified_bytes(&sums_url, MAX_CHECKSUMS_BODY, cancelled, deadline)?;
    let sums_text = std::str::from_utf8(&sums_bytes).map_err(|_| "SHA256SUMS.txt is not UTF-8")?;
    let expected_sha = parse_sha256sums(sums_text, &exe_name)?;

    fs::create_dir_all(dir).map_err(|e| format!("Could not prepare updates folder: {e}"))?;
    let final_path = dir.join(&exe_name);
    let part_path = dir.join(format!("{exe_name}.part"));

    let (_session, _conn, request, total_bytes) =
        open_verified_https_stream(&exe_url, cancelled, deadline)?;
    if total_bytes.is_some_and(|n| n > MAX_INSTALLER_BYTES) {
        return Err("Installer exceeds maximum allowed size".into());
    }

    let write_res: Result<(), String> = (|| {
        let file =
            File::create(&part_path).map_err(|e| format!("Could not create download file: {e}"))?;
        let mut writer = BufWriter::new(file);
        let mut downloaded = 0u64;
        let mut last_pct: Option<u8> = None;
        let mut buf = [0u8; 64 * 1024];
        loop {
            checkpoint_capped(cancelled, deadline, PHASE_TIMEOUT)?;
            let mut read = 0u32;
            // SAFETY: writes at most buf.len() bytes into local buffer.
            unsafe {
                WinHttpReadData(
                    request.0,
                    buf.as_mut_ptr().cast(),
                    buf.len() as u32,
                    &mut read,
                )
            }
            .map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            downloaded = downloaded
                .checked_add(u64::from(read))
                .ok_or("Download size overflow")?;
            if downloaded > MAX_INSTALLER_BYTES {
                return Err("Installer download exceeded maximum allowed size".into());
            }
            writer
                .write_all(&buf[..read as usize])
                .map_err(|e| format!("Could not write download file: {e}"))?;
            if let Some(total) = total_bytes.filter(|&t| t > 0) {
                let pct = ((downloaded.saturating_mul(100)) / total).min(100) as u8;
                if last_pct != Some(pct) {
                    last_pct = Some(pct);
                    on_progress(Some(pct));
                }
            } else if last_pct.is_none() {
                last_pct = Some(0);
                on_progress(None);
            }
        }
        writer
            .flush()
            .map_err(|e| format!("Could not flush download file: {e}"))?;
        if downloaded == 0 {
            return Err("Downloaded installer file was empty".into());
        }
        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = fs::remove_file(&part_path);
        return Err(err);
    }

    let actual_sha = match sha256_file(&part_path) {
        Ok(digest) => digest,
        Err(err) => {
            let _ = fs::remove_file(&part_path);
            return Err(err);
        }
    };
    if actual_sha != expected_sha {
        let _ = fs::remove_file(&part_path);
        return Err("Installer SHA-256 verification failed".into());
    }

    if final_path.exists() {
        let _ = fs::remove_file(&final_path);
    }
    if let Err(err) = fs::rename(&part_path, &final_path) {
        let _ = fs::remove_file(&part_path);
        return Err(format!("Could not finalize installer file: {err}"));
    }
    Ok(final_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_update_messages_are_english_and_versioned() {
        assert_eq!(
            message(&Ok(None)),
            format!(
                "You are using the latest version of WallpaperLive (v{}).",
                env!("CARGO_PKG_VERSION")
            )
        );
        let newer = message(&Ok(Some("v9.0.0".into())));
        assert!(newer.contains("v9.0.0") && newer.contains("Open the GitHub release page?"));
        let failed = message(&Err("GitHub update check returned HTTP 403".into()));
        assert!(failed.contains("HTTP 403") && !failed.contains("latest version"));
    }
    #[test]
    fn versions_are_strict_numeric_triples() {
        assert_eq!(version("0.1.1"), Some((0, 1, 1)));
        assert!(version("0.10.0") > version("0.9.99"));
        for bad in [
            "",
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            "01.2.3",
            "1.2.03",
            "1.2.3-alpha",
            "1.2.3+build",
            "1.-2.3",
            "1. 2.3",
            "1.2.3\n",
            "4294967296.0.0",
        ] {
            assert_eq!(version(bad), None, "accepted {bad}");
        }
    }
    #[test]
    fn cancellation_prevents_any_network_request() {
        assert!(
            check(&AtomicBool::new(true))
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(checkpoint(&AtomicBool::new(false), Instant::now()).is_err());
    }
    #[test]
    #[ignore = "One read-only HTTPS request to GitHub latest release; no download, install, or UI"]
    fn native_latest_release_check() -> Result<(), String> {
        let result = check(&AtomicBool::new(false))?;
        if let Some(tag) = &result {
            let remote = tag
                .strip_prefix('v')
                .and_then(version)
                .ok_or("Noncanonical release tag")?;
            assert!(Some(remote) > version(env!("CARGO_PKG_VERSION")));
        }
        println!("Latest stable update: {result:?}");
        Ok(())
    }
    #[test]
    fn release_schema_filters_drafts_prereleases_and_untrusted_urls() {
        assert_eq!(parse_release(r#"{"tag_name":"v4294967295.0.0","draft":false,"prerelease":false,"html_url":"https://invalid.example/"}"#).unwrap(), Some("v4294967295.0.0".into()));
        let same = format!(
            r#"{{"tag_name":"v{}","draft":false,"prerelease":false}}"#,
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(parse_release(&same).unwrap(), None);
        assert_eq!(
            parse_release(r#"{"tag_name":"v0.0.0","draft":false,"prerelease":false}"#).unwrap(),
            None
        );
        assert_eq!(
            parse_release(r#"{"tag_name":"v9.0.0","draft":true,"prerelease":false}"#).unwrap(),
            None
        );
        assert_eq!(
            parse_release(r#"{"tag_name":"v9.0.0-beta","draft":false,"prerelease":true}"#).unwrap(),
            None
        );
        for bad in [
            "not JSON",
            "[]",
            "{}",
            r#"{"tag_name":"v9.0.0","draft":"false","prerelease":false}"#,
            r#"{"tag_name":9,"draft":false,"prerelease":false}"#,
            r#"{"tag_name":"v01.2.3","draft":false,"prerelease":false}"#,
            r#"{"tag_name":"v9.0.0-beta","draft":false,"prerelease":false}"#,
        ] {
            assert!(parse_release(bad).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn checksum_and_url_validation_enforce_github_hosts_and_exact_digests() {
        assert_eq!(
            release_page_url(Some("v0.2.2")),
            "https://github.com/Raflyf/WallpaperLive/releases/tag/v0.2.2"
        );
        assert_eq!(
            installer_filename("v0.2.2").unwrap(),
            "wallpaperlivesetup-v0.2.2.exe"
        );
        assert!(installer_filename("../evil").is_err());

        assert!(
            split_https_url(
                "https://github.com/Raflyf/WallpaperLive/releases/download/v0.2.2/a.exe"
            )
            .is_ok()
        );
        assert!(
            split_https_url("https://objects.githubusercontent.com/github-production/foo").is_ok()
        );
        assert!(
            split_https_url("https://release-assets.githubusercontent.com/github-production/foo")
                .is_ok()
        );
        assert!(split_https_url("http://github.com/a").is_err());
        assert!(split_https_url("https://evil.example/a.exe").is_err());
        assert!(split_https_url("https://github.com@evil.example/a").is_err());

        let expected = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let manifest = format!("{expected}  wallpaperlivesetup-v0.2.2.exe\n");
        let parsed = parse_sha256sums(&manifest, "wallpaperlivesetup-v0.2.2.exe").unwrap();
        assert_eq!(parsed[0], 0x01);
        assert_eq!(parsed[31], 0xef);
        assert!(parse_sha256sums(&manifest, "other.exe").is_err());
        assert!(
            parse_sha256sums(
                "deadbeef  wallpaperlivesetup-v0.2.2.exe\n",
                "wallpaperlivesetup-v0.2.2.exe"
            )
            .is_err()
        );

        let tmp = std::env::temp_dir().join(format!("wl-sha256-test-{}.bin", std::process::id()));
        fs::write(&tmp, b"abc").unwrap();
        let digest = sha256_file(&tmp).unwrap();
        let _ = fs::remove_file(&tmp);
        assert_eq!(
            digest,
            [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
                0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
                0xf2, 0x00, 0x15, 0xad
            ]
        );
    }
}
