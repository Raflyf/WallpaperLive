#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod app;
mod core;
mod desktop;
mod mpv;
mod visuals;
fn main() {
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
