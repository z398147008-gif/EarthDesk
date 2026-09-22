use serde::Serialize;

#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn right(&self) -> i32 {
        self.x + self.w
    }
    fn bottom(&self) -> i32 {
        self.y + self.h
    }
    fn cx(&self) -> i32 {
        self.x + self.w / 2
    }
    fn cy(&self) -> i32 {
        self.y + self.h / 2
    }
}

/// A guide line to draw across the screen while dragging.
#[derive(Debug, Clone, Serialize)]
pub struct Guide {
    /// "v" for a vertical line at `pos` (an x coordinate), "h" for horizontal.
    pub orient: &'static str,
    pub pos: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapResult {
    pub x: i32,
    pub y: i32,
    pub guides: Vec<Guide>,
}

/// Snap `moving` against the monitor work area and the other widgets.
///
/// Each axis is resolved independently and only the single closest candidate
/// within `threshold` wins, so a widget can't be pulled two ways at once.
pub fn snap(moving: Rect, others: &[Rect], monitor: Rect, threshold: i32) -> SnapResult {
    // (candidate position for the moving edge, the line to draw)
    let mut xs: Vec<(i32, i32)> = vec![
        (monitor.x, monitor.x),                                  // left edge
        (monitor.right() - moving.w, monitor.right()),           // right edge
        (monitor.cx() - moving.w / 2, monitor.cx()),             // horizontal centre
    ];
    let mut ys: Vec<(i32, i32)> = vec![
        (monitor.y, monitor.y),
        (monitor.bottom() - moving.h, monitor.bottom()),
        (monitor.cy() - moving.h / 2, monitor.cy()),
    ];

    for o in others {
        // left-to-left, right-to-right, left-to-right, right-to-left, centre-to-centre
        xs.push((o.x, o.x));
        xs.push((o.right() - moving.w, o.right()));
        xs.push((o.right(), o.right()));
        xs.push((o.x - moving.w, o.x));
        xs.push((o.cx() - moving.w / 2, o.cx()));

        ys.push((o.y, o.y));
        ys.push((o.bottom() - moving.h, o.bottom()));
        ys.push((o.bottom(), o.bottom()));
        ys.push((o.y - moving.h, o.y));
        ys.push((o.cy() - moving.h / 2, o.cy()));
    }

    let mut guides = Vec::new();

    let mut x = moving.x;
    if let Some(&(target, line)) = xs
        .iter()
        .filter(|(t, _)| (t - moving.x).abs() <= threshold)
        .min_by_key(|(t, _)| (t - moving.x).abs())
    {
        x = target;
        guides.push(Guide { orient: "v", pos: line });
    }

    let mut y = moving.y;
    if let Some(&(target, line)) = ys
        .iter()
        .filter(|(t, _)| (t - moving.y).abs() <= threshold)
        .min_by_key(|(t, _)| (t - moving.y).abs())
    {
        y = target;
        guides.push(Guide { orient: "h", pos: line });
    }

    SnapResult { x, y, guides }
}
