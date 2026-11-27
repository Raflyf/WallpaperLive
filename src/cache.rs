use crate::{
    core::{FPS_OPTIONS, GpuPreference, Quality, validate_media, validate_path_syntax},
    mpv::{self, MediaInfo},
    thumbnail::SilentChild,
};
use std::{
    ffi::OsStr,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::{
        Security::Cryptography::*,
        Storage::FileSystem::GetDiskFreeSpaceExW,
        System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, SetPriorityClass},
    },
    core::PCWSTR,
};

pub const CACHE_FPS: u32 = 60;
const MAX_OUTPUT: u64 = 512 * 1024 * 1024;
const MAX_CACHE: u64 = 2 * 1024 * 1024 * 1024;
const RESPONSE_LIMIT: u64 = 65536;

#[derive(Debug)]
pub struct Prepared {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub cached: bool,
    pub note: String,
}
#[allow(clippy::too_many_arguments)]
pub fn prepare(
    _dll: &Path,
    source: &Path,
    directory: &Path,
    display: (u32, u32),
    fps: u32,
    quality: Quality,
    gpu_preference: GpuPreference,
    cancelled: &AtomicBool,
) -> Result<Prepared, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Preparation cancelled".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let w_arg = display.0.to_string();
    let h_arg = display.1.to_string();
    let fps_arg = fps.to_string();
    // Do not pass arbitrary DLL paths on the CLI; worker resolves its own runtime\libmpv-2.dll.
    let args = [
        OsStr::new("--prepare-cache"),
        source.as_os_str(),
        directory.as_os_str(),
        OsStr::new(&w_arg),
        OsStr::new(&h_arg),
        OsStr::new(&fps_arg),
        OsStr::new(quality.key()),
        OsStr::new(gpu_preference.key()),
    ];
    let child = SilentChild::spawn(&exe, &args, 0x0800_4000)
        .map_err(|e| format!("Could not start video preparation: {e}"))?;
    let text = preparation_output(child, Some(directory), cancelled)?;
    let result = parse_response(&text)?;
    let original = validate_media(source)?;
    let root = directory.canonicalize().map_err(|e| e.to_string())?;
    let actual = result.path.canonicalize().map_err(|e| e.to_string())?;
    if result.path != original && (!result.cached || !actual.starts_with(root)) {
        return Err("Preparation returned an unexpected file".into());
    }
    Ok(result)
}

fn remove_pid_partials(directory: &Path, pid: u32) {
    let prefix = format!("partial-{pid}-");
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    use std::os::windows::fs::MetadataExt;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.starts_with(&prefix)
            && name.ends_with(".mp4")
            && fs::symlink_metadata(&path)
                .is_ok_and(|meta| meta.is_file() && meta.file_attributes() & 0x400 == 0)
        {
            let _ = fs::remove_file(&path);
        }
    }
}

