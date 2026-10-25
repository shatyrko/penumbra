//! `penumbra probe`: find out what a monitor's own settings are called in DDC/CI. Read-only.
//!
//! Gaming monitors (HP OMEN, ...) keep their picture modes, overdrive, black stabilizer, crosshair,
//! OLED care ... behind vendor-specific codes (0xE0-0xFF) that no standard names. The way to learn
//! one: change that setting in the monitor's own menu and see which code moved. `probe watch` does
//! that live; `probe snap` + `probe diff` do it by hand. Nothing here writes to the monitor.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::ddc::{DdcBackend, DdcDisplay, Ddcutil};

/// MCCS names of the standard codes (the ones that are not in here are unknown to us).
pub fn name(code: u8) -> &'static str {
    match code {
        0x02 => "New control value",
        0x04 => "Restore factory defaults",
        0x05 => "Restore factory brightness / contrast",
        0x08 => "Restore factory colour",
        0x0B => "Colour temperature increment",
        0x0C => "Colour temperature request",
        0x10 => "Brightness",
        0x12 => "Contrast",
        0x14 => "Colour preset",
        0x16 => "Red gain",
        0x18 => "Green gain",
        0x1A => "Blue gain",
        0x1E => "Auto setup",
        0x1F => "Auto colour setup",
        0x52 => "Active control",
        0x59 => "Saturation: red",
        0x5A => "Saturation: yellow",
        0x5B => "Saturation: green",
        0x5C => "Saturation: cyan",
        0x5D => "Saturation: blue",
        0x5E => "Saturation: magenta",
        0x60 => "Input source",
        0x62 => "Speaker volume",
        0x63 => "Speaker select",
        0x6C => "Black level: red",
        0x6E => "Black level: green",
        0x70 => "Black level: blue",
        0x72 => "Gamma",
        0x86 => "Display scaling",
        0x87 => "Sharpness",
        0x8A => "Colour saturation",
        0x8D => "Audio mute",
        0x90 => "Hue",
        0x9B => "Hue: red",
        0x9C => "Hue: yellow",
        0x9D => "Hue: green",
        0x9E => "Hue: cyan",
        0x9F => "Hue: blue",
        0xA0 => "Hue: magenta",
        0xAA => "Screen orientation",
        0xAC => "Horizontal frequency",
        0xAE => "Vertical frequency",
        0xB0 => "Settings (store / restore)",
        0xB6 => "Display technology",
        0xC0 => "Display usage time",
        0xC6 => "Application enable key",
        0xC8 => "Display controller type",
        0xC9 => "Display firmware level",
        0xCA => "OSD / button control",
        0xCC => "OSD language",
        0xD0 => "Output select",
        0xD6 => "Power mode",
        0xDC => "Display application (picture mode)",
        0xDF => "VCP version",
        0xE0..=0xFF => "vendor-specific",
        _ => "?",
    }
}

/// `C 50 100` -> "50 / 100"; `SNC x0b` -> "0x0b (11)"; anything else as ddcutil printed it.
pub fn pretty(raw: &str) -> String {
    let f: Vec<&str> = raw.split_whitespace().collect();
    match f.as_slice() {
        ["C", cur, max, ..] => format!("{cur} / {max}"),
        ["SNC", v, ..] | ["NC", v, ..] => match u32::from_str_radix(v.trim_start_matches('x'), 16) {
            Ok(n) => format!("0x{n:02x} ({n})"),
            Err(_) => raw.to_string(),
        },
        _ => raw.to_string(),
    }
}

