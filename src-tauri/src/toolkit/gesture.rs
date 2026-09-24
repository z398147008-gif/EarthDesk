//! Turning a pointer path into a string of directions ("↓→").
//!
//! The approach used by WGestures / MouseInc: walk the path in short steps,
//! quantise each step's heading into one of eight (or four) directions, and
//! only accept a new direction once the pointer has kept going that way for a
//! little while. Small wobbles at the start of a stroke or around a corner
//! therefore never show up as extra segments.

pub const ARROWS: [(char, f64); 8] = [
    ('→', 0.0),
    ('↘', 45.0),
    ('↓', 90.0),
    ('↙', 135.0),
    ('←', 180.0),
    ('↖', 225.0),
    ('↑', 270.0),
    ('↗', 315.0),
];

/// More than this many segments is scribbling, not a gesture.
pub const MAX_SEGMENTS: usize = 8;

#[derive(Debug, Clone)]
pub struct Recognizer {
    /// Step length and the length a new direction must keep before it
    /// counts, both in physical pixels.
    step: f64,
    min_segment: f64,
    diagonals: bool,
    anchor: (f64, f64),
    pending: Option<char>,
    pending_len: f64,
    dirs: Vec<char>,
}

impl Recognizer {
    /// `scale` is the monitor's DPI scale (1.0 at 100%).
    pub fn new(start: (i32, i32), scale: f64, diagonals: bool) -> Self {
        Recognizer {
            step: 6.0 * scale,
            min_segment: 26.0 * scale,
            diagonals,
            anchor: (start.0 as f64, start.1 as f64),
            pending: None,
            pending_len: 0.0,
            dirs: Vec::new(),
        }
    }

    fn quantise(&self, dx: f64, dy: f64) -> char {
        // Screen y grows downwards, so atan2(dy, dx) is a clockwise angle
        // with 0 pointing right -- the same convention as ARROWS.
        let mut a = dy.atan2(dx).to_degrees();
        if a < 0.0 {
            a += 360.0;
        }
        if !self.diagonals {
            let i = (((a + 45.0) % 360.0) / 90.0) as usize;
            return ['→', '↓', '←', '↑'][i.min(3)];
        }
        // Diagonals get a narrower slice (30°) than the four main directions
        // (60°): a slightly slanted straight stroke is far more common than a
        // deliberate diagonal, and it should read as the straight one.
        for (arrow, centre) in ARROWS {
            let half = if (centre as i32) % 90 == 0 { 30.0 } else { 15.0 };
            let mut d = (a - centre).abs();
            if d > 180.0 {
                d = 360.0 - d;
            }
            if d <= half {
                return arrow;
            }
        }
        '→'
    }

    /// Feed the next pointer position. Returns true when the recognised
    /// string changed.
    pub fn push(&mut self, p: (i32, i32)) -> bool {
        let (x, y) = (p.0 as f64, p.1 as f64);
        let (dx, dy) = (x - self.anchor.0, y - self.anchor.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < self.step {
            return false;
        }
        self.anchor = (x, y);
        let dir = self.quantise(dx, dy);
        if self.dirs.last() == Some(&dir) {
            // Still going the same way: any tentative turn was a wobble.
            self.pending = None;
            self.pending_len = 0.0;
            return false;
        }
        if self.pending == Some(dir) {
            self.pending_len += len;
        } else {
            self.pending = Some(dir);
            self.pending_len = len;
        }
        if self.pending_len >= self.min_segment {
            self.dirs.push(dir);
            self.pending = None;
            self.pending_len = 0.0;
            return true;
        }
        false
    }

    pub fn gesture(&self) -> String {
        self.dirs.iter().collect()
    }

    pub fn too_long(&self) -> bool {
        self.dirs.len() > MAX_SEGMENTS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(points: &[(i32, i32)], diagonals: bool) -> String {
        let mut r = Recognizer::new(points[0], 1.0, diagonals);
        let mut last = points[0];
        for &p in &points[1..] {
            // Interpolate like a real mouse would report.
            let n = 20;
            for i in 1..=n {
                let q = (last.0 + (p.0 - last.0) * i / n, last.1 + (p.1 - last.1) * i / n);
                r.push(q);
            }
            last = p;
        }
        r.gesture()
    }

    #[test]
    fn straight_lines() {
        assert_eq!(run(&[(500, 500), (300, 500)], true), "←");
        assert_eq!(run(&[(500, 500), (700, 510)], true), "→");
        assert_eq!(run(&[(500, 500), (500, 300)], true), "↑");
    }

    #[test]
    fn corners() {
        assert_eq!(run(&[(500, 500), (500, 700), (700, 700)], true), "↓→");
        assert_eq!(run(&[(500, 500), (500, 300), (500, 520)], true), "↑↓");
    }

    #[test]
    fn slanted_strokes_stay_straight() {
        // 20° off horizontal is still "left".
        assert_eq!(run(&[(500, 500), (300, 427)], true), "←");
    }

    #[test]
    fn diagonals() {
        assert_eq!(run(&[(500, 500), (650, 350)], true), "↗");
        assert_eq!(run(&[(500, 500), (650, 350)], false).len(), "↑".len());
    }

    #[test]
    fn short_wiggle_is_ignored() {
        assert_eq!(run(&[(500, 500), (500, 700), (512, 700), (512, 900)], true), "↓");
    }
}
