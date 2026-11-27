use std::{
    ffi::{OsStr, c_void},
    fs::{self, File, OpenOptions},
    io::{BufWriter, Read, Write},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows::{
    Data::Json::{IJsonObjectStatics, JsonObject},
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        Networking::WinHttp::*,
        Security::Cryptography::{
            BCRYPT_ALG_HANDLE, BCRYPT_HASH_HANDLE, BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS,
            BCRYPT_SHA256_ALGORITHM, BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptCloseAlgorithmProvider,
            BCryptCreateHash, BCryptDestroyHash, BCryptFinishHash, BCryptGenRandom, BCryptHashData,
            BCryptOpenAlgorithmProvider,
        },
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, GetFileInformationByHandle,
        },
        System::{
            Threading::{
                CREATE_DEFAULT_ERROR_MODE, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
            },
            WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        },
    },
    core::{HSTRING, Interface, PCWSTR, PWSTR, w},
};

const MAX_BODY: usize = 128 * 1024;
const MAX_CHECKSUMS_BODY: usize = 32 * 1024;
const MAX_INSTALLER_BYTES: u64 = 256 * 1024 * 1024;
const DECODER_URL: &str = "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261107/mpv-dev-x86_64-20261107-git-9028187538.7z";
const DECODER_ARCHIVE_SHA256: &str =
    "df7c433a2d481fdaf6163b201f26abbb0eff0c215c51368397088c6d8f7a001b";
pub const DECODER_DLL_SHA256: &str =
    "c91949577ae9cc2d500208da51f2c4a24cfa7a02d0ce6e130bf67ec738b450bb";
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const PHASE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug)]
pub enum DownloadEvent {
    Progress(Option<u8>),
    Verifying,
    Ready(PathBuf, [u8; 32]),
    Failed(String),
}

pub fn expected_decoder_sha256() -> Result<[u8; 32], String> {
    parse_sha256_hex(DECODER_DLL_SHA256)
}

pub fn random_bytes(buf: &mut [u8]) -> Result<(), String> {
    // SAFETY: fills caller buffer using Windows CNG system preferred CSPRNG.
    unsafe { BCryptGenRandom(None, buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
        .ok()
        .map_err(|e| format!("System random generator failed: {e}"))
}

pub fn random_hex_16() -> Result<String, String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0u8; 16];
    random_bytes(&mut bytes)?;
    let mut out = String::with_capacity(32);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    Ok(out)
}

fn configure_tls_session(session: &Handle) -> Result<(), String> {
    // Enforce TLS 1.2+ (preferring TLS 1.2 | TLS 1.3; fall back to TLS 1.2 on older Windows 10 builds).
    if session
        .option(
            WINHTTP_OPTION_SECURE_PROTOCOLS,
            WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3,
        )
        .is_err()
    {
        session.option(
            WINHTTP_OPTION_SECURE_PROTOCOLS,
            WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2,
        )?;
    }
    Ok(())
}