/// Which monitor: `display` is "focused" / "external" / "all" / "" (then it must be the only one), a
/// connector (HDMI-A-1), the i2c bus number or part of the model / serial.
pub fn pick(displays: &[DdcDisplay], display: &str) -> Result<DdcDisplay, String> {
    let list = || {
        displays
            .iter()
            .map(|d| format!("  {} (bus {}, {} {})", if d.connector.is_empty() { "?" } else { &d.connector }, d.bus, d.mfg, d.model))
            .collect::<Vec<_>>()
            .join("\n")
    };
    if displays.is_empty() {
        return Err("no monitor answers DDC/CI (is it connected, DDC/CI on in its menu, and do you have access to /dev/i2c-*?)".into());
    }
    let generic = matches!(display, "" | "focused" | "external" | "all");
    if generic {
        return match displays {
            [one] => Ok(one.clone()),
            _ => Err(format!("several monitors answer; pick one with -d:\n{}", list())),
        };
    }
    let want = display.to_lowercase();
    let hits: Vec<&DdcDisplay> = displays
        .iter()
        .filter(|d| {
            d.connector.to_lowercase() == want
                || d.bus.to_string() == want
                || d.model.to_lowercase().contains(&want)
                || (!d.serial.is_empty() && d.serial.to_lowercase().contains(&want))
        })
        .collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("no monitor matches '{display}':\n{}", list())),
        _ => Err(format!("'{display}' matches several monitors:\n{}", list())),
    }
}

pub fn pick_live(display: &str) -> Result<(Ddcutil, DdcDisplay), String> {
    let ddc = Ddcutil::default();
    let displays = ddc.detect().map_err(|e| e.to_string())?;
    let d = pick(&displays, display)?;
    Ok((ddc, d))
}

fn label(d: &DdcDisplay) -> String {
    format!("{} {} ({}, bus {})", d.mfg, d.model, if d.connector.is_empty() { "?" } else { &d.connector }, d.bus)
}

fn print_table(codes: &BTreeMap<u8, String>) {
    let mut section = "";
    for (&code, raw) in codes {
        let s = match code {
            0xE0..=0xFF => "Vendor-specific (0xE0-0xFF): the gaming features live here",
            _ if name(code) == "?" => "Not in the standard list",
            _ => "Standard",
        };
        if s != section {
            println!("\n{s}");
            section = s;
        }
        println!("  0x{code:02X}  {:<38} {}", name(code), pretty(raw));
    }
}

pub fn scan(display: &str, json: bool) -> Result<(), String> {
    let (ddc, d) = pick_live(display)?;
    eprintln!("Reading every DDC code of {} (read-only, 10-60 s)...", label(&d));
    let codes = ddc.scan(d.bus).map_err(|e| e.to_string())?;
    if json {
        let m: BTreeMap<String, &String> = codes.iter().map(|(c, v)| (format!("0x{c:02X}"), v)).collect();
        println!("{}", serde_json::to_string_pretty(&m).unwrap_or_default());
    } else {
        println!("{}: {} codes answer", label(&d), codes.len());
        print_table(&codes);
    }
    Ok(())
}

// ---------------------------------------------------------------- snapshots and diffs

#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub display: String,
    pub taken: u64,
    pub codes: BTreeMap<String, String>,
}

pub fn dir() -> PathBuf {
    if let Ok(p) = std::env::var("PENUMBRA_PROBE_DIR") {
        return PathBuf::from(p);
    }
    let base = directories::BaseDirs::new();
    let root = base
        .as_ref()
        .and_then(|b| b.state_dir().map(|p| p.to_path_buf()))
        .or_else(|| base.as_ref().map(|b| b.data_local_dir().to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    root.join("penumbra").join("probe")
}

fn path_of(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return Err(format!("not a usable snapshot name: '{name}'"));
    }
    Ok(dir().join(format!("{name}.json")))
}

pub fn load(name: &str) -> Result<Snapshot, String> {
    let p = path_of(name)?;
    let text = std::fs::read_to_string(&p).map_err(|_| format!("no snapshot '{name}' (penumbra probe list)"))?;
    serde_json::from_str(&text).map_err(|e| format!("snapshot '{name}' is unreadable: {e}"))
}

pub fn snap(display: &str, name: &str) -> Result<(), String> {
    let p = path_of(name)?;
    let (ddc, d) = pick_live(display)?;
    eprintln!("Reading every DDC code of {} (read-only, 10-60 s)...", label(&d));
    let codes = ddc.scan(d.bus).map_err(|e| e.to_string())?;
    let snap = Snapshot {
        display: label(&d),
        taken: SystemTime::now().duration_since(UNIX_EPOCH).map(|t| t.as_secs()).unwrap_or(0),
        codes: codes.iter().map(|(c, v)| (format!("{c:02X}"), v.clone())).collect(),
    };
    std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    std::fs::write(&p, serde_json::to_string_pretty(&snap).unwrap_or_default()).map_err(|e| e.to_string())?;
    println!("saved '{name}': {} codes ({})", snap.codes.len(), p.display());
    Ok(())
}

pub fn list() -> Result<(), String> {
    let mut names: Vec<String> = std::fs::read_dir(dir())
        .map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).map(String::from)).collect())
        .unwrap_or_default();
    names.sort();
    if names.is_empty() {
        println!("no snapshots yet (penumbra probe snap NAME); folder: {}", dir().display());
    }
    for n in names {
        if let Ok(s) = load(&n) {
            println!("{n:<20} {} codes  {}", s.codes.len(), s.display);
        }
    }
    Ok(())
}

