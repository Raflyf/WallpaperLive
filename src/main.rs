#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod app;
mod cache;
mod core;
mod desktop;
mod library;
mod mpv;
mod online;
mod thumbnail;
mod tray;
mod update;
mod visuals;
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--thumbnail") {
        use std::io::Write;
        let result = args
            .get(1)
            .ok_or_else(|| "Thumbnail media is missing".to_string())
            .and_then(|path| thumbnail::extract(std::path::Path::new(path)))
            .and_then(|bytes| {
                std::io::stdout()
                    .lock()
                    .write_all(&bytes)
                    .map_err(|e| e.to_string())
            });
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    if args.first().is_some_and(|arg| arg == "--prepare-cache") {
        use std::io::Write;
        match cache::worker(&args[1..]) {
            Ok(()) => std::process::exit(0),
            Err(error) => {
                let _ = writeln!(
                    std::io::stdout().lock(),
                    "error\n{}",
                    error.replace(['\r', '\n'], " ")
                );
                std::process::exit(1);
            }
        }
    }
    if let Err(error) = app::run() {
        let message: Vec<u16> = error.encode_utf16().chain(Some(0)).collect();
        // SAFETY: NUL-terminated temporary string lives until native message box returns.
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                None,
                windows::core::PCWSTR(message.as_ptr()),
                windows::core::w!("WallpaperLive"),
                windows::Win32::UI::WindowsAndMessaging::MB_OK
                    | windows::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
            );
        }
        std::process::exit(1);
    }
}