fn preparation_output(
    mut child: SilentChild,
    directory: Option<&Path>,
    cancelled: &AtomicBool,
) -> Result<String, String> {
    let pid = child.pid();
    let stdout = child
        .take_stdout()
        .ok_or("Preparation worker has no output")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::with_capacity(512);
        stdout
            .take(RESPONSE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    let wait_result = loop {
        if cancelled.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(920) {
            break match child.kill_and_wait() {
                Ok(()) => Err(if cancelled.load(Ordering::Relaxed) {
                    "Preparation cancelled"
                } else {
                    "Video preparation timed out"
                }
                .into()),
                Err(e) => Err(e),
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
    let cleanup = || {
        if let Some(dir) = directory {
            remove_pid_partials(dir, pid);
        }
    };
    let success = match wait_result {
        Ok(ok) => ok,
        Err(e) => {
            cleanup();
            return Err(e);
        }
    };
    let bytes = match read_result
        .map_err(|_| "Preparation reader failed".to_owned())
        .and_then(|res| res.map_err(|e| e.to_string()))
    {
        Ok(bytes) => bytes,
        Err(e) => {
            cleanup();
            return Err(e);
        }
    };
    if bytes.len() > RESPONSE_LIMIT as usize {
        cleanup();
        return Err("Preparation response is too large".into());
    }
    let Ok(text) = String::from_utf8(bytes) else {
        cleanup();
        return Err("Invalid preparation response".into());
    };
    if !success {
        cleanup();
        return Err(text
            .strip_prefix("error\n")
            .unwrap_or("Video preparation failed")
            .trim()
            .to_owned());
    }
    if cancelled.load(Ordering::Relaxed) {
        cleanup();
        return Err("Preparation cancelled".into());
    }
    Ok(text)
}

fn parse_response(text: &str) -> Result<Prepared, String> {
    let lines: Vec<_> = text.lines().collect();
    if lines.len() != 8 || lines[0] != "ready=1" {
        return Err("Invalid preparation response".into());
    }
    let width = lines[2]
        .parse::<u32>()
        .map_err(|_| "Invalid prepared width")?;
    let height = lines[3]
        .parse::<u32>()
        .map_err(|_| "Invalid prepared height")?;
    let fps = lines[4]
        .parse::<f64>()
        .map_err(|_| "Invalid prepared frame rate")?;
    if !(1..=32768).contains(&width)
        || !(1..=32768).contains(&height)
        || !fps.is_finite()
        || !(0.0..=1000.0).contains(&fps)
        || !matches!(lines[5], "true" | "false")
        || lines[6] != "end=1"
    {
        return Err("Invalid prepared media properties".into());
    }
    Ok(Prepared {
        path: validate_media(Path::new(lines[1]))?,
        width,
        height,
        fps,
        cached: lines[5] == "true",
        note: lines[7].to_owned(),
    })
}
fn resolve_worker_decoder(explicit_arg: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(requested) = explicit_arg {
        let valid_name = requested
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("libmpv-2.dll"));
        if !valid_name
            || !requested.is_absolute()
            || validate_path_syntax(requested).is_err()
            || crate::core::has_reparse_ancestor(requested)?
        {
            return Err("Arbitrary external decoder path is not allowed".into());
        }
        let trusted = mpv::trusted_runtime_decoder_path()?;
        let req_canon = requested
            .canonicalize()
            .map_err(|_| "Arbitrary external decoder path is not allowed".to_owned())?;
        let trusted_canon = trusted
            .canonicalize()
            .map_err(|e| format!("Could not resolve runtime decoder: {e}"))?;
        if !req_canon
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&trusted_canon.as_os_str().to_string_lossy())
        {
            return Err("Arbitrary external decoder path is not allowed".into());
        }
        return Ok(trusted);
    }
    mpv::trusted_runtime_decoder_path()
}

pub fn worker(args: &[std::ffi::OsString]) -> Result<(), String> {
    if !(5..=8).contains(&args.len()) {
        return Err("Invalid preparation arguments".into());
    }
    // SAFETY: keep cache encoding unobtrusive via below-normal scheduling priority without single-core pinning.
    unsafe {
        let _ = SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
    }
    let parse_u32 = |arg: &std::ffi::OsString| arg.to_str().and_then(|s| s.parse::<u32>().ok());
    // Support current 5..=7 arg layout (internal runtime decoder resolution) and reject any
    // legacy 6..=8 arg invocation whose DLL argument is not the exact canonical runtime DLL.
    let (explicit_dll, offset) = if args.len() <= 7
        && parse_u32(&args[2]).is_some()
        && parse_u32(&args[3]).is_some()
        && parse_u32(&args[4]).is_some()
    {
        (None, 0usize)
    } else if (6..=8).contains(&args.len()) {
        (Some(Path::new(&args[0])), 1usize)
    } else {
        return Err("Invalid preparation arguments".into());
    };
    let number = |i: usize| {
        args.get(offset + i)
            .and_then(parse_u32)
            .ok_or("Invalid preparation dimensions".to_owned())
    };
    let dims = (number(2)?, number(3)?);
    let fps = number(4)?;
    let quality = if let Some(raw) = args.get(offset + 5) {
        raw.to_str()
            .and_then(Quality::from_key)
            .ok_or("Invalid preparation quality".to_owned())?
    } else {
        Quality::Optimize
    };
    let gpu_preference = if let Some(raw) = args.get(offset + 6) {
        raw.to_str()
            .and_then(GpuPreference::from_key)
            .ok_or("Invalid preparation GPU preference".to_owned())?
    } else {
        GpuPreference::Auto
    };
    let dll = resolve_worker_decoder(explicit_dll)?;
    let prepared = prepare_local_with_options(
        &dll,
        Path::new(&args[offset]),
        Path::new(&args[offset + 1]),
        dims,
        fps,
        quality,
        gpu_preference,
        &AtomicBool::new(false),
    )?;
    let path = prepared
        .path
        .to_str()
        .ok_or("Prepared path is not valid Unicode")?;
    let text = format!(
        "ready=1\n{path}\n{}\n{}\n{}\n{}\nend=1\n{}\n",
        prepared.width, prepared.height, prepared.fps, prepared.cached, prepared.note
    );
    std::io::stdout()
        .lock()
        .write_all(text.as_bytes())
        .map_err(|e| e.to_string())
}

fn target_size_for_quality(
    width: u32,
    height: u32,
    display: (u32, u32),
    quality: Quality,
) -> Result<(u32, u32), String> {
    let capped_display = if quality == Quality::Low {
        if display.0 >= display.1 {
            (display.0.min(1280), display.1.min(720))
        } else {
            (display.0.min(720), display.1.min(1280))
        }
    } else {
        display
    };
    target_size(width, height, capped_display)
}

fn target_size(width: u32, height: u32, display: (u32, u32)) -> Result<(u32, u32), String> {
    if [width, height, display.0, display.1]
        .iter()
        .any(|n| !(1..=32768).contains(n))
    {
        return Err("Invalid media or display dimensions".into());
    }
    if width <= display.0 && height <= display.1 {
        return Ok((width, height));
    }
    let ratio =
        (f64::from(display.0) / f64::from(width)).min(f64::from(display.1) / f64::from(height));
    let scaled = |value| ((f64::from(value) * ratio).floor() as u32 / 2 * 2).max(2);
    Ok((scaled(width), scaled(height)))
}
fn valid_output(output: &MediaInfo, source: &MediaInfo, dimensions: (u32, u32), fps: u32) -> bool {
    !output.image
        && !output.hdr
        && (output.width, output.height) == dimensions
        && output.fps.is_finite()
        && output.fps > 0.0
        && (output.fps - source.fps.min(f64::from(fps))).abs() <= 0.2f64.max(2.0 / output.duration)
        && (output.duration - source.duration).abs() <= 0.5
        && source.audio == output.audio
}
fn cache_bytes(directory: &Path) -> Result<u64, String> {
    fs::read_dir(directory)
        .map_err(|e| e.to_string())?
        .try_fold(0u64, |size, entry| {
            let metadata = entry
                .map_err(|e| e.to_string())?
                .metadata()
                .map_err(|e| e.to_string())?;
            size.checked_add(if metadata.is_file() {
                metadata.len()
            } else {
                0
            })
            .ok_or("Cache size overflow".into())
        })
}
pub fn cache_size(directory: &Path) -> u64 {
    cache_bytes(directory).unwrap_or(0)
}
pub fn clear_unused_cache(directory: &Path, preserve: &[PathBuf]) -> Result<(usize, u64), String> {
    validate_path_syntax(directory)?;
    if !directory.is_dir() {
        return Ok((0, 0));
    }
    let protected: Vec<_> = preserve
        .iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect();
    let mut count = 0;
    let mut freed = 0;
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if (name.starts_with("wl-v2-") && name.ends_with(".mp4"))
                || name.starts_with("partial-")
            {
                let actual = path
                    .canonicalize()
                    .map_err(|e| format!("Cache path could not be verified: {e}"))?;
                if preserve.iter().any(|p| p == &path)
                    || protected.iter().any(|p| {
                        p.as_os_str()
                            .to_string_lossy()
                            .eq_ignore_ascii_case(&actual.as_os_str().to_string_lossy())
                    })
                {
                    continue;
                }
                let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
                use std::os::windows::fs::MetadataExt;
                if !meta.is_file() || meta.file_attributes() & 0x400 != 0 {
                    continue;
                }
                if let Ok(meta) = entry.metadata() {
                    let len = meta.len();
                    if fs::remove_file(&path).is_ok() {
                        count += 1;
                        freed += len;
                    }
                }
            }
        }
    }
    Ok((count, freed))
}
fn unchanged(path: &Path, size: u64, modified: SystemTime) -> Result<(), String> {
    let now = fs::metadata(path).map_err(|e| e.to_string())?;
    if now.len() != size || now.modified().map_err(|e| e.to_string())? != modified {
        return Err("Source changed during preparation; original file was kept".into());
    }
    Ok(())
}
#[cfg(test)]
pub fn prepare_local(
    dll: &Path,
    source: &Path,
    directory: &Path,
    display: (u32, u32),
    fps: u32,
    cancelled: &AtomicBool,
) -> Result<Prepared, String> {
    prepare_local_with_quality(
        dll,
        source,
        directory,
        display,
        fps,
        Quality::Optimize,
        cancelled,
    )
}