fn harden_https_request(request: &Handle) -> Result<(), String> {
    request.option(
        WINHTTP_OPTION_DISABLE_FEATURE,
        WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION,
    )?;
    request.option(
        WINHTTP_OPTION_AUTOLOGON_POLICY,
        WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH,
    )?;
    // Enforce certificate revocation checking on TLS connections.
    request.option(WINHTTP_OPTION_ENABLE_FEATURE, WINHTTP_ENABLE_SSL_REVOCATION)?;
    Ok(())
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

#[cfg(test)]
pub fn installer_filename(tag: &str) -> Result<String, String> {
    let canonical = normalize_tag(tag)?;
    Ok(format!("wallpaperlivesetup-{canonical}.msi"))
}

pub fn resolve_installer_from_sums(
    sums_text: &str,
    tag: &str,
) -> Result<(String, [u8; 32]), String> {
    let canonical = normalize_tag(tag)?;
    let msi_name = format!("wallpaperlivesetup-{canonical}.msi");
    if let Ok(digest) = parse_sha256sums(sums_text, &msi_name) {
        return Ok((msi_name, digest));
    }
    let exe_name = format!("wallpaperlivesetup-{canonical}.exe");
    let digest = parse_sha256sums(sums_text, &exe_name)?;
    Ok((exe_name, digest))
}

pub fn allowed_download_host(host: &str) -> bool {
    matches!(
        host.to_ascii_lowercase().as_str(),
        "github.com" | "objects.githubusercontent.com" | "release-assets.githubusercontent.com"
    )
}

pub fn split_https_url(url: &str) -> Result<(&str, &str), String> {
    if url
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\0')
    {
        return Err("Update download URL contains disallowed characters".into());
    }
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
        || authority.contains('\\')
        || !allowed_download_host(authority)
    {
        return Err("Untrusted update download host".into());
    }
    Ok((authority, path))
}

pub fn parse_sha256_hex(hex: &str) -> Result<[u8; 32], String> {
    let hex = hex.trim();
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid SHA256SUMS digest entry".into());
    }
    let mut digest = [0u8; 32];
    for (i, slot) in digest.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| "Invalid SHA256SUMS digest entry")?;
    }
    Ok(digest)
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
        return parse_sha256_hex(hex_part);
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
    configure_tls_session(&session)?;
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
    harden_https_request(&request)?;
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

pub fn sha256_reader<R: Read>(reader: &mut R) -> Result<[u8; 32], String> {
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

    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Could not hash file: {e}"))?;
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

pub fn sha256_file(path: &Path) -> Result<[u8; 32], String> {
    if crate::core::has_reparse_ancestor(path)? {
        return Err("Refusing to hash reparse point or symlink path".into());
    }
    let mut file = open_regular_readonly_locked(path)?;
    sha256_reader(&mut file)
}

fn open_regular_readonly_locked(path: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .map_err(|e| format!("Could not open file for verification: {e}"))?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: valid owned file handle; inspects attributes atomically on the open handle.
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
        .map_err(|e| format!("Could not inspect file attributes: {e}"))?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0
    {
        return Err("Refusing to read reparse point or directory".into());
    }
    Ok(file)
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
    configure_tls_session(&session)?;
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
        harden_https_request(&request)?;
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

struct VerifiedDownload<'a> {
    url: &'a str,
    part_path: &'a Path,
    expected_sha: [u8; 32],
    max_bytes: u64,
    label: &'a str,
}

fn download_verified_file(
    spec: VerifiedDownload<'_>,
    cancelled: &AtomicBool,
    deadline: Instant,
    on_progress: &mut dyn FnMut(Option<u8>),
) -> Result<(), String> {
    let VerifiedDownload {
        url,
        part_path,
        expected_sha,
        max_bytes,
        label,
    } = spec;
    let (_session, _conn, request, total_bytes) =
        open_verified_https_stream(url, cancelled, deadline)?;
    if total_bytes.is_some_and(|n| n > max_bytes) {
        return Err(format!("{label} exceeds maximum allowed size"));
    }

    if crate::core::has_reparse_ancestor(part_path)? {
        return Err("Download target path must not traverse a reparse point".into());
    }
    let write_res: Result<(), String> = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(part_path)
            .map_err(|e| format!("Could not create download file: {e}"))?;
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
            if downloaded > max_bytes {
                return Err(format!("{label} download exceeded maximum allowed size"));
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
            return Err(format!("Downloaded {label} file was empty"));
        }
        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = fs::remove_file(part_path);
        return Err(err);
    }

    let actual_sha = match sha256_file(part_path) {
        Ok(digest) => digest,
        Err(err) => {
            let _ = fs::remove_file(part_path);
            return Err(err);
        }
    };
    if actual_sha != expected_sha {
        let _ = fs::remove_file(part_path);
        return Err(format!("{label} SHA-256 verification failed"));
    }
    Ok(())
}

