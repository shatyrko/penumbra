//! DDC/CI: talking to the monitor itself (brightness, contrast, volume, input, power...).
//! The real backend drives the `ddcutil` command line tool; a mock backend lets the
//! interface be developed and tested without a monitor.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub const VCP_BRIGHTNESS: u8 = 0x10;
pub const VCP_CONTRAST: u8 = 0x12;
pub const VCP_PRESET: u8 = 0x14;
pub const VCP_GAIN_R: u8 = 0x16;
pub const VCP_GAIN_G: u8 = 0x18;
pub const VCP_GAIN_B: u8 = 0x1A;
pub const VCP_INPUT: u8 = 0x60;
pub const VCP_VOLUME: u8 = 0x62;
pub const VCP_SHARPNESS: u8 = 0x87;
pub const VCP_MUTE: u8 = 0x8D;
pub const VCP_POWER: u8 = 0xD6;

/// Every feature the interface knows how to show.
pub const KNOWN_FEATURES: [u8; 11] = [
    VCP_BRIGHTNESS, VCP_CONTRAST, VCP_PRESET, VCP_GAIN_R, VCP_GAIN_G, VCP_GAIN_B, VCP_INPUT, VCP_VOLUME, VCP_SHARPNESS, VCP_MUTE, VCP_POWER,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VcpValue {
    pub current: u32,
    /// 0 for features that are lists of choices instead of ranges.
    pub max: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DdcDisplay {
    pub bus: u32,
    /// Connector without the card prefix: "HDMI-A-1". Empty when unknown.
    pub connector: String,
    pub mfg: String,
    pub model: String,
    pub serial: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DdcCaps {
    pub model: String,
    pub mccs: String,
    pub features: BTreeSet<u8>,
    /// Allowed values of choice features: input sources, colour presets, power modes.
    pub values: BTreeMap<u8, Vec<(u32, String)>>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum DdcError {
    #[error("ddcutil is not installed")]
    Missing,
    #[error("ddcutil failed: {0}")]
    Failed(String),
    #[error("timed out")]
    Timeout,
    #[error("unreadable answer: {0}")]
    Parse(String),
}

pub trait DdcBackend: Send + Sync {
    fn detect(&self) -> Result<Vec<DdcDisplay>, DdcError>;
    fn capabilities(&self, bus: u32) -> Result<DdcCaps, DdcError>;
    fn get(&self, bus: u32, codes: &[u8]) -> Result<HashMap<u8, VcpValue>, DdcError>;
    fn set(&self, bus: u32, code: u8, value: u32) -> Result<(), DdcError>;
}

// ---------------------------------------------------------------- parsing

pub fn parse_detect(text: &str) -> Vec<DdcDisplay> {
    let mut out = Vec::new();
    let mut cur: Option<DdcDisplay> = None;
    for line in text.lines() {
        let t = line.trim();
        if line.starts_with("Display ") {
            if let Some(d) = cur.take() {
                out.push(d);
            }
            cur = Some(DdcDisplay { bus: u32::MAX, connector: String::new(), mfg: String::new(), model: String::new(), serial: String::new() });
        } else if line.starts_with("Invalid display") || line.starts_with("Phantom display") || line.starts_with("Disabled display") {
            if let Some(d) = cur.take() {
                out.push(d);
            }
            cur = None;
        } else if let Some(d) = cur.as_mut() {
            if let Some(rest) = t.strip_prefix("I2C bus:") {
                d.bus = rest.trim().trim_start_matches("/dev/i2c-").parse().unwrap_or(u32::MAX);
            } else if let Some(rest) = t.strip_prefix("DRM_connector:") {
                let c = rest.trim();
                // "card1-HDMI-A-1" -> "HDMI-A-1"
                d.connector = match c.split_once('-') {
                    Some((card, conn)) if card.starts_with("card") => conn.to_string(),
                    _ => c.to_string(),
                };
            } else if let Some(rest) = t.strip_prefix("Mfg id:") {
                d.mfg = rest.trim().to_string();
            } else if let Some(rest) = t.strip_prefix("Model:") {
                d.model = rest.trim().to_string();
            } else if let Some(rest) = t.strip_prefix("Serial number:") {
                d.serial = rest.trim().to_string();
            }
        }
    }
    if let Some(d) = cur.take() {
        out.push(d);
    }
    out.retain(|d| d.bus != u32::MAX);
    out
}

pub fn parse_getvcp(text: &str) -> HashMap<u8, VcpValue> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 || f[0] != "VCP" {
            continue;
        }
        let Ok(code) = u8::from_str_radix(f[1], 16) else { continue };
        match f[2] {
            "C" if f.len() >= 5 => {
                if let (Ok(c), Ok(m)) = (f[3].parse::<u32>(), f[4].parse::<u32>()) {
                    out.insert(code, VcpValue { current: c, max: m });
                }
            }
            "SNC" | "CNC" => {
                let v = f[3].trim_start_matches('x');
                if let Ok(c) = u32::from_str_radix(v, 16) {
                    out.insert(code, VcpValue { current: c, max: 0 });
                }
            }
            _ => {}
        }
    }
    out
}

pub fn parse_capabilities(text: &str) -> DdcCaps {
    let mut caps = DdcCaps::default();
    let mut feature: Option<(u8, usize)> = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Model:") {
            caps.model = rest.trim().to_string();
            continue;
        }
        if let Some(rest) = t.strip_prefix("MCCS version:") {
            caps.mccs = rest.trim().to_string();
            continue;
        }
        if let Some(rest) = t.strip_prefix("Feature:") {
            let code = rest.split_whitespace().next().and_then(|c| u8::from_str_radix(c, 16).ok());
            feature = code.map(|c| (c, indent));
            if let Some(c) = code {
                caps.features.insert(c);
            }
            continue;
        }
        if let Some((code, f_indent)) = feature {
            if indent > f_indent {
                if let Some((hex, name)) = t.split_once(':') {
                    if hex.len() <= 4 && !hex.is_empty() {
                        if let Ok(v) = u32::from_str_radix(hex, 16) {
                            caps.values.entry(code).or_default().push((v, name.trim().to_string()));
                        }
                    }
                }
            } else if !t.is_empty() {
                feature = None;
            }
        }
    }
    caps
}