#[cfg(test)]
pub fn prepare_local_with_quality(
    dll: &Path,
    source: &Path,
    directory: &Path,
    display: (u32, u32),
    fps: u32,
    quality: Quality,
    cancelled: &AtomicBool,
) -> Result<Prepared, String> {
    prepare_local_with_options(
        dll,
        source,
        directory,
        display,
        fps,
        quality,
        GpuPreference::Auto,
        cancelled,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_local_with_options(
    dll: &Path,
    source: &Path,
    directory: &Path,
    display: (u32, u32),
    fps: u32,
    quality: Quality,
    gpu_preference: GpuPreference,
    cancelled: &AtomicBool,
) -> Result<Prepared, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Preparation cancelled".into());
    }
    if !FPS_OPTIONS.contains(&fps) {
        return Err("Invalid frame-rate limit".into());
    }
    let source = validate_media(source)?;
    validate_path_syntax(directory)?;
    fs::create_dir_all(directory).map_err(|e| format!("Could not create video cache: {e}"))?;
    // Pin verified libmpv-2.dll once for the entire probe -> encode -> validate workflow.
    let _decoder_session = mpv::open_session(dll)?;
    let probed = mpv::probe_source(dll, &source)?;
    let info = &probed.info;
    if quality == Quality::Native {
        return Ok(Prepared {
            path: source,
            width: info.width,
            height: info.height,
            fps: info.fps,
            cached: false,
            note: "Native quality (original media)".into(),
        });
    }
    let target = target_size_for_quality(info.width, info.height, display, quality)?;
    let mut prepared = Prepared {
        path: source.clone(),
        width: info.width,
        height: info.height,
        fps: info.fps,
        cached: false,
        note: "Original media".into(),
    };
    let image_file = matches!(
        source
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("gif" | "apng" | "png" | "jpg" | "jpeg" | "bmp" | "webp")
    );
    if info.image || image_file {
        prepared.note = "Original image (video cache only)".into();
        return Ok(prepared);
    }
    if info.hdr {
        prepared.note = "Original HDR (conversion unavailable)".into();
        return Ok(prepared);
    }
    if target == (info.width, info.height) && info.fps <= f64::from(fps) + 0.05 {
        return Ok(prepared);
    }
    if info.duration <= 0.0 || !info.duration.is_finite() || info.duration > 3600.0 {
        return Err("Automatic cache supports finite videos up to one hour".into());
    }
    let target = (target.0 / 2 * 2, target.1 / 2 * 2);
    if target.0 < 2 || target.1 < 2 {
        return Err("Video dimensions cannot be encoded safely".into());
    }
    let metadata = fs::metadata(&source).map_err(|e| e.to_string())?;
    if metadata.len() > 16 * 1024 * 1024 * 1024 {
        return Err("Automatic cache supports source files up to 16 GiB".into());
    }
    let modified = metadata.modified().map_err(|e| e.to_string())?;
    let hash = sha256(&source, cancelled)?;
    unchanged(&source, metadata.len(), modified)?;
    let destination = directory.join(format!(
        "wl-v2-{hash}-{}x{}-{fps}-{}.mp4",
        target.0,
        target.1,
        quality.key()
    ));
    if destination.is_file() {
        if !(1..=MAX_OUTPUT).contains(&fs::metadata(&destination).map_err(|e| e.to_string())?.len())
        {
            return Err("Cached video exceeds file limit".into());
        }
        let output = mpv::probe_media(dll, &destination)?;
        unchanged(&source, metadata.len(), modified)?;
        if !valid_output(&output, info, target, fps) {
            return Err("Cached video is invalid; move it aside before retrying".into());
        }
        prepared.path = destination;
        prepared.width = output.width;
        prepared.height = output.height;
        prepared.fps = info.fps.min(f64::from(fps));
        prepared.cached = true;
        prepared.note = "Screen-sized cache".into();
        return Ok(prepared);
    }
    if cache_bytes(directory)? > MAX_CACHE - MAX_OUTPUT {
        return Err(
            "Video cache is full (2 GiB limit); move unused cache files aside before retrying"
                .into(),
        );
    }
    let wide: Vec<_> = directory
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut free = 0;
    unsafe {
        GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None)
            .map_err(|e| e.to_string())?;
    }
    if free < MAX_OUTPUT + 64 * 1024 * 1024 {
        return Err("Not enough disk space to prepare wallpaper".into());
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    struct PartialCleanup(Option<PathBuf>);
    impl Drop for PartialCleanup {
        fn drop(&mut self) {
            if let Some(path) = self.0.take() {
                let _ = fs::remove_file(path);
            }
        }
    }
    let partial = directory.join(format!("partial-{}-{stamp}.mp4", std::process::id()));
    if partial.exists() || destination.exists() {
        return Err("Cache destination already exists; existing bytes were kept".into());
    }
    let mut partial_guard = PartialCleanup(Some(partial.clone()));
    let output = mpv::encode_probed(
        dll,
        &source,
        &partial,
        &probed,
        target.0,
        target.1,
        fps,
        quality,
        gpu_preference,
        cancelled,
    )?;
    if fs::metadata(&partial).map_err(|e| e.to_string())?.len() > MAX_OUTPUT {
        return Err("Prepared video exceeds 512 MiB; partial cache is not used".into());
    }
    unchanged(&source, metadata.len(), modified)?;
    if !valid_output(&output, info, target, fps) {
        return Err("Prepared video failed resolution, frame-rate, duration or audio validation; original file was kept".into());
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("Preparation cancelled".into());
    }
    fs::rename(&partial, &destination)
        .map_err(|e| format!("Could not commit video cache; previous files were kept: {e}"))?;
    partial_guard.0 = None;
    prepared.path = destination;
    prepared.width = output.width;
    prepared.height = output.height;
    prepared.fps = info.fps.min(f64::from(fps));
    prepared.cached = true;
    prepared.note = "Screen-sized cache".into();
    Ok(prepared)
}

