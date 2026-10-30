//! Lights built into monitors (and other devices) through the standard HID LampArray protocol
//! (HID usage page 0x59, "Lighting and Illumination"; what Windows calls Dynamic Lighting).
//! Example: the HP OMEN 32 OLED's rear light, 104 lamps in a ring.
//!
//! A LampArray runs its own effect ("autonomous mode") until the computer takes over; then every
//! lamp shows what we send until we hand it back. The monitor's own light switch stays the master:
//! when the light is off in its menu, nothing we send shows.

use std::fs::{self, File, OpenOptions};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};

use rustix::ioctl::{opcode, Direction, Ioctl, IoctlOutput, Opcode};
use serde::{Deserialize, Serialize};

// ------------------------------------------------------------------ settings and looks

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LightMode {
    /// The device's own effect (we leave it alone).
    #[default]
    Own,
    /// Every lamp dark.
    Off,
    /// Our colours.
    On,
}

/// What is remembered for one light.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LightSetting {
    pub mode: LightMode,
    /// A key of [`LOOKS`]; "custom" uses `colour`.
    pub look: String,
    pub colour: [u8; 3],
    /// 0.0 - 1.0
    pub brightness: f64,
}

impl Default for LightSetting {
    fn default() -> Self {
        LightSetting { mode: LightMode::Own, look: "white".into(), colour: [255, 255, 255], brightness: 1.0 }
    }
}

/// The looks offered: key, label, colour stops going round the ring (one stop = one colour).
pub const LOOKS: &[(&str, &str, &[[u8; 3]])] = &[
    ("white", "White", &[[255, 255, 255]]),
    ("warm", "Warm white", &[[255, 170, 90]]),
    ("purple", "Purple", &[[170, 60, 255]]),
    ("blue", "Blue", &[[30, 90, 255]]),
    ("red", "Red", &[[255, 20, 10]]),
    ("sunset", "Sunset", &[[200, 120, 255], [255, 80, 150], [255, 140, 40]]),
    ("ocean", "Ocean", &[[0, 200, 200], [20, 80, 255], [120, 40, 255]]),
    ("aurora", "Aurora", &[[40, 255, 120], [0, 200, 255], [160, 60, 255]]),
    ("custom", "Custom colour", &[]),
];

pub fn look_label(key: &str) -> &'static str {
    LOOKS.iter().find(|l| l.0 == key).map(|l| l.1).unwrap_or("Custom colour")
}

/// "#ff8800", "ff8800", "#f80" → RGB.
pub fn parse_colour(text: &str) -> Option<[u8; 3]> {
    let h = text.trim().trim_start_matches('#');
    let h = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 => h.to_string(),
        _ => return None,
    };
    let v = u32::from_str_radix(&h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// The colour of every lamp for a setting. `lamps`: positions in mm, as seen from the front.
pub fn render(s: &LightSetting, lamps: &[(f32, f32)]) -> Vec<[u8; 3]> {
    if s.mode != LightMode::On {
        return vec![[0, 0, 0]; lamps.len()];
    }
    // LED light looks far brighter than its numbers: a square makes the slider feel even.
    let k = s.brightness.clamp(0.0, 1.0).powi(2);
    let scale = |c: [u8; 3]| c.map(|x| (x as f64 * k).round() as u8);
    let stops: Vec<[u8; 3]> = match LOOKS.iter().find(|l| l.0 == s.look) {
        Some((_, _, stops)) if !stops.is_empty() => stops.to_vec(),
        _ => vec![s.colour],
    };
    if stops.len() == 1 {
        return vec![scale(stops[0]); lamps.len()];
    }
    // Gradient round the ring: the angle of each lamp around the centre, from the top, clockwise.
    let n = lamps.len().max(1) as f32;
    let cx = lamps.iter().map(|p| p.0).sum::<f32>() / n;
    let cy = lamps.iter().map(|p| p.1).sum::<f32>() / n;
    lamps
        .iter()
        .map(|&(x, y)| {
            let a = (x - cx).atan2(cy - y); // 0 at the top, growing clockwise
            let t = (a / std::f32::consts::TAU).rem_euclid(1.0) * stops.len() as f32;
            let i = t.floor() as usize % stops.len();
            let j = (i + 1) % stops.len();
            let f = t - t.floor();
            let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * f).round() as u8;
            scale([mix(stops[i][0], stops[j][0]), mix(stops[i][1], stops[j][1]), mix(stops[i][2], stops[j][2])])
        })
        .collect()
}

// ------------------------------------------------------------------ the report descriptor