pub fn download_verified_installer(
    tag: &str,
    dir: &Path,
    cancelled: &AtomicBool,
    mut on_progress: impl FnMut(Option<u8>),
) -> Result<(PathBuf, [u8; 32]), String> {
    let canonical = normalize_tag(tag)?;
    let sums_url = format!(
        "https://github.com/Raflyf/WallpaperLive/releases/download/{canonical}/SHA256SUMS.txt"
    );
    let deadline = Instant::now() + DOWNLOAD_TIMEOUT;

    on_progress(Some(0));
    let sums_bytes = fetch_verified_bytes(&sums_url, MAX_CHECKSUMS_BODY, cancelled, deadline)?;
    let sums_text = std::str::from_utf8(&sums_bytes).map_err(|_| "SHA256SUMS.txt is not UTF-8")?;
    let (installer_name, expected_sha) = resolve_installer_from_sums(sums_text, &canonical)?;
    let exe_url = format!(
        "https://github.com/Raflyf/WallpaperLive/releases/download/{canonical}/{installer_name}"
    );

    if crate::core::has_reparse_ancestor(dir)? {
        return Err("Updates folder must not traverse a reparse point".into());
    }
    fs::create_dir_all(dir).map_err(|e| format!("Could not prepare updates folder: {e}"))?;
    if crate::core::has_reparse_ancestor(dir)? {
        return Err("Updates folder must not be a reparse point".into());
    }
    let suffix = random_hex_16()?;
    let final_path = dir.join(&installer_name);
    let part_path = dir.join(format!("{installer_name}.{suffix}.part"));

    download_verified_file(
        VerifiedDownload {
            url: &exe_url,
            part_path: &part_path,
            expected_sha,
            max_bytes: MAX_INSTALLER_BYTES,
            label: "Installer",
        },
        cancelled,
        deadline,
        &mut on_progress,
    )?;

    if final_path.exists() {
        if crate::core::is_reparse_point(&final_path).unwrap_or(true) {
            let _ = fs::remove_file(&part_path);
            return Err("Existing installer destination is a reparse point".into());
        }
        let _ = fs::remove_file(&final_path);
    }
    if let Err(err) = fs::rename(&part_path, &final_path) {
        let _ = fs::remove_file(&part_path);
        return Err(format!("Could not finalize installer file: {err}"));
    }
    Ok((final_path, expected_sha))
}

fn system32_dir() -> PathBuf {
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join("System32")
}

