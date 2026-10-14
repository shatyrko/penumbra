//! Start with the session, and a launcher entry, through plain XDG files.

use std::path::PathBuf;

fn exe() -> String {
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "penumbra".into())
}

fn entry(autostart: bool) -> String {
    let mut t = format!(
        "[Desktop Entry]\nType=Application\nName=Penumbra\nGenericName=Display control\nComment=Brightness, DDC/CI, resolution and scaling for your displays\nExec={}\nIcon=penumbra\nTerminal=false\nCategories=Settings;HardwareSettings;\nKeywords=monitor;display;brightness;ddc;resolution;\nStartupNotify=false\n",
        exe()
    );
    if autostart {
        t.push_str("X-GNOME-Autostart-enabled=true\n");
    }
    t
}

fn autostart_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.config_dir().join("autostart").join("penumbra.desktop"))
}

fn launcher_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.data_dir().join("applications").join("penumbra.desktop"))
}

pub fn is_enabled() -> bool {
    autostart_path().map(|p| p.exists()).unwrap_or(false)
}

pub fn set(on: bool) -> Result<PathBuf, String> {
    let p = autostart_path().ok_or("no config directory")?;
    if on {
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&p, entry(true)).map_err(|e| e.to_string())?;
    } else {
        let _ = std::fs::remove_file(&p);
    }
    Ok(p)
}

/// Launcher entry and icon in the user's data directory.
pub fn install_launcher() -> Result<PathBuf, String> {
    let p = launcher_path().ok_or("no data directory")?;
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&p, entry(false)).map_err(|e| e.to_string())?;
    if let Some(d) = directories::BaseDirs::new() {
        let dir = d.data_dir().join("icons/hicolor/scalable/apps");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("penumbra.svg"), include_str!("../data/penumbra.svg")).map_err(|e| e.to_string())?;
    }
    Ok(p)
}