/// Where the LampArray reports are, read from the device's HID report descriptor (the report
/// numbers are the vendor's choice).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reports {
    pub attributes: u8,
    pub lamp_request: u8,
    pub lamp_response: u8,
    pub multi_update: u8,
    pub range_update: u8,
    pub control: u8,
    /// Lamps per multi-update report.
    pub per_update: usize,
}

const U_ATTRIBUTES: u32 = 0x02;
const U_LAMP_REQUEST: u32 = 0x20;
const U_LAMP_RESPONSE: u32 = 0x22;
const U_MULTI_UPDATE: u32 = 0x50;
const U_RANGE_UPDATE: u32 = 0x60;
const U_CONTROL: u32 = 0x70;

/// None when the descriptor is not a LampArray or lacks a report we need.
pub fn parse_descriptor(d: &[u8]) -> Option<Reports> {
    let mut i = 0;
    let (mut page, mut usage, mut report_id, mut size, mut count) = (0u32, 0u32, 0u8, 0u32, 0u32);
    let mut lamp_array = false;
    let mut coll: Vec<u32> = Vec::new();
    let mut r = Reports::default();
    while i < d.len() {
        let b = d[i];
        if b == 0xFE {
            // long item: skip
            let len = *d.get(i + 1)? as usize;
            i += 3 + len;
            continue;
        }
        let n = [0, 1, 2, 4][(b & 3) as usize];
        let data = d.get(i + 1..i + 1 + n)?;
        let v = data.iter().rev().fold(0u32, |a, x| (a << 8) | *x as u32);
        i += 1 + n;
        match b & 0xFC {
            0x04 => page = v,
            0x08 => usage = v,
            0x84 => report_id = v as u8,
            0x74 => size = v,
            0x94 => count = v,
            0xA0 => {
                if page == 0x59 && usage == 0x01 && coll.is_empty() {
                    lamp_array = true;
                }
                if page == 0x59 {
                    let slot = match usage {
                        U_ATTRIBUTES => Some(&mut r.attributes),
                        U_LAMP_REQUEST => Some(&mut r.lamp_request),
                        U_LAMP_RESPONSE => Some(&mut r.lamp_response),
                        U_MULTI_UPDATE => Some(&mut r.multi_update),
                        U_RANGE_UPDATE => Some(&mut r.range_update),
                        U_CONTROL => Some(&mut r.control),
                        _ => None,
                    };
                    if let Some(s) = slot {
                        *s = report_id;
                    }
                }
                coll.push(usage);
            }
            0xC0 => {
                coll.pop();
            }
            0xB0 if coll.last() == Some(&U_MULTI_UPDATE) && size == 16 => r.per_update = count as usize,
            _ => {}
        }
    }
    let have = [r.attributes, r.lamp_request, r.lamp_response, r.range_update, r.control].iter().all(|x| *x != 0);
    (lamp_array && have).then_some(r)
}

// ------------------------------------------------------------------ reports on the wire

pub fn attributes_report(id: u8) -> Vec<u8> {
    let mut b = vec![0u8; 23];
    b[0] = id;
    b
}

/// LampArrayKind values that are not a display's light: keyboards, mice, game controllers,
/// wearables. Penumbra leaves those to their own tools.
pub const NOT_DISPLAY_KINDS: &[u32] = &[1, 2, 3, 8];

/// (lamp count, bounding box in mm, kind, minimum update interval in ms)
pub fn parse_attributes(b: &[u8]) -> Option<(usize, (f32, f32, f32), u32, f32)> {
    let u16_ = |o: usize| Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?));
    let u32_ = |o: usize| Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?));
    Some((u16_(1)? as usize, (u32_(3)? as f32 / 1000.0, u32_(7)? as f32 / 1000.0, u32_(11)? as f32 / 1000.0), u32_(15)?, u32_(19)? as f32 / 1000.0))
}

/// (lamp id, x mm, y mm) from a LampAttributesResponse.
pub fn parse_lamp(b: &[u8]) -> Option<(u16, f32, f32)> {
    let id = u16::from_le_bytes(b.get(1..3)?.try_into().ok()?);
    let x = u32::from_le_bytes(b.get(3..7)?.try_into().ok()?);
    let y = u32::from_le_bytes(b.get(7..11)?.try_into().ok()?);
    Some((id, x as f32 / 1000.0, y as f32 / 1000.0))
}

pub fn range_report(id: u8, start: u16, end: u16, c: [u8; 3], complete: bool) -> Vec<u8> {
    let mut b = vec![id, complete as u8];
    b.extend(start.to_le_bytes());
    b.extend(end.to_le_bytes());
    b.extend([c[0], c[1], c[2], 255]);
    b
}

