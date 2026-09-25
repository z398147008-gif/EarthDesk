//! Turning a pointer path into a string of directions ("↓→").
//!
//! The approach used by WGestures / MouseInc: walk the path in short steps,
//! quantise each step's heading into one of eight (or four) directions, and
//! only accept a new direction once the pointer has kept going that way for a
//! little while. Small wobbles at the start of a stroke or around a corner
//! therefore never show up as extra segments.
//!
//! When the string still does not name a rule (a V drawn with a rounded
//! bottom reads "↓↘↗"), the stroke is matched by its overall shape against
//! the rules' ideal shapes instead ($1-recognizer style: resample, normalise,
//! mean point distance), see `best_shape`.

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
    /// The whole path (for matching by shape when the direction string is
    /// off by a wobble).
    points: Vec<(f64, f64)>,
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
            points: vec![(start.0 as f64, start.1 as f64)],
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
        if self.points.len() < 20_000 {
            self.points.push((x, y));
        }
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

    pub fn points(&self) -> &[(f64, f64)] {
        &self.points
    }}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn bbox(pts: &[(f64, f64)]) -> (f64, f64) {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in pts {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    (x1 - x0, y1 - y0)
}

// --- Matching by shape -------------------------------------------------------------

const N: usize = 32;

/// The ideal stroke for a direction string: unit-length segments.
fn template(gesture: &str) -> Vec<(f64, f64)> {
    let mut p = (0.0, 0.0);
    let mut out = vec![p];
    for c in gesture.chars() {
        let Some((_, a)) = ARROWS.iter().find(|(ch, _)| *ch == c) else { continue };
        let r = a.to_radians();
        p = (p.0 + r.cos(), p.1 + r.sin());
        out.push(p);
    }
    out
}

/// N points evenly spaced along the path.
fn resample(pts: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let total: f64 = pts.windows(2).map(|w| dist(w[0], w[1])).sum();
    if pts.len() < 2 || total <= 0.0 {
        return vec![pts.first().copied().unwrap_or((0.0, 0.0)); N];
    }
    let step = total / (N - 1) as f64;
    let mut out = vec![pts[0]];
    let mut acc = 0.0;
    let mut prev = pts[0];
    let mut i = 1;
    while i < pts.len() && out.len() < N {
        let cur = pts[i];
        let d = dist(prev, cur);
        if acc + d >= step && d > 0.0 {
            let t = (step - acc) / d;
            let q = (prev.0 + t * (cur.0 - prev.0), prev.1 + t * (cur.1 - prev.1));
            out.push(q);
            prev = q;
            acc = 0.0;
        } else {
            acc += d;
            prev = cur;
            i += 1;
        }
    }
    while out.len() < N {
        out.push(*pts.last().unwrap());
    }
    out
}

/// Centre on the centroid, scale the larger side of the bounding box to 1
/// (uniformly: a V must not match a Λ, and a wide V should still match a
/// narrow one).
fn normalise(mut pts: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    let n = pts.len() as f64;
    let (cx, cy) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0 / n, a.1 + p.1 / n));
    let (w, h) = bbox(&pts);
    let s = w.max(h).max(1e-9);
    for p in &mut pts {
        *p = ((p.0 - cx) / s, (p.1 - cy) / s);
    }
    pts
}

/// Mean distance between the stroke and the ideal shape of `gesture`
/// (0 = identical; about 0.1 is a sloppy but clear match).
pub fn shape_distance(points: &[(f64, f64)], gesture: &str) -> f64 {
    let a = normalise(resample(points));
    let b = normalise(resample(&template(gesture)));
    a.iter().zip(&b).map(|(p, q)| dist(*p, *q)).sum::<f64>() / N as f64
}

/// The closest of `gestures` to the stroke, if it is close enough and
/// clearly closer than the runner-up.
pub fn best_shape<'a>(points: &[(f64, f64)], gestures: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut scored: Vec<(f64, &str)> = gestures.map(|g| (shape_distance(points, g), g)).collect();
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.dedup_by(|a, b| a.1 == b.1);
    let (best, g) = *scored.first()?;
    let second = scored.get(1).map(|s| s.0).unwrap_or(f64::MAX);
    (best < 0.2 && best < second * 0.75).then_some(g)
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

    fn stroke(points: &[(i32, i32)]) -> Recognizer {
        let mut r = Recognizer::new(points[0], 1.0, true);
        let mut last = points[0];
        for &p in &points[1..] {
            for i in 1..=20 {
                r.push((last.0 + (p.0 - last.0) * i / 20, last.1 + (p.1 - last.1) * i / 20));
            }
            last = p;
        }
        r
    }

    #[test]
    fn a_v_is_its_shape() {
        // Steep left arm, rounded bottom, right arm: the live string picks up
        // an extra ↓ / ↘, the shape does not.
        let v = stroke(&[(100, 100), (150, 260), (165, 300), (180, 310), (195, 300), (300, 110)]);
        assert_ne!(v.gesture(), "↘↗");
        let rules = ["↑↓", "↘↗", "↓→", "←", "→", "↓"];
        assert_eq!(best_shape(v.points(), rules.iter().copied()), Some("↘↗"));
        // With only ↓↑ among the rules, a narrow V still means ↓↑.
        let narrow = stroke(&[(100, 100), (130, 300), (160, 110)]);
        assert_eq!(best_shape(narrow.points(), ["↑↓", "↓↑", "↓→", "→"].iter().copied()), Some("↓↑"));
    }

    #[test]
    fn scribbles_match_nothing() {
        let z = stroke(&[(100, 100), (300, 120), (110, 200), (310, 330), (90, 90), (250, 400)]);
        assert_eq!(best_shape(z.points(), ["↑↓", "↘↗", "↓→", "←"].iter().copied()), None);
    }

    #[test]
    fn sloppy_l_is_down_right() {
        let l = stroke(&[(100, 100), (105, 250), (130, 290), (300, 300)]);
        assert_eq!(best_shape(l.points(), ["↓→", "↓←", "→", "↓", "↘"].iter().copied()), Some("↓→"));
    }

    #[test]
    fn short_wiggle_is_ignored() {
        assert_eq!(run(&[(500, 500), (500, 700), (512, 700), (512, 900)], true), "↓");
    }
}
