use std::path::{Path, PathBuf};

pub const FPS_OPTIONS: [u32; 4] = [24, 30, 45, 60];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Fit {
    #[default]
    Fill,
    Contain,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub path: Option<PathBuf>,
    pub monitor: String,
    pub wallpapers: Vec<(String, PathBuf)>,
    pub fit: Fit,
    pub pause_battery: bool,
    pub audio_enabled: bool,
    pub pause_fullscreen: bool,
    pub fps: u32,
    pub acrylic_blur: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            path: None,
            monitor: String::new(),
            wallpapers: Vec::new(),
            fit: Fit::Fill,
            pause_battery: true,
            audio_enabled: false,
            pause_fullscreen: true,
            fps: 30,
            acrylic_blur: true,
        }
    }
}

impl Settings {
    pub fn encode(&self) -> Result<String, String> {
        let path = self
            .path
            .as_ref()
            .map(|p| p.to_str().ok_or("File name is not valid Unicode"))
            .transpose()?
            .unwrap_or("");
        if [path, &self.monitor]
            .iter()
            .any(|s| s.chars().any(char::is_control))
        {
            return Err("Configuration contains control characters".into());
        }
        if !FPS_OPTIONS.contains(&self.fps) {
            return Err("Invalid frame-rate limit".into());
        }
        if self.wallpapers.len() > 64 {
            return Err("Too many display wallpaper assignments".into());
        }
        let mut content = format!(
            "version=4\nfit={}\nbattery={}\nmonitor={}\npath={}\naudio={}\nfullscreen={}\nfps={}\n",
            if self.fit == Fit::Fill {
                "fill"
            } else {
                "contain"
            },
            self.pause_battery,
            self.monitor,
            path,
            self.audio_enabled,
            self.pause_fullscreen,
            self.fps
        );
        let mut monitors = std::collections::HashSet::new();
        for (monitor, path) in &self.wallpapers {
            if monitor.is_empty()
                || monitor == "*"
                || monitor.chars().any(char::is_control)
                || !monitors.insert(monitor.to_lowercase())
            {
                return Err("Invalid or duplicate display wallpaper assignment".into());
            }
            validate_path_syntax(path)?;
            if !supported_extension(path) {
                return Err("Invalid display wallpaper format".into());
            }
            content.push_str(&format!(
                "display={monitor}\nwallpaper={}\n",
                path.display()
            ));
        }
        if !self.acrylic_blur {
            content.push_str("acrylic=false\n");
        }
        Ok(content)
    }
    pub fn decode(text: &str) -> Result<Self, String> {
        if text.len() > 4 * 1024 * 1024 {
            return Err("Configuration is too large".into());
        }
        let mut lines: Vec<_> = text.lines().collect();
        let mut acrylic = None;
        if lines.first() == Some(&"version=4")
            && lines
                .last()
                .is_some_and(|line| line.starts_with("acrylic="))
        {
            let line = lines.pop().unwrap_or("");
            acrylic = Some(match line {
                "acrylic=true" => true,
                "acrylic=false" => false,
                _ => return Err("Invalid acrylic blur setting".into()),
            });
        }
        let legacy = lines.len() == 5 && lines[0] == "version=1";
        let previous = lines.len() == 7 && lines[0] == "version=2";
        let current = lines.len() == 8 && lines[0] == "version=3";
        let multiple = lines.len() >= 8
            && lines[0] == "version=4"
            && (lines.len() - 8).is_multiple_of(2)
            && lines.len() <= 136;
        if !legacy && !previous && !current && !multiple {
            return Err("Invalid configuration version or contents".into());
        }
        let fps = if current || multiple {
            lines[7]
                .strip_prefix("fps=")
                .and_then(|s| s.parse::<u32>().ok())
                .filter(|f| FPS_OPTIONS.contains(f))
                .ok_or("Invalid frame-rate limit")?
        } else {
            30
        };
        let audio_enabled = if legacy {
            false
        } else {
            match lines[5] {
                "audio=true" => true,
                "audio=false" => false,
                _ => return Err("Invalid audio setting".into()),
            }
        };
        let pause_fullscreen = if legacy {
            true
        } else {
            match lines[6] {
                "fullscreen=true" => true,
                "fullscreen=false" => false,
                _ => return Err("Invalid fullscreen pause setting".into()),
            }
        };
        let fit = match lines[1] {
            "fit=fill" => Fit::Fill,
            "fit=contain" => Fit::Contain,
            _ => return Err("Invalid display fit mode".into()),
        };
        let pause_battery = match lines[2] {
            "battery=true" => true,
            "battery=false" => false,
            _ => return Err("Invalid battery pause setting".into()),
        };
        let monitor = lines[3]
            .strip_prefix("monitor=")
            .ok_or("Invalid monitor setting")?
            .to_owned();
        let path = lines[4].strip_prefix("path=").ok_or("Invalid file path")?;
        if monitor.chars().any(char::is_control) {
            return Err("Invalid monitor setting".into());
        }
        if !path.is_empty() {
            validate_path_syntax(Path::new(path))?;
        }
        let mut wallpapers = Vec::new();
        if multiple {
            for pair in lines[8..].as_chunks::<2>().0 {
                let display = pair[0]
                    .strip_prefix("display=")
                    .ok_or("Invalid display assignment")?;
                let media = pair[1]
                    .strip_prefix("wallpaper=")
                    .ok_or("Invalid display wallpaper")?;
                if display.is_empty()
                    || display == "*"
                    || display.chars().any(char::is_control)
                    || wallpapers.iter().any(|(name, _): &(String, PathBuf)| {
                        name.to_lowercase() == display.to_lowercase()
                    })
                {
                    return Err("Invalid or duplicate display wallpaper assignment".into());
                }
                let media = PathBuf::from(media);
                validate_path_syntax(&media)?;
                if !supported_extension(&media) {
                    return Err("Invalid display wallpaper format".into());
                }
                wallpapers.push((display.to_owned(), media));
            }
        }
        Ok(Self {
            path: if path.is_empty() {
                None
            } else {
                Some(PathBuf::from(path))
            },
            monitor,
            wallpapers,
            fit,
            pause_battery,
            audio_enabled,
            pause_fullscreen,
            fps,
            acrylic_blur: acrylic.unwrap_or(true),
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PauseReasons {
    pub manual: bool,
    pub locked: bool,
    pub remote: bool,
    pub disconnected: bool,
    pub display_off: bool,
    pub suspended: bool,
    pub battery: bool,
    pub fullscreen: bool,
    pub maximized: bool,
}
impl PauseReasons {
    pub fn foreground(&mut self, eligible: bool, maximized: bool, area: Rect, display: Rect) {
        self.fullscreen = eligible && area.covers(display);
        self.maximized = eligible && maximized && area.intersects(display);
    }
    pub fn session_event(&mut self, event: u32) {
        match event {
            1 => {
                self.disconnected = false;
                self.remote = false;
            }
            2 => self.disconnected = true,
            3 => {
                self.remote = true;
                self.disconnected = false;
            }
            4 => self.disconnected = true,
            6 => self.disconnected = true,
            7 => self.locked = true,
            8 => self.locked = false,
            _ => {}
        }
    }
    pub fn reason(&self) -> Option<&'static str> {
        if self.locked {
            Some("Paused: session inactive")
        } else if self.remote || self.disconnected {
            Some("Paused: remote or disconnected session")
        } else if self.suspended {
            Some("Paused: system suspended")
        } else if self.display_off {
            Some("Paused: display off")
        } else if self.manual {
            Some("Paused manually")
        } else if self.battery {
            Some("Paused: on battery")
        } else if self.maximized {
            Some("Paused: maximized application")
        } else if self.fullscreen {
            Some("Paused: fullscreen application")
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
impl Rect {
    pub fn intersects(self, other: Self) -> bool {
        self.right > self.left
            && self.bottom > self.top
            && other.right > other.left
            && other.bottom > other.top
            && self.left < other.right
            && other.left < self.right
            && self.top < other.bottom
            && other.top < self.bottom
    }
    pub fn covers(self, other: Self) -> bool {
        self.right > self.left
            && self.bottom > self.top
            && other.right > other.left
            && other.bottom > other.top
            && self.left <= other.left
            && self.top <= other.top
            && self.right >= other.right
            && self.bottom >= other.bottom
    }
}

pub fn validate_path_syntax(path: &Path) -> Result<(), String> {
    let s = path.to_str().ok_or("File name is not valid Unicode")?;
    let b = s.as_bytes();
    if b.len() < 3
        || !b[0].is_ascii_alphabetic()
        || b[1] != b':'
        || !matches!(b[2], b'\\' | b'/')
        || s[2..].contains(':')
        || s.chars().any(char::is_control)
    {
        return Err(
            "Choose a file on a local drive, not a URL, network share, or device path".into(),
        );
    }
    Ok(())
}
fn extension_lowercase(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
}
pub fn live_extension(path: &Path) -> bool {
    matches!(
        extension_lowercase(path).as_deref(),
        Some("mp4" | "gif" | "mkv" | "webm" | "mov" | "avi" | "mpg" | "mpeg" | "m4v" | "apng")
    )
}
pub fn supported_extension(path: &Path) -> bool {
    live_extension(path)
        || matches!(
            extension_lowercase(path).as_deref(),
            Some("png" | "jpg" | "jpeg" | "bmp" | "webp")
        )
}
pub fn validate_media(path: &Path) -> Result<PathBuf, String> {
    validate_path_syntax(path)?;
    if !supported_extension(path) {
        return Err("Unsupported file format".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("Could not read the file: {e}"))?;
    let s = canonical.to_str().ok_or("File name is not valid Unicode")?;
    let clean = PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(s));
    validate_path_syntax(&clean)?;
    if !clean.is_file() {
        return Err("Choose a media file, not a folder".into());
    }
    Ok(clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conservative_defaults() {
        let s = Settings::default();
        assert!(s.path.is_none());
        assert!(s.pause_battery);
        assert_eq!(s.fit, Fit::Fill);
    }

    #[test]
    fn settings_roundtrip_unicode() {
        let s = Settings {
            path: Some(PathBuf::from(r"C:\media\pemandangan senja é.mp4")),
            monitor: r"\\.\DISPLAY1".into(),
            wallpapers: Vec::new(),
            fit: Fit::Contain,
            pause_battery: false,
            audio_enabled: true,
            pause_fullscreen: false,
            fps: 45,
            acrylic_blur: false,
        };
        assert_eq!(Settings::decode(&s.encode().unwrap()).unwrap(), s);
        assert!(
            Settings::decode(
                &s.encode()
                    .unwrap()
                    .replace("acrylic=false", "acrylic=maybe")
            )
            .is_err()
        );
        let empty = Settings::default();
        assert!(empty.acrylic_blur);
        assert_eq!(Settings::decode(&empty.encode().unwrap()).unwrap(), empty);
    }

    #[test]
    fn multi_display_settings_and_pause_stay_independent() {
        let mut settings = Settings {
            monitor: "*".into(),
            wallpapers: vec![
                (r"\\.\DISPLAY1".into(), PathBuf::from(r"C:\media\senja.mp4")),
                (r"\\.\DISPLAY2".into(), PathBuf::from(r"C:\media\hujan.gif")),
            ],
            ..Default::default()
        };
        let encoded = settings.encode().unwrap();
        assert_eq!(Settings::decode(&encoded).unwrap(), settings);
        assert!(Settings::decode(&(encoded.clone() + "display=bad\n")).is_err());
        assert!(Settings::decode(&encoded.replace(r"\\.\DISPLAY2", r"\\.\DISPLAY1")).is_err());
        assert!(Settings::decode(&encoded.replace("hujan.gif", "hujan.exe")).is_err());
        settings.wallpapers[0].0.push('\n');
        assert!(settings.encode().is_err());
        let old = "version=3\nfit=fill\nbattery=true\nmonitor=\\\\.\\DISPLAY2\npath=C:\\media\\senja.mp4\naudio=false\nfullscreen=true\nfps=30\n";
        let migrated = Settings::decode(old).unwrap();
        assert_eq!(migrated.monitor, r"\\.\DISPLAY2");
        assert_eq!(migrated.path, Some(PathBuf::from(r"C:\media\senja.mp4")));
        assert!(migrated.wallpapers.is_empty());
        let first = Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let second = Rect {
            left: 0,
            top: 0,
            right: 2712,
            bottom: 1220,
        };
        let mut left = PauseReasons::default();
        let mut right = PauseReasons::default();
        left.foreground(true, true, first, first);
        right.foreground(true, true, first, second);
        assert!(left.reason().is_some() && right.reason().is_none());
        right.manual = true;
        right.foreground(false, false, first, second);
        assert_eq!(right.reason(), Some("Paused manually"));
    }

    #[test]
    fn invalid_settings_are_rejected() {
        for text in [
            "",
            "version=2\n",
            "version=1\nfit=fill\nbattery=true\nmonitor=\npath=\nfit=fill\n",
            "version=1\nfit=bad\nbattery=true\nmonitor=\npath=\n",
            "version=1\nfit=fill\nbattery=maybe\nmonitor=\npath=\n",
        ] {
            assert!(Settings::decode(text).is_err(), "accepted {text:?}");
        }
    }

    #[test]
    fn legacy_settings_keep_audio_disabled_and_fullscreen_pause_enabled() {
        let text = "version=1\nfit=contain\nbattery=false\nmonitor=\\\\.\\DISPLAY1\npath=C:\\media\\wallpaper.mp4\n";
        let settings = Settings::decode(text).unwrap();
        assert!(!settings.audio_enabled);
        assert!(settings.pause_fullscreen);
        assert_eq!(settings.fit, Fit::Contain);
        let mut changed = settings;
        changed.audio_enabled = true;
        changed.pause_fullscreen = false;
        assert_eq!(
            Settings::decode(&changed.encode().unwrap()).unwrap(),
            changed
        );
        assert!(
            Settings::decode(
                &changed
                    .encode()
                    .unwrap()
                    .replace("audio=true", "audio=maybe")
            )
            .is_err()
        );
    }

    #[test]
    fn frame_rate_preferences_migrate_and_validate() {
        for fps in FPS_OPTIONS {
            let settings = Settings {
                fps,
                ..Settings::default()
            };
            assert_eq!(
                Settings::decode(&settings.encode().unwrap()).unwrap().fps,
                fps
            );
        }
        let old =
            "version=2\nfit=fill\nbattery=true\nmonitor=\npath=\naudio=true\nfullscreen=true\n";
        assert_eq!(Settings::decode(old).unwrap().fps, 30);
        let settings = Settings {
            fps: 120,
            ..Settings::default()
        };
        assert!(settings.encode().is_err());
        assert!(
            Settings::decode(
                &Settings::default()
                    .encode()
                    .unwrap()
                    .replace("fps=30", "fps=120")
            )
            .is_err()
        );
        let display = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let maximized = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        assert!(!maximized.covers(display));
        assert!(maximized.intersects(display));
        assert!(!maximized.intersects(Rect {
            left: 1920,
            top: 0,
            right: 3840,
            bottom: 1080
        }));
        assert!(!Rect::default().intersects(display));
        let mut reasons = PauseReasons {
            maximized: true,
            ..Default::default()
        };
        assert_eq!(reasons.reason(), Some("Paused: maximized application"));
        reasons.manual = true;
        reasons.maximized = false;
        assert_eq!(reasons.reason(), Some("Paused manually"));
    }

    #[test]
    fn local_paths_only() {
        for p in [
            "https://example.org/a.mp4",
            r"\\server\share\a.mp4",
            r"\\?\C:\a.mp4",
            r"\\.\pipe\a.mp4",
            r"C:\media\a.mp4:evil",
            "C:\\a.mp4\n",
        ] {
            assert!(validate_path_syntax(Path::new(p)).is_err(), "accepted {p}");
        }
        assert!(validate_path_syntax(Path::new(r"C:\media\file with spaces.mp4")).is_ok());
    }

    #[test]
    fn finite_extensions() {
        for extension in [
            "mp4", "GIF", "mkv", "webm", "mov", "avi", "mpg", "mpeg", "m4v", "apng", "png", "jpg",
            "jpeg", "bmp", "webp",
        ] {
            assert!(
                supported_extension(Path::new(&format!("wallpaper.{extension}"))),
                "{extension}"
            );
        }
        for extension in ["exe", "html", "m3u", "m3u8", "mp3", "bat", "lnk"] {
            assert!(!supported_extension(Path::new(&format!(
                "wallpaper.{extension}"
            ))));
        }
    }

    #[test]
    fn live_extensions_exclude_static_images() {
        for extension in [
            "mp4", "MP4", "GIF", "mkv", "webm", "mov", "avi", "mpg", "mpeg", "m4v", "apng",
        ] {
            assert!(
                live_extension(Path::new(&format!("wallpaper.{extension}"))),
                "{extension}"
            );
        }
        for extension in [
            "jpg", "JPG", "jpeg", "JPEG", "png", "PNG", "bmp", "BMP", "webp", "WEBP", "exe",
        ] {
            assert!(
                !live_extension(Path::new(&format!("preview.{extension}"))),
                "{extension}"
            );
        }
    }

    #[test]
    fn fullscreen_negative_monitor_origin() {
        let monitor = Rect {
            left: -1920,
            top: -200,
            right: 0,
            bottom: 880,
        };
        assert!(monitor.covers(monitor));
        assert!(
            !Rect {
                right: -1,
                ..monitor
            }
            .covers(monitor)
        );
        assert!(!Rect::default().covers(monitor));
        assert!(!monitor.covers(Rect::default()));
    }

    #[test]
    fn manual_pause_survives_automatic_resume() {
        let mut p = PauseReasons {
            manual: true,
            fullscreen: true,
            ..Default::default()
        };
        assert!(p.reason().is_some());
        p.fullscreen = false;
        assert_eq!(p.reason(), Some("Paused manually"));
        p.manual = false;
        assert!(p.reason().is_none());
        p.battery = true;
        assert_eq!(p.reason(), Some("Paused: on battery"));
        p.locked = true;
        assert_eq!(p.reason(), Some("Paused: session inactive"));
    }

    #[test]
    fn session_unlock_does_not_clear_remote_pause() {
        let mut reasons = PauseReasons::default();
        reasons.session_event(3);
        reasons.session_event(8);
        assert!(reasons.reason().is_some());
        reasons.session_event(1);
        assert!(reasons.reason().is_none());
        reasons.session_event(7);
        reasons.session_event(10);
        assert!(reasons.reason().is_some());
        reasons.session_event(8);
        assert!(reasons.reason().is_none());
    }

    #[test]
    fn absent_media_rejected() {
        assert!(
            validate_media(Path::new(r"C:\definitely-missing-wallpaper-3f491927.mp4")).is_err()
        );
    }
}