fn append_quoted_arg(out: &mut Vec<u16>, arg: &OsStr) {
    out.push(b'"' as u16);
    let mut backslashes = 0usize;
    for u in arg.encode_wide() {
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

pub fn launch_verified_installer(path: &Path, expected_sha: [u8; 32]) -> Result<(), String> {
    crate::core::validate_path_syntax(path)?;
    if !crate::core::is_local_drive(path) {
        return Err("Installer must reside on a local drive".into());
    }
    if crate::core::has_reparse_ancestor(path)? {
        return Err("Refusing to launch installer through a reparse point".into());
    }
    // Hold a FILE_SHARE_READ-only handle across SHA-256 verification and CreateProcessW to prevent TOCTOU swap.
    let mut locked_file = open_regular_readonly_locked(path)?;
    let actual_sha = sha256_reader(&mut locked_file)?;
    if actual_sha != expected_sha {
        return Err("Installer SHA-256 verification failed right before launch".into());
    }

    let sys32 = system32_dir();
    let is_msi = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("msi"));
    let is_exe = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    if !is_msi && !is_exe {
        return Err("Unsupported installer file type".into());
    }

    let (app_path, mut cmd_line) = if is_msi {
        let msiexec = sys32.join("msiexec.exe");
        if !msiexec.is_file() || crate::core::has_reparse_ancestor(&msiexec)? {
            return Err("Windows Installer (msiexec.exe) is unavailable".into());
        }
        let mut cmd = Vec::new();
        append_quoted_arg(&mut cmd, msiexec.as_os_str());
        for ch in " /i ".encode_utf16() {
            cmd.push(ch);
        }
        append_quoted_arg(&mut cmd, path.as_os_str());
        cmd.push(0);
        (msiexec, cmd)
    } else {
        let mut cmd = Vec::new();
        append_quoted_arg(&mut cmd, path.as_os_str());
        cmd.push(0);
        (path.to_path_buf(), cmd)
    };

    let app_w: Vec<u16> = app_path.as_os_str().encode_wide().chain(Some(0)).collect();
    let cwd_w: Vec<u16> = sys32.as_os_str().encode_wide().chain(Some(0)).collect();
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    // SAFETY: launches verified installer or System32\msiexec.exe with CWD locked to System32.
    let res = unsafe {
        CreateProcessW(
            PCWSTR(app_w.as_ptr()),
            PWSTR(cmd_line.as_mut_ptr()),
            None,
            None,
            false,
            CREATE_DEFAULT_ERROR_MODE,
            None,
            PCWSTR(cwd_w.as_ptr()),
            &startup,
            &mut pi,
        )
    };
    drop(locked_file);
    res.map_err(|e| format!("Could not launch the downloaded installer: {e}"))?;
    // SAFETY: close process/thread handles returned by CreateProcessW immediately.
    unsafe {
        if !pi.hThread.is_invalid() {
            let _ = CloseHandle(pi.hThread);
        }
        if !pi.hProcess.is_invalid() {
            let _ = CloseHandle(pi.hProcess);
        }
    }
    Ok(())
}

fn system_tar_exe() -> Result<PathBuf, String> {
    let tar = system32_dir().join("tar.exe");
    if tar.is_file() && !crate::core::has_reparse_ancestor(&tar)? {
        Ok(tar)
    } else {
        Err("Windows tar.exe is unavailable to extract libmpv-2.dll".into())
    }
}

pub fn download_verified_decoder(
    runtime_dir: &Path,
    cancelled: &AtomicBool,
    mut on_progress: impl FnMut(Option<u8>),
) -> Result<(PathBuf, [u8; 32]), String> {
    let archive_sha = parse_sha256_hex(DECODER_ARCHIVE_SHA256)?;
    let dll_sha = parse_sha256_hex(DECODER_DLL_SHA256)?;
    if crate::core::has_reparse_ancestor(runtime_dir)? {
        return Err("Runtime folder must not traverse a reparse point".into());
    }
    let dll_path = runtime_dir.join("libmpv-2.dll");
    if dll_path.exists() {
        if crate::core::is_reparse_point(&dll_path).unwrap_or(true) || !dll_path.is_file() {
            return Err("Decoder path exists but is not a regular DLL file".into());
        }
        if sha256_file(&dll_path)? == dll_sha {
            return Ok((dll_path, dll_sha));
        }
        return Err("Existing libmpv-2.dll differs from pinned build; left untouched.".into());
    }

    fs::create_dir_all(runtime_dir)
        .map_err(|e| format!("Could not prepare runtime folder: {e}"))?;
    if crate::core::has_reparse_ancestor(runtime_dir)? {
        return Err("Runtime folder must not be a reparse point".into());
    }
    let suffix = random_hex_16()?;
    let stage = runtime_dir.join(format!(".decoder-stage-{suffix}"));
    fs::create_dir(&stage).map_err(|e| format!("Could not prepare staging folder: {e}"))?;
    let archive_path = stage.join("mpv-dev.7z");
    let staged_dll = stage.join("libmpv-2.dll");

    let cleanup = || {
        let _ = fs::remove_file(&archive_path);
        let _ = fs::remove_file(&staged_dll);
        let _ = fs::remove_dir(&stage);
    };

    let deadline = Instant::now() + DOWNLOAD_TIMEOUT;
    on_progress(Some(0));
    if let Err(err) = download_verified_file(
        VerifiedDownload {
            url: DECODER_URL,
            part_path: &archive_path,
            expected_sha: archive_sha,
            max_bytes: MAX_INSTALLER_BYTES,
            label: "Decoder archive",
        },
        cancelled,
        deadline,
        &mut on_progress,
    ) {
        cleanup();
        return Err(err);
    }

    let tar = match system_tar_exe() {
        Ok(p) => p,
        Err(err) => {
            cleanup();
            return Err(err);
        }
    };
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut child = match Command::new(&tar)
        .current_dir(&stage)
        .arg("-xf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&stage)
        .arg("libmpv-2.dll")
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            cleanup();
            return Err(format!("Could not run tar.exe: {err}"));
        }
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(exit)) => break Ok(exit),
            Ok(None) => {}
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("Could not monitor tar.exe: {err}"));
            }
        }
        if cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            break Err("Decoder download cancelled".into());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break Err("Decoder extraction timed out".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    match status {
        Ok(exit) if exit.success() => {}
        Ok(exit) => {
            cleanup();
            return Err(format!("Decoder extraction failed ({exit})"));
        }
        Err(err) => {
            cleanup();
            return Err(err);
        }
    }

    if crate::core::is_reparse_point(&staged_dll).unwrap_or(true) || !staged_dll.is_file() {
        cleanup();
        return Err("libmpv-2.dll was missing from the downloaded archive".into());
    }
    match sha256_file(&staged_dll) {
        Ok(digest) if digest == dll_sha => {}
        Ok(_) => {
            cleanup();
            return Err("Extracted libmpv-2.dll SHA-256 verification failed".into());
        }
        Err(err) => {
            cleanup();
            return Err(err);
        }
    }

    if let Err(err) = fs::rename(&staged_dll, &dll_path) {
        cleanup();
        return Err(format!("Could not install libmpv-2.dll: {err}"));
    }
    cleanup();
    Ok((dll_path, dll_sha))
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
            "wallpaperlivesetup-v0.2.2.msi"
        );
        assert!(installer_filename("../evil").is_err());

        assert!(
            split_https_url(
                "https://github.com/Raflyf/WallpaperLive/releases/download/v0.2.2/a.msi"
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
        assert!(split_https_url("https://github.com/a\r\nX-Injected: 1").is_err());
        assert!(split_https_url("https://github.com/a\0b").is_err());
        assert!(split_https_url("https://github.com/a b").is_err());
        assert!(split_https_url("https://github.com/a\tb").is_err());

        let r1 = random_hex_16().unwrap();
        let r2 = random_hex_16().unwrap();
        assert_eq!(r1.len(), 32);
        assert!(r1.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(r1, r2);

        let expected = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let manifest_msi = format!("{expected}  wallpaperlivesetup-v0.2.2.msi\n");
        let (resolved_name, resolved_sha) =
            resolve_installer_from_sums(&manifest_msi, "v0.2.2").unwrap();
        assert_eq!(resolved_name, "wallpaperlivesetup-v0.2.2.msi");
        assert_eq!(resolved_sha[0], 0x01);
        assert_eq!(resolved_sha[31], 0xef);

        let manifest_legacy = format!("{expected}  wallpaperlivesetup-v0.2.2.exe\n");
        let (legacy_name, _) = resolve_installer_from_sums(&manifest_legacy, "v0.2.2").unwrap();
        assert_eq!(legacy_name, "wallpaperlivesetup-v0.2.2.exe");

        let parsed = parse_sha256sums(&manifest_msi, "wallpaperlivesetup-v0.2.2.msi").unwrap();
        assert_eq!(parsed[0], 0x01);
        assert_eq!(parsed[31], 0xef);
        assert!(parse_sha256sums(&manifest_msi, "other.msi").is_err());
        assert!(
            parse_sha256sums(
                "deadbeef  wallpaperlivesetup-v0.2.2.msi\n",
                "wallpaperlivesetup-v0.2.2.msi"
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

    #[test]
    fn decoder_pins_validate_and_reject_mismatched_existing_dll() {
        assert!(parse_sha256_hex(DECODER_ARCHIVE_SHA256).is_ok());
        assert!(parse_sha256_hex(DECODER_DLL_SHA256).is_ok());
        assert!(split_https_url(DECODER_URL).is_ok());

        let dir = std::env::temp_dir().join(format!("wl-decoder-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let dll = dir.join("libmpv-2.dll");
        fs::write(&dll, b"wrong-bytes").unwrap();
        let err = download_verified_decoder(&dir, &AtomicBool::new(false), |_| {}).unwrap_err();
        assert!(err.contains("differs from pinned build"));
        let _ = fs::remove_file(&dll);
        let _ = fs::remove_dir(&dir);
    }
}
