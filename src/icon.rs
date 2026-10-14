//! The tray icon, drawn by code (a monitor with a sun) so that it needs no icon theme.

fn sd_round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - hw + r;
    let qy = (py - cy).abs() - hh + r;
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

fn sd_capsule(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32, r: f32) -> f32 {
    let (pax, pay) = (px - ax, py - ay);
    let (bax, bay) = (bx - ax, by - ay);
    let h = ((pax * bax + pay * bay) / (bax * bax + bay * bay).max(1e-9)).clamp(0.0, 1.0);
    let (dx, dy) = (pax - bax * h, pay - bay * h);
    (dx * dx + dy * dy).sqrt() - r
}

fn glyph(x: f32, y: f32) -> f32 {
    // Distance (negative inside) to the union of the glyph shapes in the unit square.
    let screen = sd_round_rect(x, y, 0.5, 0.42, 0.40, 0.28, 0.08).abs() - 0.0375;
    let neck = sd_capsule(x, y, 0.5, 0.70, 0.5, 0.80, 0.0375);
    let foot = sd_capsule(x, y, 0.33, 0.86, 0.67, 0.86, 0.0375);
    let sun = ((x - 0.5).powi(2) + (y - 0.42).powi(2)).sqrt() - 0.095;
    let mut d = screen.min(neck).min(foot).min(sun);
    for i in 0..8 {
        let a = i as f32 * std::f32::consts::FRAC_PI_4;
        let (s, c) = a.sin_cos();
        let r = sd_capsule(x, y, 0.5 + c * 0.150, 0.42 + s * 0.150, 0.5 + c * 0.185, 0.42 + s * 0.185, 0.022);
        d = d.min(r);
    }
    d
}

/// ARGB32 pixels in network byte order, as StatusNotifierItem wants them.
pub fn argb(size: u32, rgb: (u8, u8, u8)) -> Vec<u8> {
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    let ss = 4;
    for y in 0..size {
        for x in 0..size {
            let mut cover = 0.0;
            for sy in 0..ss {
                for sx in 0..ss {
                    let fx = (x as f32 + (sx as f32 + 0.5) / ss as f32) / size as f32;
                    let fy = (y as f32 + (sy as f32 + 0.5) / ss as f32) / size as f32;
                    if glyph(fx, fy) < 0.0 {
                        cover += 1.0;
                    }
                }
            }
            let a = (cover / (ss * ss) as f32 * 255.0).round() as u8;
            out.extend_from_slice(&[a, rgb.0, rgb.1, rgb.2]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_ink_and_gaps() {
        let px = argb(32, (255, 255, 255));
        assert_eq!(px.len(), 32 * 32 * 4);
        let alpha: Vec<u8> = px.chunks(4).map(|c| c[0]).collect();
        assert!(alpha.iter().filter(|a| **a > 200).count() > 60);
        assert_eq!(alpha[0], 0); // corner is empty
    }
}
