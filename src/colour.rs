//! Gamma ramps: brightness, colour temperature and gamma curve.

use crate::model::Gamma;

/// Relative RGB gain for a colour temperature (Tanner Helland's fit of the blackbody locus).
/// 6500 K is neutral: (1, 1, 1).
pub fn kelvin_to_rgb(kelvin: f64) -> (f64, f64, f64) {
    let t = kelvin.clamp(1000.0, 12000.0) / 100.0;
    let raw = |t: f64| -> (f64, f64, f64) {
        let r = if t <= 66.0 { 255.0 } else { (329.698727446 * (t - 60.0).powf(-0.1332047592)).clamp(0.0, 255.0) };
        let g = if t <= 66.0 {
            (99.4708025861 * t.ln() - 161.1195681661).clamp(0.0, 255.0)
        } else {
            (288.1221695283 * (t - 60.0).powf(-0.0755148492)).clamp(0.0, 255.0)
        };
        let b = if t >= 66.0 {
            255.0
        } else if t <= 19.0 {
            0.0
        } else {
            (138.5177312231 * (t - 10.0).ln() - 305.0447927307).clamp(0.0, 255.0)
        };
        (r / 255.0, g / 255.0, b / 255.0)
    };
    let (r, g, b) = raw(t);
    let (nr, ng, nb) = raw(65.0);
    ((r / nr).min(1.0), (g / ng).min(1.0), (b / nb).min(1.0))
}

/// Build the three ramps of `size` entries each, laid out red, green, blue (u16 each).
pub fn ramp(size: usize, g: &Gamma) -> Vec<u16> {
    let (kr, kg, kb) = kelvin_to_rgb(g.temperature);
    let gains = [kr, kg, kb];
    let mut out = vec![0u16; size * 3];
    let denom = (size.max(2) - 1) as f64;
    for (c, gain) in gains.iter().enumerate() {
        for i in 0..size {
            let x = i as f64 / denom;
            let v = x.powf(1.0 / g.gamma.max(0.1)) * gain * g.brightness.clamp(0.0, 1.0);
            out[c * size + i] = (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_at_6500() {
        let (r, g, b) = kelvin_to_rgb(6500.0);
        assert!((r - 1.0).abs() < 0.01 && (g - 1.0).abs() < 0.01 && (b - 1.0).abs() < 0.01);
    }

    #[test]
    fn warm_has_less_blue() {
        let (r, _, b) = kelvin_to_rgb(3000.0);
        assert!(b < r && b < 0.7);
    }

    #[test]
    fn ramp_identity_is_linear() {
        let r = ramp(256, &Gamma::default());
        assert_eq!(r.len(), 768);
        assert_eq!(r[0], 0);
        assert_eq!(r[255], 65535);
        assert!((r[128] as i32 - 32896).abs() < 400);
    }

    #[test]
    fn brightness_scales_the_top() {
        let r = ramp(256, &Gamma { brightness: 0.5, ..Gamma::default() });
        assert!((r[255] as i32 - 32768).abs() < 2);
    }
}