fn sha256(path: &Path, cancelled: &AtomicBool) -> Result<String, String> {
    struct Hash(BCRYPT_HASH_HANDLE);
    impl Drop for Hash {
        fn drop(&mut self) {
            unsafe {
                let _ = BCryptDestroyHash(self.0);
            }
        }
    }
    let check = |status: windows::Win32::Foundation::NTSTATUS| {
        if status.0 >= 0 {
            Ok(())
        } else {
            Err(format!("Cache checksum failed ({:#x})", status.0 as u32))
        }
    };
    let mut handle = BCRYPT_HASH_HANDLE::default();
    unsafe {
        check(BCryptCreateHash(
            BCRYPT_SHA256_ALG_HANDLE,
            &mut handle,
            None,
            None,
            0,
        ))?;
    }
    let hash = Hash(handle);
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = [0u8; 65536];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Preparation cancelled".into());
        }
        let n = file.read(&mut bytes).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        unsafe {
            check(BCryptHashData(hash.0, &bytes[..n], 0))?;
        }
    }
    let mut digest = [0u8; 32];
    unsafe {
        check(BCryptFinishHash(hash.0, &mut digest, 0))?;
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_preparation_child_is_killed_and_reaped() {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
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
            0x08000000,
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
        let started = Instant::now();
        assert!(
            preparation_output(child, None, &AtomicBool::new(true))
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        unsafe {
            assert_eq!(
                WaitForSingleObject(HANDLE(process.as_raw_handle()), 0),
                WAIT_OBJECT_0
            );
        }
    }

    #[test]
    #[ignore = "Requires pinned runtime DLL and owned sample.mp4; retains cache/source fixtures under scratch, no UI"]
    fn native_cache_reuse_and_source_invalidation() -> Result<(), String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let directory = root.join(format!(
            "scratch/cache-reuse-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let source = directory.join("original.mp4");
        let original =
            fs::read(root.join("scratch/media/sample.mp4")).map_err(|e| e.to_string())?;
        fs::write(&source, &original).map_err(|e| e.to_string())?;
        let dll = root.join("runtime/libmpv-2.dll");
        let flag = AtomicBool::new(false);
        let first = prepare_local(&dll, &source, &directory, (320, 180), 30, &flag)?;
        assert!(first.cached && first.width == 320 && first.height == 180 && first.fps <= 30.0);
        let metadata = fs::metadata(&first.path).map_err(|e| e.to_string())?;
        let copy = fs::read(&first.path).map_err(|e| e.to_string())?;
        let second = prepare_local(&dll, &source, &directory, (320, 180), 30, &flag)?;
        assert_eq!(first.path, second.path);
        assert_eq!(
            fs::metadata(&second.path)
                .map_err(|e| e.to_string())?
                .modified()
                .unwrap(),
            metadata.modified().unwrap()
        );
        assert_eq!(fs::read(&source).map_err(|e| e.to_string())?, original);
        let retained = directory.join("retained-cache.mp4");
        assert!(!retained.exists());
        fs::rename(&first.path, &retained).map_err(|e| e.to_string())?;
        let restored = prepare_local(&dll, &source, &directory, (320, 180), 30, &flag)?;
        assert_eq!(restored.path, first.path);
        assert_eq!(fs::read(&retained).map_err(|e| e.to_string())?, copy);
        let mut changed = original.clone();
        changed.push(0); // Harmless MP4 trailing byte tests content identity at the same source path.
        fs::write(&source, &changed).map_err(|e| e.to_string())?;
        let refreshed = prepare_local(&dll, &source, &directory, (320, 180), 30, &flag)?;
        assert_ne!(refreshed.path, restored.path);
        assert!(restored.path.is_file());
        assert_eq!(fs::read(&source).map_err(|e| e.to_string())?, changed);
        assert!(
            prepare_local(
                &dll,
                &source,
                &directory,
                (320, 180),
                30,
                &AtomicBool::new(true),
            )
            .is_err()
        );
        println!(
            "Cache reuse, missing-cache recovery, source-content invalidation and source preservation passed"
        );
        Ok(())
    }

    #[test]
    fn display_target_never_upscales_and_keeps_aspect() {
        assert_eq!(target_size(3840, 2160, (1920, 1080)).unwrap(), (1920, 1080));
        assert_eq!(target_size(3840, 2160, (3840, 2160)).unwrap(), (3840, 2160));
        assert_eq!(target_size(1280, 720, (1920, 1080)).unwrap(), (1280, 720));
        assert_eq!(target_size(2160, 3840, (1920, 1080)).unwrap(), (606, 1080));
        assert!(target_size(0, 2160, (1920, 1080)).is_err());
        assert!(parse_response("ready=1\nnot media").is_err());
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scratch")
            .join(format!(
                "cache-hash-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("hash.txt");
        fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256(&path, &AtomicBool::new(false)).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(sha256(&path, &AtomicBool::new(true)).is_err());
        assert_eq!(fs::read(path).unwrap(), b"abc");
    }

    #[test]
    fn cache_clear_preserves_imported_files() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scratch")
            .join(format!(
                "test-cache-prune-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&dir).unwrap();
        let file1 = dir.join("wl-v2-abc-1920x1080-60.mp4");
        let file2 = dir.join("wl-v2-def-1920x1080-60.mp4");
        let partial = dir.join("partial-1234-5678.mp4");
        let other = dir.join("unrelated.txt");
        fs::write(&file1, vec![0u8; 1000]).unwrap();
        fs::write(&file2, vec![0u8; 2000]).unwrap();
        fs::write(&partial, vec![0u8; 500]).unwrap();
        fs::write(&other, vec![0u8; 300]).unwrap();
        assert_eq!(cache_size(&dir), 3800);
        let protected = PathBuf::from(file2.to_string_lossy().to_lowercase());
        let (count, freed) = clear_unused_cache(&dir, std::slice::from_ref(&protected)).unwrap();
        assert_eq!(count, 2);
        assert_eq!(freed, 1500);
        assert!(!file1.exists());
        assert!(!partial.exists());
        assert!(file2.exists());
        assert!(other.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn low_quality_caps_cache_dimensions_to_720p_without_distorting_aspect() {
        assert_eq!(
            target_size_for_quality(3840, 2160, (2560, 1440), Quality::High).unwrap(),
            (2560, 1440)
        );
        assert_eq!(
            target_size_for_quality(3840, 2160, (2560, 1440), Quality::Low).unwrap(),
            (1280, 720)
        );
        assert_eq!(
            target_size_for_quality(2160, 3840, (1080, 1920), Quality::Low).unwrap(),
            (720, 1280)
        );
    }

    #[test]
    fn worker_validates_argument_count_and_gpu_preference_key() {
        assert!(worker(&[]).is_err());
        let bad_gpu: Vec<std::ffi::OsString> = vec![
            "in.mp4".into(),
            "cache".into(),
            "1920".into(),
            "1080".into(),
            "60".into(),
            "optimize".into(),
            "invalid-gpu".into(),
        ];
        assert_eq!(
            worker(&bad_gpu).unwrap_err(),
            "Invalid preparation GPU preference"
        );
        for bad_dll in [
            "libmpv.dll",
            r"C:\Windows\System32\kernel32.dll",
            r"\\attacker\share\evil.dll",
        ] {
            let evil: Vec<std::ffi::OsString> = vec![
                bad_dll.into(),
                r"C:\in.mp4".into(),
                r"C:\cache".into(),
                "1920".into(),
                "1080".into(),
                "60".into(),
                "optimize".into(),
                "auto".into(),
            ];
            let err = worker(&evil).unwrap_err();
            assert!(
                err.contains("Arbitrary external decoder path")
                    || err.contains("Decoder is missing"),
                "unexpected error for {bad_dll}: {err}"
            );
        }
        for pref in crate::core::GPU_OPTIONS {
            assert_eq!(GpuPreference::from_key(pref.key()), Some(pref));
        }
    }
}