pub enum Change {
    Changed { code: u8, from: String, to: String },
    Appeared { code: u8, value: String },
    Gone { code: u8, value: String },
}

/// What is different between two readings, by code.
pub fn diff(a: &BTreeMap<u8, String>, b: &BTreeMap<u8, String>) -> Vec<Change> {
    let mut out = Vec::new();
    for (&code, va) in a {
        match b.get(&code) {
            Some(vb) if vb != va => out.push(Change::Changed { code, from: va.clone(), to: vb.clone() }),
            None => out.push(Change::Gone { code, value: va.clone() }),
            _ => {}
        }
    }
    for (&code, vb) in b {
        if !a.contains_key(&code) {
            out.push(Change::Appeared { code, value: vb.clone() });
        }
    }
    out.sort_by_key(|c| match c {
        Change::Changed { code, .. } | Change::Appeared { code, .. } | Change::Gone { code, .. } => *code,
    });
    out
}

pub fn describe(c: &Change) -> String {
    match c {
        Change::Changed { code, from, to } => format!("0x{code:02X}  {:<34} {}  ->  {}", name(*code), pretty(from), pretty(to)),
        Change::Appeared { code, value } => format!("0x{code:02X}  {:<34} appeared: {}", name(*code), pretty(value)),
        Change::Gone { code, value } => format!("0x{code:02X}  {:<34} gone (was {})", name(*code), pretty(value)),
    }
}

fn codes_of(s: &Snapshot) -> BTreeMap<u8, String> {
    s.codes.iter().filter_map(|(c, v)| u8::from_str_radix(c, 16).ok().map(|c| (c, v.clone()))).collect()
}

/// `diff A B`, or `diff A` against the monitor as it is now.
pub fn diff_cmd(display: &str, a: &str, b: Option<&str>) -> Result<(), String> {
    let sa = load(a)?;
    let (from, to, to_name) = match b {
        Some(b) => {
            let sb = load(b)?;
            (codes_of(&sa), codes_of(&sb), b.to_string())
        }
        None => {
            let (ddc, d) = pick_live(display)?;
            eprintln!("Reading the monitor now (read-only)...");
            (codes_of(&sa), ddc.scan(d.bus).map_err(|e| e.to_string())?, "now".to_string())
        }
    };
    let changes = diff(&from, &to);
    println!("{a} -> {to_name}: {} difference(s)", changes.len());
    for c in &changes {
        println!("  {}", describe(c));
    }
    Ok(())
}

// ---------------------------------------------------------------- watch

