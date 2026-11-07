use std::{
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use windows::{
    Data::Json::{IJsonObjectStatics, JsonObject},
    Win32::{
        Networking::WinHttp::*,
        System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    },
    core::{HSTRING, Interface, PCWSTR, w},
};

const MAX_BODY: usize = 128 * 1024;
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);

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
        return Err("Update check cancelled".into());
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("Update check timed out".into());
    }
    Ok(remaining.min(Duration::from_secs(3)).as_millis().max(1) as i32)
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

fn fetch(cancelled: &AtomicBool, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
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
        return Ok(None);
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
    Ok(Some(body))
}

pub fn check(cancelled: &AtomicBool) -> Result<Option<String>, String> {
    let deadline = Instant::now() + TOTAL_TIMEOUT;
    // ponytail: native synchronous calls have 3s phase limits; cancellation waits for the current call.
    let body = fetch(cancelled, deadline)?;
    checkpoint(cancelled, deadline)?;
    let result = match body {
        Some(body) => {
            parse_release(std::str::from_utf8(&body).map_err(|_| "Release JSON is not UTF-8")?)
        }
        None => Ok(None),
    };
    checkpoint(cancelled, deadline)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
