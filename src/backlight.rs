//! The built-in panel: Linux backlight in sysfs, written through systemd-logind when the
//! file itself is not writable by the user.

use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Backlight {
    pub name: String,
    pub dir: PathBuf,
    pub max: u32,
}

impl Backlight {
    /// The first usable backlight, preferring firmware/platform over raw so that the range matches
    /// what the keyboard keys use.
    pub fn find() -> Option<Backlight> {
        Backlight::find_in("/sys/class/backlight")
    }

    pub fn find_in(root: &str) -> Option<Backlight> {
        let mut found: Vec<(u8, Backlight)> = Vec::new();
        for entry in fs::read_dir(root).ok()?.flatten() {
            let dir = entry.path();
            let max: u32 = fs::read_to_string(dir.join("max_brightness")).ok()?.trim().parse().ok()?;
            if max == 0 {
                continue;
            }
            let kind = fs::read_to_string(dir.join("type")).unwrap_or_default();
            let rank = match kind.trim() {
                "firmware" => 0,
                "platform" => 1,
                _ => 2,
            };
            found.push((rank, Backlight { name: entry.file_name().to_string_lossy().into_owned(), dir, max }));
        }
        found.sort_by_key(|(r, b)| (*r, b.name.clone()));
        found.into_iter().next().map(|(_, b)| b)
    }

    pub fn read(&self) -> Option<u32> {
        fs::read_to_string(self.dir.join("brightness")).ok()?.trim().parse().ok()
    }

    /// 0.0 - 1.0
    pub fn fraction(&self) -> Option<f64> {
        self.read().map(|v| v as f64 / self.max as f64)
    }

    pub fn set_fraction(&self, f: f64) -> Result<(), String> {
        let raw = ((f.clamp(0.0, 1.0) * self.max as f64).round() as u32).max(1);
        self.set_raw(raw)
    }

    pub fn set_raw(&self, raw: u32) -> Result<(), String> {
        if fs::write(self.dir.join("brightness"), raw.to_string()).is_ok() {
            return Ok(());
        }
        let conn = zbus::blocking::Connection::system().map_err(|e| e.to_string())?;
        conn.call_method(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1/session/auto",
            Some("org.freedesktop.login1.Session"),
            "SetBrightness",
            &("backlight", self.name.as_str(), raw),
        )
        .map(|_| ())
        .map_err(|e| format!("cannot set the backlight: {e}"))
    }
}