/// Live: change a setting in the monitor's own menu and the codes that move are printed at once.
pub fn watch(display: &str) -> Result<(), String> {
    let (ddc, d) = pick_live(display)?;
    eprintln!("Reading every DDC code of {} first (read-only, 10-60 s)...", label(&d));
    let mut last = ddc.scan(d.bus).map_err(|e| e.to_string())?;
    let codes: Vec<u8> = last.keys().copied().collect();
    println!("Watching {} codes. Now change a setting in the monitor's own menu: the codes that move are", codes.len());
    println!("printed here (each round takes a few seconds). Ctrl+C stops.\n");
    let mut full_rescan = false;
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let now = if full_rescan { ddc.scan(d.bus) } else { ddc.read_raw(d.bus, &codes) };
        let now = match now {
            Ok(n) if !n.is_empty() => n,
            Ok(_) if !full_rescan => {
                // ddcutil would not read the codes one by one: scan all of them every round instead (slower)
                full_rescan = true;
                eprintln!("(reading single codes does not work on this monitor: scanning all of them each round)");
                continue;
            }
            Ok(_) => continue,
            Err(e) => {
                eprintln!("read failed: {e}");
                continue;
            }
        };
        let stamp = clock();
        for c in diff(&last, &now) {
            println!("{stamp}  {}", describe(&c));
        }
        last = now;
    }
}

fn clock() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|t| t.as_secs()).unwrap_or(0);
    // local time without a time library: ask `date` (the probe is a developer tool)
    std::process::Command::new("date")
        .arg("+%H:%M:%S")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| format!("t+{secs}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(items: &[(u8, &str)]) -> BTreeMap<u8, String> {
        items.iter().map(|(c, v)| (*c, v.to_string())).collect()
    }

    fn display(bus: u32, connector: &str, model: &str, serial: &str) -> DdcDisplay {
        DdcDisplay { bus, connector: connector.into(), mfg: "HPN".into(), model: model.into(), serial: serial.into() }
    }

    #[test]
    fn diff_finds_changed_new_and_gone() {
        let a = map(&[(0x10, "C 50 100"), (0xE7, "SNC x01"), (0xF1, "SNC x00")]);
        let b = map(&[(0x10, "C 50 100"), (0xE7, "SNC x03"), (0xE8, "SNC x00")]);
        let d = diff(&a, &b);
        assert_eq!(d.len(), 3);
        assert!(matches!(&d[0], Change::Changed { code: 0xE7, from, to } if from == "SNC x01" && to == "SNC x03"));
        assert!(matches!(&d[1], Change::Appeared { code: 0xE8, .. }));
        assert!(matches!(&d[2], Change::Gone { code: 0xF1, .. }));
    }

    #[test]
    fn same_readings_have_no_difference() {
        let a = map(&[(0x10, "C 50 100")]);
        assert!(diff(&a, &a.clone()).is_empty());
    }

    #[test]
    fn values_are_shown_readably() {
        assert_eq!(pretty("C 50 100"), "50 / 100");
        assert_eq!(pretty("SNC x0b"), "0x0b (11)");
        assert_eq!(pretty("CNC x01 x02 x03 x04"), "CNC x01 x02 x03 x04");
    }

    #[test]
    fn names_cover_standard_and_vendor_codes() {
        assert_eq!(name(0x10), "Brightness");
        assert_eq!(name(0xE7), "vendor-specific");
        assert_eq!(name(0x33), "?");
    }

    #[test]
    fn picks_the_only_monitor_or_asks() {
        let one = vec![display(5, "HDMI-A-1", "OMEN 32 OLED", "CN44441MK5")];
        assert_eq!(pick(&one, "focused").unwrap().bus, 5);
        let two = vec![one[0].clone(), display(7, "DP-3", "Other", "X1")];
        assert!(pick(&two, "focused").unwrap_err().contains("pick one with -d"));
        assert_eq!(pick(&two, "dp-3").unwrap().bus, 7);
        assert_eq!(pick(&two, "omen").unwrap().bus, 5);
        assert_eq!(pick(&two, "7").unwrap().connector, "DP-3");
        assert!(pick(&two, "nope").is_err());
        assert!(pick(&[], "focused").is_err());
    }

    #[test]
    fn snapshot_names_cannot_escape_the_folder() {
        assert!(path_of("../x").is_err());
        assert!(path_of(".hidden").is_err());
        assert!(path_of("").is_err());
        assert!(path_of("before-hdr").is_ok());
    }
}