/// Multi-update reports for a whole frame; the last one says the frame is complete.
pub fn frame_reports(r: &Reports, colours: &[[u8; 3]]) -> Vec<Vec<u8>> {
    if colours.is_empty() {
        return Vec::new();
    }
    // All the same colour (or no multi-update report): one range update does it.
    if r.multi_update == 0 || r.per_update == 0 || colours.iter().all(|c| *c == colours[0]) {
        return vec![range_report(r.range_update, 0, colours.len() as u16 - 1, colours[0], true)];
    }
    let per = r.per_update;
    let chunks: Vec<_> = colours.chunks(per).enumerate().collect();
    let last = chunks.len() - 1;
    chunks
        .into_iter()
        .map(|(ci, chunk)| {
            let mut b = vec![r.multi_update, chunk.len() as u8, (ci == last) as u8];
            for k in 0..per {
                let id = if k < chunk.len() { (ci * per + k) as u16 } else { 0 };
                b.extend(id.to_le_bytes());
            }
            for k in 0..per {
                let c = chunk.get(k).copied().unwrap_or([0, 0, 0]);
                b.extend([c[0], c[1], c[2], 255]);
            }
            b
        })
        .collect()
}

// ------------------------------------------------------------------ devices

/// HIDIOCSFEATURE / HIDIOCGFEATURE: the size is part of the request number.
struct Feature<'a> {
    op: Opcode,
    buf: &'a mut [u8],
}

unsafe impl Ioctl for Feature<'_> {
    type Output = IoctlOutput;
    const IS_MUTATING: bool = true;
    fn opcode(&self) -> Opcode {
        self.op
    }
    fn as_ptr(&mut self) -> *mut core::ffi::c_void {
        self.buf.as_mut_ptr().cast()
    }
    unsafe fn output_from_ptr(out: IoctlOutput, _: *mut core::ffi::c_void) -> rustix::io::Result<IoctlOutput> {
        Ok(out)
    }
}

fn set_feature(f: &File, data: &[u8]) -> Result<(), String> {
    let mut buf = data.to_vec();
    let op = opcode::from_components(Direction::ReadWrite, b'H', 0x06, buf.len());
    unsafe { rustix::ioctl::ioctl(f.as_fd(), Feature { op, buf: &mut buf }) }.map(|_| ()).map_err(|e| e.to_string())
}

fn get_feature(f: &File, mut buf: Vec<u8>) -> Result<Vec<u8>, String> {
    let op = opcode::from_components(Direction::ReadWrite, b'H', 0x07, buf.len());
    unsafe { rustix::ioctl::ioctl(f.as_fd(), Feature { op, buf: &mut buf }) }.map_err(|e| e.to_string())?;
    Ok(buf)
}

/// A LampArray found in /sys/class/hidraw, not opened yet.
#[derive(Clone, Debug)]
pub struct Found {
    pub dev: PathBuf,
    /// Stable identity: vendor:product + serial (or the USB path when there is none).
    pub id: String,
    pub name: String,
    pub serial: String,
    pub reports: Reports,
}

/// Whether a light belongs to a display: the same serial number, or the display's model in the
/// light's name ("ENE OMEN 32 OLED External Light Device" and model "OMEN 32 OLED").
pub fn belongs_to(f: &Found, model: &str, serial: &str) -> bool {
    if !f.serial.is_empty() && !serial.is_empty() {
        return f.serial.eq_ignore_ascii_case(serial);
    }
    let m = model.trim().to_lowercase();
    m.len() >= 4 && f.name.to_lowercase().contains(&m)
}

pub fn scan() -> Vec<Found> {
    scan_in(Path::new("/sys/class/hidraw"))
}

fn scan_in(root: &Path) -> Vec<Found> {
    let mut out = Vec::new();
    let Ok(dir) = fs::read_dir(root) else { return out };
    for e in dir.flatten() {
        let p = e.path();
        let Ok(desc) = fs::read(p.join("device/report_descriptor")) else { continue };
        let Some(reports) = parse_descriptor(&desc) else { continue };
        let uevent = fs::read_to_string(p.join("device/uevent")).unwrap_or_default();
        let field = |k: &str| uevent.lines().find_map(|l| l.strip_prefix(k)).unwrap_or("").to_string();
        let hid = field("HID_ID=");
        let serial = field("HID_UNIQ=");
        let name = field("HID_NAME=");
        let ids: Vec<&str> = hid.split(':').collect();
        let vp = if ids.len() == 3 { format!("{}:{}", &ids[1][ids[1].len().saturating_sub(4)..], &ids[2][ids[2].len().saturating_sub(4)..]) } else { hid.clone() };
        let id = if serial.is_empty() { format!("{vp}|{}", field("HID_PHYS=")) } else { format!("{vp}|{serial}") }.to_lowercase();
        out.push(Found { dev: PathBuf::from("/dev").join(e.file_name()), id, name, serial, reports });
    }
    out.sort_by(|a, b| a.dev.cmp(&b.dev));
    out
}

