//! Umbriel-specific helpers: which output has focus, virtual outputs. Everything else goes
//! through standard Wayland protocols and works on any wlroots-style compositor.

use std::process::Command;

pub fn available() -> bool {
    Command::new("umbriel").arg("--version").output().is_ok()
}

/// The name of the focused output, from `umbriel outputs`.
pub fn focused_output() -> Option<String> {
    let out = Command::new("umbriel").arg("outputs").output().ok()?;
    parse_focused(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_focused(text: &str) -> Option<String> {
    let mut current: Option<String> = None;
    for line in text.lines() {
        if !line.starts_with(' ') && !line.trim().is_empty() {
            current = line.split_whitespace().next().map(|s| s.to_string());
        } else if line.trim() == "Focused: yes" {
            return current;
        }
    }
    None
}

pub fn valid_virtual_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 32 && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn create_virtual(name: &str, width: i32, height: i32, hz: u32) -> Result<(), String> {
    if !valid_virtual_name(name) {
        return Err("a name may use letters, digits, - _ and . only".into());
    }
    let out = Command::new("umbriel")
        .args(["output-create", name, &format!("{width}x{height}@{hz}")])
        .output()
        .map_err(|e| format!("umbriel: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn destroy_virtual(name: &str) -> Result<(), String> {
    let out = Command::new("umbriel").args(["output-destroy", name]).output().map_err(|e| format!("umbriel: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focused_parse() {
        let t = "eDP-1 \"BOE (eDP-1)\"\n  Enabled: yes\n  Focused: no\nHDMI-A-1 \"HP\"\n  Enabled: yes\n  Focused: yes\n";
        assert_eq!(parse_focused(t).as_deref(), Some("HDMI-A-1"));
        assert_eq!(parse_focused("eDP-1 \"x\"\n  Focused: no\n"), None);
    }

    #[test]
    fn names() {
        assert!(valid_virtual_name("Stream-1"));
        assert!(!valid_virtual_name("a b"));
        assert!(!valid_virtual_name(""));
    }
}
