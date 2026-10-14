//! Plain data describing displays, shared by every part of the program.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mode {
    pub width: i32,
    pub height: i32,
    /// Refresh rate in millihertz, as the compositor reports it.
    pub refresh_mhz: i32,
    pub preferred: bool,
}

impl Mode {
    pub fn hz(&self) -> f64 {
        self.refresh_mhz as f64 / 1000.0
    }

    pub fn same_size(&self, other: &Mode) -> bool {
        self.width == other.width && self.height == other.height
    }

    /// "165 Hz", "59.94 Hz": trailing zeros dropped.
    pub fn hz_label(&self) -> String {
        let hz = self.hz();
        if (hz - hz.round()).abs() < 0.005 {
            format!("{} Hz", hz.round() as i64)
        } else {
            format!("{:.2} Hz", hz)
        }
    }

    pub fn label(&self) -> String {
        format!("{}×{} @ {}", self.width, self.height, self.hz_label())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transform {
    Normal,
    R90,
    R180,
    R270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl Transform {
    pub const ROTATIONS: [Transform; 4] = [Transform::Normal, Transform::R90, Transform::R180, Transform::R270];

    pub fn degrees(self) -> i32 {
        match self {
            Transform::Normal | Transform::Flipped => 0,
            Transform::R90 | Transform::Flipped90 => 90,
            Transform::R180 | Transform::Flipped180 => 180,
            Transform::R270 | Transform::Flipped270 => 270,
        }
    }

    pub fn flipped(self) -> bool {
        matches!(self, Transform::Flipped | Transform::Flipped90 | Transform::Flipped180 | Transform::Flipped270)
    }

    pub fn from_parts(degrees: i32, flipped: bool) -> Transform {
        match (degrees.rem_euclid(360), flipped) {
            (90, false) => Transform::R90,
            (180, false) => Transform::R180,
            (270, false) => Transform::R270,
            (0, true) => Transform::Flipped,
            (90, true) => Transform::Flipped90,
            (180, true) => Transform::Flipped180,
            (270, true) => Transform::Flipped270,
            _ => Transform::Normal,
        }
    }

    pub fn swaps_axes(self) -> bool {
        matches!(self.degrees(), 90 | 270)
    }

    pub fn label(self) -> String {
        let r = match self.degrees() {
            0 => "Landscape".to_string(),
            90 => "Rotated 90°".to_string(),
            180 => "Upside down".to_string(),
            _ => "Rotated 270°".to_string(),
        };
        if self.flipped() {
            format!("{r}, mirrored")
        } else {
            r
        }
    }
}

/// One output as the compositor describes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputInfo {
    /// Connector name: "eDP-1", "HDMI-A-1", "Virtual-1".
    pub name: String,
    pub description: String,
    pub make: String,
    pub model: String,
    pub serial: String,
    pub phys_mm: (i32, i32),
    pub enabled: bool,
    pub mode: Option<Mode>,
    pub modes: Vec<Mode>,
    pub position: (i32, i32),
    pub transform: Transform,
    pub scale: f64,
    /// Adaptive sync (VRR) state, when the compositor offers it.
    pub vrr: Option<bool>,
}

impl OutputInfo {
    pub fn is_internal(&self) -> bool {
        let n = self.name.to_ascii_lowercase();
        n.starts_with("edp") || n.starts_with("lvds") || n.starts_with("dsi")
    }

    pub fn is_virtual(&self) -> bool {
        let n = self.name.to_ascii_lowercase();
        n.starts_with("virtual") || n.starts_with("headless") || n.starts_with("unknown")
    }

    /// Stable identity across connectors: make, model and serial.
    pub fn identity(&self) -> String {
        if self.serial.is_empty() && self.make.is_empty() && self.model.is_empty() {
            self.name.clone()
        } else {
            format!("{}|{}|{}", self.make, self.model, self.serial)
        }
    }

    /// A readable name: "HP OMEN 32", falling back to the connector.
    pub fn title(&self) -> String {
        let t = format!("{} {}", self.make, self.model).trim().to_string();
        if t.is_empty() || t == "Unknown Unknown" {
            self.name.clone()
        } else {
            t
        }
    }

    /// Size in logical pixels: the pixel size divided by the scale, axes swapped when rotated.
    pub fn logical_size(&self) -> (i32, i32) {
        match &self.mode {
            Some(m) => {
                let (w, h) = if self.transform.swaps_axes() { (m.height, m.width) } else { (m.width, m.height) };
                ((w as f64 / self.scale).round() as i32, (h as f64 / self.scale).round() as i32)
            }
            None => (0, 0),
        }
    }

    pub fn diagonal_inches(&self) -> Option<f64> {
        let (w, h) = self.phys_mm;
        if w <= 0 || h <= 0 {
            return None;
        }
        Some(((w * w + h * h) as f64).sqrt() / 25.4)
    }

    pub fn ppi(&self) -> Option<f64> {
        let m = self.mode.as_ref()?;
        let (w, _) = self.phys_mm;
        if w <= 0 {
            return None;
        }
        Some(m.width as f64 / (w as f64 / 25.4))
    }
}

/// What to change on one output. `None` leaves the field as it is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OutputRequest {
    pub name: String,
    pub enabled: Option<bool>,
    /// (width, height, refresh in mHz)
    pub mode: Option<(i32, i32, i32)>,
    pub position: Option<(i32, i32)>,
    pub transform: Option<Transform>,
    pub scale: Option<f64>,
    pub vrr: Option<bool>,
}

/// Software colour and dimming applied through the gamma ramps of the compositor.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gamma {
    /// 0.0 (black) to 1.0 (untouched).
    pub brightness: f64,
    /// Colour temperature in kelvin; 6500 leaves the colours alone.
    pub temperature: f64,
    /// Display gamma; 1.0 leaves the curve alone.
    pub gamma: f64,
}

impl Default for Gamma {
    fn default() -> Self {
        Gamma { brightness: 1.0, temperature: 6500.0, gamma: 1.0 }
    }
}

impl Gamma {
    pub fn is_identity(&self) -> bool {
        (self.brightness - 1.0).abs() < 1e-4 && (self.temperature - 6500.0).abs() < 1.0 && (self.gamma - 1.0).abs() < 1e-4
    }
}
