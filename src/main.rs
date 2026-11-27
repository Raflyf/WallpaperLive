#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod app;
mod cache;
mod clock_widget;
mod core;
mod desktop;
mod library;
mod mpv;
mod thumbnail;
mod tray;
mod update;
mod update_popup;
mod visuals;
fn harden_process() {
    use windows::Win32::System::LibraryLoader::{
        LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories, SetDllDirectoryW,
    };
    use windows::Win32::System::SystemServices::{
        PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY, PROCESS_MITIGATION_IMAGE_LOAD_POLICY,
    };
    use windows::Win32::System::Threading::{
        ProcessExtensionPointDisablePolicy, ProcessImageLoadPolicy, SetProcessMitigationPolicy,
    };
    use windows::core::w;
    // SAFETY: process-wide DLL search and mitigation hardening executed once at entry before any dynamic loads.
    unsafe {
        let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
        let _ = SetDllDirectoryW(w!(""));
        let mut ext = PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY::default();
        ext.Anonymous.Flags = 1; // DisableExtensionPoints = 1
        let _ = SetProcessMitigationPolicy(
            ProcessExtensionPointDisablePolicy,
            (&ext as *const PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY).cast(),
            std::mem::size_of::<PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY>(),
        );
        let mut img = PROCESS_MITIGATION_IMAGE_LOAD_POLICY::default();
        img.Anonymous.Flags = 0x1 | 0x2; // NoRemoteImages = 1, NoLowMandatoryLabelImages = 1
        let _ = SetProcessMitigationPolicy(
            ProcessImageLoadPolicy,
            (&img as *const PROCESS_MITIGATION_IMAGE_LOAD_POLICY).cast(),
            std::mem::size_of::<PROCESS_MITIGATION_IMAGE_LOAD_POLICY>(),
        );
    }
}

fn main() {
    harden_process();
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
    if args.first().is_some_and(|arg| arg == "--cleanup-uninstall") {
        app::cleanup_uninstall();
        std::process::exit(0);
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
