#![allow(dead_code)]
#[path = "../src/core.rs"]
mod core;
#[path = "../src/desktop.rs"]
mod desktop;
#[path = "../src/mpv.rs"]
mod mpv;
use std::{path::PathBuf, time::Instant};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::*,
    },
    core::w,
};

unsafe extern "system" fn proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: diagnostic window owns no Rust state, default native processing only.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}
fn main() -> Result<(), String> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("Usage: native-smoke <local media> [seconds] [--window]")?;
    let media = core::validate_media(&PathBuf::from(path))?;
    let args: Vec<_> = std::env::args().skip(2).collect();
    let duration = args
        .first()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 1.0 && *v <= 300.0)
        .unwrap_or(8.0);
    let window_mode = args.iter().any(|a| a == "--window");
    // SAFETY: one UI thread, decoder destroyed before owning surface and window.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            lpszClassName: w!("WallpaperLive.Smoke"),
            ..Default::default()
        });
        let window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("WallpaperLive.Smoke"),
            w!("WallpaperLive native decoder test"),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            640,
            400,
            None,
            None,
            instance,
            None,
        )
        .map_err(|e| e.to_string())?;
        let surface = if window_mode {
            let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
            None
        } else {
            Some(desktop::Surface::create(
                desktop::monitors()?.remove(0).rect,
            )?)
        };
        let host = surface.as_ref().map(|s| s.hwnd).unwrap_or(window);
        let player = mpv::Mpv::open_at(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime/libmpv-2.dll"),
            host,
            window,
            core::Fit::Fill,
        )?;
        player.load(&media)?;
        if SetTimer(window, 1, 100, None) == 0 {
            return Err("Timer unavailable".into());
        }
        let start = Instant::now();
        let mut loaded = false;
        let mut paused = false;
        let mut resumed = false;
        let mut failure = None;
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            for event in player.poll() {
                match event {
                    mpv::Event::Loaded => println!("headers_loaded file={}", media.display()),
                    mpv::Event::Ready => {
                        let format = player.property(c"video-format");
                        if format.is_none() {
                            continue;
                        }
                        if let Some(surface) = &surface {
                            surface.prepare_renderer()?;
                            surface.show();
                        }
                        loaded = true;
                        println!(
                            "ready file={} hwdec={:?} vo={:?} audio={:?} format={:?}",
                            media.display(),
                            player.property(c"hwdec-current"),
                            player.property(c"current-vo"),
                            player.property(c"aid"),
                            format
                        );
                        for name in [
                            c"video-params/w",
                            c"video-params/h",
                            c"video-params/pixelformat",
                            c"video-dec-params/pixelformat",
                            c"decoder-frame-drop-count",
                            c"options/vd-lavc-threads",
                            c"options/hwdec-extra-frames",
                            c"options/swapchain-depth",
                            c"options/demuxer-max-bytes",
                            c"options/demuxer-max-back-bytes",
                            c"options/demuxer-readahead-secs",
                            c"demuxer-cache-state/fw-bytes",
                            c"mute",
                        ] {
                            println!("{}={:?}", name.to_string_lossy(), player.property(name));
                        }
                    }
                    mpv::Event::Failed(e) => failure = Some(e),
                    mpv::Event::Pause(value) => println!("pause={value}"),
                }
            }
            let elapsed = start.elapsed().as_secs_f64();
            if failure.is_some() || elapsed > duration {
                break;
            }
            if elapsed > duration * 0.5 && !paused {
                player.pause(true)?;
                paused = true;
                println!(
                    "pause_requested position={:?}",
                    player.property(c"time-pos")
                );
            }
            if elapsed > duration * 0.75 && !resumed {
                println!("paused_position={:?}", player.property(c"time-pos"));
                player.pause(false)?;
                resumed = true;
            }
        }
        println!("end loaded={loaded} desktop={}", !window_mode);
        drop(player);
        drop(surface);
        let _ = KillTimer(window, 1);
        DestroyWindow(window).map_err(|e| e.to_string())?;
        if let Some(error) = failure {
            return Err(error);
        }
        if !loaded {
            return Err("Media never loaded".into());
        }
    }
    Ok(())
}
