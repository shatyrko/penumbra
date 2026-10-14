//! Pure helpers for resolution lists, scale choices and arranging screens side by side.

use crate::model::{Mode, OutputInfo};

/// Distinct pixel sizes, largest first.
pub fn sizes(modes: &[Mode]) -> Vec<(i32, i32)> {
    let mut v: Vec<(i32, i32)> = Vec::new();
    for m in modes {
        if !v.contains(&(m.width, m.height)) {
            v.push((m.width, m.height));
        }
    }
    v.sort_by_key(|(w, h)| std::cmp::Reverse((*w as i64) * (*h as i64) * 10000 + *w as i64));
    v
}

/// The modes of one size, fastest first.
pub fn rates(modes: &[Mode], size: (i32, i32)) -> Vec<Mode> {
    let mut v: Vec<Mode> = modes.iter().filter(|m| (m.width, m.height) == size).cloned().collect();
    v.sort_by_key(|m| std::cmp::Reverse(m.refresh_mhz));
    v
}

pub const SCALE_PRESETS: [f64; 12] = [0.75, 1.0, 1.25, 4.0 / 3.0, 1.5, 1.6, 1.75, 2.0, 2.25, 2.5, 3.0, 4.0];

/// (scale, label) choices for a screen of `px` pixels. The current scale is included even when
/// it is not a preset. Returns the list and the index of the current scale.
pub fn scale_options(px: (i32, i32), current: f64) -> (Vec<(f64, String)>, usize) {
    let mut scales: Vec<f64> = SCALE_PRESETS.to_vec();
    if !scales.iter().any(|s| (s - current).abs() < 0.01) {
        scales.push(current);
        scales.sort_by(|a, b| a.partial_cmp(b).unwrap());
    }
    let mut out = Vec::new();
    for s in scales {
        let lw = (px.0 as f64 / s).round() as i32;
        let lh = (px.1 as f64 / s).round() as i32;
        if lw < 640 && (s - current).abs() >= 0.01 {
            continue;
        }
        out.push((s, format!("{lw} × {lh}   ({}%)", (s * 100.0).round() as i32)));
    }
    let idx = out.iter().position(|(s, _)| (s - current).abs() < 0.01).unwrap_or(0);
    (out, idx)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
}

pub fn rect_of(o: &OutputInfo) -> Rect {
    let (w, h) = o.logical_size();
    Rect { x: o.position.0, y: o.position.1, w, h }
}

/// Snap a dragged rectangle to the edges of the others (within `tol` logical pixels), and if it
/// still overlaps one, push it out along the shallowest side so screens never overlap.
pub fn snap(mut r: Rect, others: &[Rect], tol: i32) -> Rect {
    // Edge to edge, with the other axis aligned to top/bottom/left/right or centred.
    let mut best_dx: Option<i32> = None;
    let mut best_dy: Option<i32> = None;
    let consider = |cur: &mut Option<i32>, d: i32| {
        if d.abs() <= tol && cur.map(|c| d.abs() < c.abs()).unwrap_or(true) {
            *cur = Some(d);
        }
    };
    for o in others {
        // horizontal snaps
        consider(&mut best_dx, o.right() - r.x); // my left to their right
        consider(&mut best_dx, o.x - r.right()); // my right to their left
        consider(&mut best_dx, o.x - r.x); // left edges
        consider(&mut best_dx, o.right() - r.right()); // right edges
        // vertical snaps
        consider(&mut best_dy, o.bottom() - r.y);
        consider(&mut best_dy, o.y - r.bottom());
        consider(&mut best_dy, o.y - r.y);
        consider(&mut best_dy, o.bottom() - r.bottom());
    }
    if let Some(d) = best_dx {
        r.x += d;
    }
    if let Some(d) = best_dy {
        r.y += d;
    }
    // Resolve overlap.
    for _ in 0..4 {
        let Some(o) = others.iter().find(|o| r.overlaps(o)) else { break };
        let moves = [
            (o.right() - r.x, 0),  // go right of it
            (o.x - r.right(), 0),  // go left of it
            (0, o.bottom() - r.y), // below
            (0, o.y - r.bottom()), // above
        ];
        let (dx, dy) = moves.iter().copied().min_by_key(|(dx, dy)| dx.abs() + dy.abs()).unwrap();
        r.x += dx;
        r.y += dy;
    }
    r
}

/// True when every rectangle touches another (no screen floats alone), which compositors want.
pub fn connected(rects: &[Rect]) -> bool {
    if rects.len() <= 1 {
        return true;
    }
    let touches = |a: &Rect, b: &Rect| {
        let h = (a.right() == b.x || b.right() == a.x) && a.y < b.bottom() && b.y < a.bottom();
        let v = (a.bottom() == b.y || b.bottom() == a.y) && a.x < b.right() && b.x < a.right();
        h || v
    };
    let mut seen = vec![false; rects.len()];
    let mut stack = vec![0];
    seen[0] = true;
    while let Some(i) = stack.pop() {
        for j in 0..rects.len() {
            if !seen[j] && touches(&rects[i], &rects[j]) {
                seen[j] = true;
                stack.push(j);
            }
        }
    }
    seen.iter().all(|s| *s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(w: i32, h: i32, hz: i32) -> Mode {
        Mode { width: w, height: h, refresh_mhz: hz, preferred: false }
    }

    #[test]
    fn sizes_and_rates() {
        let modes = vec![m(1920, 1080, 60000), m(3840, 2160, 60000), m(3840, 2160, 165000), m(2560, 1440, 144000)];
        assert_eq!(sizes(&modes), vec![(3840, 2160), (2560, 1440), (1920, 1080)]);
        assert_eq!(rates(&modes, (3840, 2160)).iter().map(|x| x.refresh_mhz).collect::<Vec<_>>(), vec![165000, 60000]);
    }

    #[test]
    fn scales() {
        let (opts, idx) = scale_options((3840, 2160), 1.5);
        assert_eq!(opts[idx].0, 1.5);
        assert!(opts[idx].1.starts_with("2560 × 1440"));
        let (opts, idx) = scale_options((2560, 1600), 1.332);
        assert!((opts[idx].0 - 1.332).abs() < 0.01 || (opts[idx].0 - 4.0 / 3.0).abs() < 0.01);
        assert!(opts.iter().any(|(_, l)| l.starts_with("1920 × 1200")));
    }

    #[test]
    fn snapping() {
        let a = Rect { x: 0, y: 0, w: 1920, h: 1200 };
        // dropped slightly off to the right of a, slightly lower
        let r = snap(Rect { x: 1930, y: 20, w: 2560, h: 1440 }, &[a], 40);
        assert_eq!((r.x, r.y), (1920, 0));
        // dropped over a: pushed out
        let r = snap(Rect { x: 500, y: 100, w: 800, h: 600 }, &[a], 10);
        assert!(!r.overlaps(&a));
        // far away: stays
        let r = snap(Rect { x: 5000, y: 3000, w: 800, h: 600 }, &[a], 40);
        assert_eq!((r.x, r.y), (5000, 3000));
    }

    #[test]
    fn connectivity() {
        let a = Rect { x: 0, y: 0, w: 100, h: 100 };
        let b = Rect { x: 100, y: 20, w: 100, h: 100 };
        let c = Rect { x: 500, y: 0, w: 100, h: 100 };
        assert!(connected(&[a, b]));
        assert!(!connected(&[a, b, c]));
    }
}
