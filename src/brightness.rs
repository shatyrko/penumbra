//! One brightness slider for hardware and software together.
//!
//! With `combined` on, the lowest part of the slider is software dimming (gamma ramps):
//! hardware brightness stays at its minimum there and the picture is darkened further.
//! Above that part the slider is the hardware brightness.

/// Share of the slider (from the bottom) that is software dimming in combined mode.
pub const SOFT_ZONE: f64 = 0.15;
/// The darkest the software dimming goes: the screen never goes fully black.
pub const MIN_DIM: f64 = 0.08;

/// Slider position → (hardware fraction 0..1, software dim factor 0..1).
pub fn split(v: f64, combined: bool) -> (f64, f64) {
    let v = v.clamp(0.0, 1.0);
    if !combined {
        return (v, 1.0);
    }
    if v >= SOFT_ZONE {
        ((v - SOFT_ZONE) / (1.0 - SOFT_ZONE), 1.0)
    } else {
        (0.0, MIN_DIM + (1.0 - MIN_DIM) * (v / SOFT_ZONE))
    }
}

/// The reverse of `split`.
pub fn join(hw: f64, dim: f64, combined: bool) -> f64 {
    if !combined {
        return hw.clamp(0.0, 1.0);
    }
    if dim < 0.9999 {
        (SOFT_ZONE * ((dim - MIN_DIM) / (1.0 - MIN_DIM))).clamp(0.0, SOFT_ZONE)
    } else {
        (SOFT_ZONE + hw.clamp(0.0, 1.0) * (1.0 - SOFT_ZONE)).clamp(SOFT_ZONE, 1.0)
    }
}

/// Slider position → dim factor for a screen with no hardware control.
pub fn soft_only(v: f64) -> f64 {
    MIN_DIM + (1.0 - MIN_DIM) * v.clamp(0.0, 1.0)
}

pub fn soft_only_inverse(dim: f64) -> f64 {
    ((dim - MIN_DIM) / (1.0 - MIN_DIM)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_join_roundtrip() {
        for i in 0..=100 {
            let v = i as f64 / 100.0;
            let (hw, dim) = split(v, true);
            assert!((join(hw, dim, true) - v).abs() < 1e-9, "v={v}");
            let (hw, dim) = split(v, false);
            assert_eq!(dim, 1.0);
            assert!((join(hw, dim, false) - v).abs() < 1e-9);
        }
    }

    #[test]
    fn ends() {
        assert_eq!(split(1.0, true), (1.0, 1.0));
        assert_eq!(split(SOFT_ZONE, true), (0.0, 1.0));
        let (hw, dim) = split(0.0, true);
        assert_eq!(hw, 0.0);
        assert!((dim - MIN_DIM).abs() < 1e-9);
    }

    #[test]
    fn soft_only_roundtrip() {
        for i in 0..=10 {
            let v = i as f64 / 10.0;
            assert!((soft_only_inverse(soft_only(v)) - v).abs() < 1e-9);
        }
    }
}