/// An opened LampArray.
pub struct LampArray {
    pub found: Found,
    file: File,
    /// Lamp positions in mm, as seen from the front, by lamp id.
    pub lamps: Vec<(f32, f32)>,
    /// What was last sent, so the same frame is not sent again for nothing.
    last: Option<Vec<[u8; 3]>>,
    pub host_control: bool,
}

impl LampArray {
    pub fn open(found: Found) -> Result<LampArray, String> {
        let file = OpenOptions::new().read(true).write(true).open(&found.dev).map_err(|e| format!("{}: {e}", found.dev.display()))?;
        let r = &found.reports;
        let attrs = get_feature(&file, attributes_report(r.attributes))?;
        let (count, _, kind, _) = parse_attributes(&attrs).ok_or("short attributes report")?;
        if NOT_DISPLAY_KINDS.contains(&kind) {
            return Err(format!("{}: not a display light (LampArray kind {kind})", found.name));
        }
        let count = count.min(4096);
        // Ask for lamp 0; every response moves on to the next lamp.
        let mut req = vec![r.lamp_request];
        req.extend(0u16.to_le_bytes());
        set_feature(&file, &req)?;
        let mut lamps = vec![(0.0, 0.0); count];
        for _ in 0..count {
            let mut b = vec![0u8; 29];
            b[0] = r.lamp_response;
            let b = get_feature(&file, b)?;
            if let Some((id, x, y)) = parse_lamp(&b) {
                if let Some(slot) = lamps.get_mut(id as usize) {
                    *slot = (x, y);
                }
            }
        }
        Ok(LampArray { found, file, lamps, last: None, host_control: false })
    }

    /// Show `colours` (one per lamp). Takes control from the device's own effect first.
    pub fn show(&mut self, colours: &[[u8; 3]], resend: bool) -> Result<(), String> {
        if !self.host_control {
            set_feature(&self.file, &[self.found.reports.control, 0])?;
            self.host_control = true;
            self.last = None;
        }
        if !resend && self.last.as_deref() == Some(colours) {
            return Ok(());
        }
        for rep in frame_reports(&self.found.reports, colours) {
            set_feature(&self.file, &rep)?;
        }
        self.last = Some(colours.to_vec());
        Ok(())
    }

    /// Back to the device's own effect.
    pub fn release(&mut self) -> Result<(), String> {
        set_feature(&self.file, &[self.found.reports.control, 1])?;
        self.host_control = false;
        self.last = None;
        Ok(())
    }