// ---------------------------------------------------------------- ddcutil

pub struct Ddcutil {
    pub sleep_multiplier: Mutex<f32>,
    pub timeout: Duration,
}

impl Default for Ddcutil {
    fn default() -> Self {
        Ddcutil { sleep_multiplier: Mutex::new(1.0), timeout: Duration::from_secs(30) }
    }
}

impl Ddcutil {
    fn run(&self, args: &[String], timeout: Duration) -> Result<String, DdcError> {
        let (ok, out, err) = self.run_full(args, timeout)?;
        if ok {
            Ok(out)
        } else {
            let msg = if err.trim().is_empty() { out } else { err };
            Err(DdcError::Failed(msg.trim().lines().last().unwrap_or("unknown error").to_string()))
        }
    }

    /// (success, stdout, stderr)
    fn run_full(&self, args: &[String], timeout: Duration) -> Result<(bool, String, String), DdcError> {
        let mut cmd = Command::new("ddcutil");
        let sm = *self.sleep_multiplier.lock().unwrap();
        if (sm - 1.0).abs() > 0.01 {
            cmd.arg(format!("--sleep-multiplier={sm}"));
        }
        let mut child = cmd
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { DdcError::Missing } else { DdcError::Failed(e.to_string()) })?;
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let t_out = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = stdout.read_to_string(&mut s);
            s
        });
        let t_err = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s);
            s
        });
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) => {
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(DdcError::Timeout);
                    }
                    std::thread::sleep(Duration::from_millis(15));
                }
                Err(e) => return Err(DdcError::Failed(e.to_string())),
            }
        };
        let out = t_out.join().unwrap_or_default();
        let err = t_err.join().unwrap_or_default();
        Ok((status.success(), out, err))
    }
}

fn s(v: &str) -> String {
    v.to_string()
}

