use crate::{
    core::{FPS_OPTIONS, validate_media, validate_path_syntax},
    mpv::{self, MediaInfo},
};
use std::{
    fs,
    io::{Read, Write},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::{Security::Cryptography::*, Storage::FileSystem::GetDiskFreeSpaceExW},
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
struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

pub fn prepare(
    dll: &Path,
    source: &Path,
    directory: &Path,
    display: (u32, u32),
    fps: u32,
    cancelled: &AtomicBool,
) -> Result<Prepared, String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Preparation cancelled".into());
    }
    let child = Child(
        Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
            .arg("--prepare-cache")
            .arg(dll)
            .arg(source)
            .arg(directory)
            .arg(display.0.to_string())
            .arg(display.1.to_string())
            .arg(fps.to_string())
            .creation_flags(0x08004000)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Could not start video preparation: {e}"))?,
    );
    let text = preparation_output(child, cancelled)?;
    let result = parse_response(&text)?;
    let original = validate_media(source)?;
    let root = directory.canonicalize().map_err(|e| e.to_string())?;
    let actual = result.path.canonicalize().map_err(|e| e.to_string())?;
    if result.path != original && (!result.cached || !actual.starts_with(root)) {
        return Err("Preparation returned an unexpected file".into());
    }
    Ok(result)
}

fn preparation_output(mut child: Child, cancelled: &AtomicBool) -> Result<String, String> {
    let stdout = child
        .0
        .stdout
        .take()
        .ok_or("Preparation worker has no output")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(RESPONSE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if cancelled.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(920) {
            child.0.kill().map_err(|e| e.to_string())?;
            child.0.wait().map_err(|e| e.to_string())?;
            let _ = reader.join();
            return Err(if cancelled.load(Ordering::Relaxed) {
                "Preparation cancelled"
            } else {
                "Video preparation timed out"
            }
            .into());
        }
        std::thread::park_timeout(Duration::from_millis(50));
    };
    let bytes = reader
        .join()
        .map_err(|_| "Preparation reader failed")?
        .map_err(|e| e.to_string())?;
    if bytes.len() > RESPONSE_LIMIT as usize {
        return Err("Preparation response is too large".into());
    }
    let text = String::from_utf8(bytes).map_err(|_| "Invalid preparation response")?;
    if !status.success() {
        return Err(text
            .strip_prefix("error\n")
            .unwrap_or("Video preparation failed")
            .trim()
            .to_owned());
    }
    if cancelled.load(Ordering::Relaxed) {
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
pub fn worker(args: &[std::ffi::OsString]) -> Result<(), String> {
    if args.len() != 6 {
        return Err("Invalid preparation arguments".into());
    }
    let number = |i: usize| {
        args[i]
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or("Invalid preparation dimensions".to_owned())
    };
    let prepared = prepare_local(
        Path::new(&args[0]),
        Path::new(&args[1]),
        Path::new(&args[2]),
        (number(3)?, number(4)?),
        number(5)?,
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
pub fn prepare_local(
    dll: &Path,
    source: &Path,
    directory: &Path,
    display: (u32, u32),
    fps: u32,
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
    let info = mpv::probe_media(dll, &source)?;
    let target = target_size(info.width, info.height, display)?;
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
    let destination = directory.join(format!("wl-v2-{hash}-{}x{}-{fps}.mp4", target.0, target.1));
    if destination.is_file() {
        if !(1..=MAX_OUTPUT).contains(&fs::metadata(&destination).map_err(|e| e.to_string())?.len())
        {
            return Err("Cached video exceeds file limit".into());
        }
        let output = mpv::probe_media(dll, &destination)?;
        unchanged(&source, metadata.len(), modified)?;
        if !valid_output(&output, &info, target, fps) {
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
    let partial = directory.join(format!("partial-{}-{stamp}.mp4", std::process::id()));
    if partial.exists() || destination.exists() {
        return Err("Cache destination already exists; existing bytes were kept".into());
    }
    mpv::encode(dll, &source, &partial, target.0, target.1, fps, cancelled)?;
    if fs::metadata(&partial).map_err(|e| e.to_string())?.len() > MAX_OUTPUT {
        return Err("Prepared video exceeds 512 MiB; partial cache is not used".into());
    }
    unchanged(&source, metadata.len(), modified)?;
    let output = mpv::probe_media(dll, &partial)?;
    if !valid_output(&output, &info, target, fps) {
        return Err("Prepared video failed resolution, frame-rate, duration or audio validation; original file was kept".into());
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("Preparation cancelled".into());
    }
    fs::rename(&partial, &destination)
        .map_err(|e| format!("Could not commit video cache; previous files were kept: {e}"))?;
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
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_preparation_child_is_killed_and_reaped() {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows::Win32::{Foundation::*, System::Threading::*};
        let child = Command::new(std::env::var_os("ComSpec").unwrap())
            .args(["/D", "/Q", "/C", "set /p wallpaperlive_test_wait="])
            .creation_flags(0x08000000)
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
        let started = Instant::now();
        assert!(
            preparation_output(Child(child), &AtomicBool::new(true))
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
}