    pub fn apply(&mut self, s: &LightSetting, resend: bool) -> Result<(), String> {
        match s.mode {
            LightMode::Own => {
                if self.host_control {
                    self.release()?;
                }
                Ok(())
            }
            _ => {
                let colours = render(s, &self.lamps);
                self.show(&colours, resend)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rear light of an HP OMEN 32 OLED (ENE controller), interface 2.
    const OMEN: &str = "05590901a10185010902a1020903150027ffff000075109501b10309040905090609070908150027ffffff7f75209505b103c085020920a1020921150027ffff000075109501b102c085030922a1020921150027ffff000075109501b10209230924092509270926150027ffffff7f75209505b10209280929092a092b092c092d150026ff0075089506b102c085040950a102090309551500250875089502b1020921150027ffff000075109508b10209510952095309540951095209530954095109520953095409510952095309540951095209530954095109520953095409510952095309540951095209530954150026ff0075089520b102c085050960a10209551500250875089501b10209610962150027ffff000075109502b1020951095209530954150026ff0075089504b102c085060970a10209711500250175089501b102c0850709a1a1010910150026ff007508954081020911150026ff0075089540910209049506b102c0c0";
    /// Its other interface: a vendor page, not a LampArray.
    const VENDOR: &str = "0672ff09a1a1010910150026ff007508954081020911150026ff0075089540910209049540b102c0";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn descriptor() {
        let r = parse_descriptor(&hex(OMEN)).unwrap();
        assert_eq!(r, Reports { attributes: 1, lamp_request: 2, lamp_response: 3, multi_update: 4, range_update: 5, control: 6, per_update: 8 });
        assert!(parse_descriptor(&hex(VENDOR)).is_none());
        assert!(parse_descriptor(&[]).is_none());
        assert!(parse_descriptor(&[0x05]).is_none());
    }

    #[test]
    fn attribute_reports() {
        // 104 lamps, 719 x 423 x 74 mm, kind 5, 20 ms
        let b = hex("016800def80a004a7306000423010005000000204e0000");
        let (n, bx, kind, ms) = parse_attributes(&b).unwrap();
        assert_eq!((n, kind), (104, 5));
        assert_eq!((bx.0.round(), bx.1.round()), (719.0, 423.0));
        assert_eq!(ms, 20.0);
        let mut l = vec![3u8];
        l.extend(7u16.to_le_bytes());
        l.extend(188_500u32.to_le_bytes());
        l.extend(263_000u32.to_le_bytes());
        assert_eq!(parse_lamp(&l), Some((7, 188.5, 263.0)));
    }

    fn ring() -> Vec<(f32, f32)> {
        (0..16).map(|i| {
            let a = i as f32 / 16.0 * std::f32::consts::TAU;
            (100.0 + 50.0 * a.sin(), 100.0 - 50.0 * a.cos())
        }).collect()
    }

    #[test]
    fn rendering() {
        let lamps = ring();
        let mut s = LightSetting { mode: LightMode::On, ..Default::default() };
        assert!(render(&s, &lamps).iter().all(|c| *c == [255, 255, 255]));
        s.brightness = 0.5;
        assert!(render(&s, &lamps).iter().all(|c| *c == [64, 64, 64]));
        s.mode = LightMode::Off;
        assert!(render(&s, &lamps).iter().all(|c| *c == [0, 0, 0]));
        s = LightSetting { mode: LightMode::On, look: "custom".into(), colour: [10, 20, 30], brightness: 1.0 };
        assert_eq!(render(&s, &lamps)[3], [10, 20, 30]);
        // gradient: the top lamp gets the first stop, the colours change round the ring
        s.look = "sunset".into();
        let g = render(&s, &lamps);
        assert_eq!(g[0], [200, 120, 255]);
        assert_ne!(g[0], g[8]);
    }

    #[test]
    fn frames() {
        let r = parse_descriptor(&hex(OMEN)).unwrap();
        let same = frame_reports(&r, &vec![[1, 2, 3]; 104]);
        assert_eq!(same, vec![vec![5, 1, 0, 0, 103, 0, 1, 2, 3, 255]]);
        let mut cols = vec![[0, 0, 0]; 20];
        cols[9] = [9, 9, 9];
        let f = frame_reports(&r, &cols);
        assert_eq!(f.len(), 3);
        assert!(f.iter().all(|b| b.len() == 3 + 8 * 2 + 8 * 4));
        assert_eq!((f[0][1], f[0][2]), (8, 0));
        assert_eq!((f[2][1], f[2][2]), (4, 1));
        // lamp 9 is the second id of the second report
        assert_eq!(u16::from_le_bytes([f[1][5], f[1][6]]), 9);
        assert_eq!(&f[1][3 + 16 + 4..3 + 16 + 8], &[9, 9, 9, 255]);
    }

    #[test]
    fn belonging() {
        let r = parse_descriptor(&hex(OMEN)).unwrap();
        let f = |name: &str, serial: &str| Found { dev: "/dev/x".into(), id: "x".into(), name: name.into(), serial: serial.into(), reports: r.clone() };
        let omen = f("ENE OMEN 32 OLED External Light Device", "AB123");
        assert!(belongs_to(&omen, "OMEN 32 OLED", "AB123"));
        assert!(!belongs_to(&omen, "OMEN 32 OLED", "ZZ999"));
        assert!(belongs_to(&f("ENE OMEN 32 OLED External Light Device", ""), "OMEN 32 OLED", "AB123"));
        // the laptop keyboard: no serial, no display model in its name
        assert!(!belongs_to(&f("ASUSTek Computer Inc. N-KEY Device", ""), "NE160QDM-NM8", ""));
        assert!(!belongs_to(&f("Some Light", ""), "", ""));
    }

    #[test]
    fn colours() {
        assert_eq!(parse_colour("#ff8800"), Some([255, 136, 0]));
        assert_eq!(parse_colour("f80"), Some([255, 136, 0]));
        assert_eq!(parse_colour("nope"), None);
    }
}