impl DdcBackend for Ddcutil {
    fn detect(&self) -> Result<Vec<DdcDisplay>, DdcError> {
        let out = self.run(&[s("detect")], Duration::from_secs(60))?;
        Ok(parse_detect(&out))
    }

    fn capabilities(&self, bus: u32) -> Result<DdcCaps, DdcError> {
        let out = self.run(&[s("--bus"), bus.to_string(), s("capabilities")], Duration::from_secs(60))?;
        Ok(parse_capabilities(&out))
    }

    fn get(&self, bus: u32, codes: &[u8]) -> Result<HashMap<u8, VcpValue>, DdcError> {
        let mut args = vec![s("--bus"), bus.to_string(), s("--terse"), s("getvcp")];
        args.extend(codes.iter().map(|c| format!("{c:02x}")));
        // A feature the monitor lacks may make ddcutil exit with an error while it still prints
        // the others, so the output is parsed whatever the exit status.
        let (ok, out, err) = self.run_full(&args, self.timeout)?;
        let values = parse_getvcp(&out);
        if values.is_empty() && !ok {
            let msg = if err.trim().is_empty() { out } else { err };
            return Err(DdcError::Failed(msg.trim().lines().last().unwrap_or("unknown error").to_string()));
        }
        Ok(values)
    }

    fn set(&self, bus: u32, code: u8, value: u32) -> Result<(), DdcError> {
        let args = vec![s("--bus"), bus.to_string(), s("--noverify"), s("setvcp"), format!("{code:02x}"), value.to_string()];
        self.run(&args, self.timeout).map(|_| ())
    }
}

// ---------------------------------------------------------------- mock

/// Pretend monitors, for developing and testing without hardware.
pub struct Mock {
    displays: Vec<DdcDisplay>,
    state: Mutex<HashMap<(u32, u8), VcpValue>>,
    /// Artificial delay per call, like a real monitor.
    pub latency: Duration,
}

impl Mock {
    /// `connectors` are the output names the fake monitors attach to.
    pub fn new(connectors: &[String]) -> Mock {
        let mut displays = Vec::new();
        let mut state = HashMap::new();
        for (i, c) in connectors.iter().enumerate() {
            let bus = 100 + i as u32;
            displays.push(DdcDisplay {
                bus,
                connector: c.clone(),
                mfg: "MCK".into(),
                model: if i == 0 { "Mock OLED 32".into() } else { format!("Mock Display {}", i + 1) },
                serial: format!("MOCK{i:04}"),
            });
            for (code, cur, max) in [(VCP_BRIGHTNESS, 60, 100), (VCP_CONTRAST, 50, 100), (VCP_VOLUME, 30, 100), (VCP_SHARPNESS, 5, 10), (VCP_GAIN_R, 100, 100), (VCP_GAIN_G, 100, 100), (VCP_GAIN_B, 100, 100)] {
                state.insert((bus, code), VcpValue { current: cur, max });
            }
            state.insert((bus, VCP_INPUT), VcpValue { current: 0x11, max: 0 });
            state.insert((bus, VCP_MUTE), VcpValue { current: 2, max: 0 });
            state.insert((bus, VCP_POWER), VcpValue { current: 1, max: 0 });
            state.insert((bus, VCP_PRESET), VcpValue { current: 5, max: 0 });
        }
        Mock { displays, state: Mutex::new(state), latency: Duration::from_millis(120) }
    }
}

impl DdcBackend for Mock {
    fn detect(&self) -> Result<Vec<DdcDisplay>, DdcError> {
        std::thread::sleep(self.latency * 5);
        Ok(self.displays.clone())
    }

    fn capabilities(&self, _bus: u32) -> Result<DdcCaps, DdcError> {
        std::thread::sleep(self.latency * 3);
        let mut c = DdcCaps { model: "Mock".into(), mccs: "2.2".into(), ..Default::default() };
        c.features.extend(KNOWN_FEATURES);
        c.values.insert(VCP_INPUT, vec![(0x0f, "DisplayPort-1".into()), (0x10, "DisplayPort-2".into()), (0x11, "HDMI-1".into()), (0x12, "HDMI-2".into())]);
        c.values.insert(VCP_PRESET, vec![(1, "sRGB".into()), (4, "5000 K".into()), (5, "6500 K".into()), (8, "9300 K".into()), (11, "User 1".into())]);
        c.values.insert(VCP_POWER, vec![(1, "On".into()), (4, "Off (DPM)".into()), (5, "Off (hard)".into())]);
        Ok(c)
    }

    fn get(&self, bus: u32, codes: &[u8]) -> Result<HashMap<u8, VcpValue>, DdcError> {
        std::thread::sleep(self.latency);
        let st = self.state.lock().unwrap();
        Ok(codes.iter().filter_map(|c| st.get(&(bus, *c)).map(|v| (*c, *v))).collect())
    }

    fn set(&self, bus: u32, code: u8, value: u32) -> Result<(), DdcError> {
        std::thread::sleep(self.latency);
        let mut st = self.state.lock().unwrap();
        if let Some(v) = st.get_mut(&(bus, code)) {
            v.current = value;
            Ok(())
        } else {
            Err(DdcError::Failed("unsupported feature".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DETECT: &str = "\
Display 1
   I2C bus:  /dev/i2c-5
   DRM_connector:           card1-HDMI-A-1
   EDID synopsis:
      Mfg id:               HPN - HP Inc.
      Model:                OMEN 32
      Product code:         12345  (0x3039)
      Serial number:        CN1234
      Binary serial number: 1 (0x00000001)
      Manufacture year:     2024,  Week: 10
   VCP version:         2.2

Invalid display
   I2C bus:  /dev/i2c-11
   DRM_connector:           card1-eDP-1
   EDID synopsis:
      Mfg id:               BOE - BOE
      Model:                NE160QDM-NM8
   This is a laptop display.  Laptop displays do not support DDC/CI.
";

    #[test]
    fn detect_skips_invalid() {
        let d = parse_detect(DETECT);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].bus, 5);
        assert_eq!(d[0].connector, "HDMI-A-1");
        assert_eq!(d[0].model, "OMEN 32");
        assert_eq!(d[0].serial, "CN1234");
    }

    #[test]
    fn getvcp_terse() {
        let v = parse_getvcp("VCP 10 C 55 100\nVCP 60 SNC x11\nVCP 12 ERR\nVCP 8D SNC x02\n");
        assert_eq!(v[&0x10], VcpValue { current: 55, max: 100 });
        assert_eq!(v[&0x60], VcpValue { current: 0x11, max: 0 });
        assert_eq!(v[&0x8D].current, 2);
        assert!(!v.contains_key(&0x12));
    }

    #[test]
    fn capabilities_values() {
        let text = "\
Model: OMEN
MCCS version: 2.2
Commands:
   Command: 01 (VCP Request)
VCP Features:
   Feature: 02 (New control value)
   Feature: 10 (Brightness)
   Feature: 12 (Contrast)
   Feature: 60 (Input Source)
      Values:
         0f: DisplayPort-1
         11: HDMI-1
   Feature: 62 (Audio speaker volume)
   Feature: D6 (Power mode)
      Values:
         01: DPM: On,  DPD: On
         04: DPM: Off, DPD: On
";
        let c = parse_capabilities(text);
        assert_eq!(c.model, "OMEN");
        assert!(c.features.contains(&0x10) && c.features.contains(&0x62));
        assert_eq!(c.values[&0x60], vec![(0x0f, "DisplayPort-1".to_string()), (0x11, "HDMI-1".to_string())]);
        assert_eq!(c.values[&0xD6].len(), 2);
        assert!(!c.values.contains_key(&0x10));
    }

    #[test]
    fn mock_roundtrip() {
        let m = Mock::new(&["X-1".to_string()]);
        let bus = m.detect().unwrap()[0].bus;
        m.set(bus, VCP_BRIGHTNESS, 77).unwrap();
        assert_eq!(m.get(bus, &[VCP_BRIGHTNESS]).unwrap()[&VCP_BRIGHTNESS].current, 77);
    }
}
