//! The content-stream emission of the writer before the cleanup (a verbatim copy of the old
//! per-operator formatting, which re-emitted the full style for every operator, had no `q` limit
//! and left `1 j` set after polylines), plus an interpreter that reduces a content stream to the
//! effective graphics state of every painting operator.

use super::super::*;

fn old_rgb_op(c: Rgb, stroke: bool) -> String {
    let f = |v: u8| n(v as f32 / 255.0);
    format!("{} {} {} {}", f(c[0]), f(c[1]), f(c[2]), if stroke { "RG" } else { "rg" })
}

/// The old content writer.
#[derive(Default)]
pub struct OldPage {
    pub height: f32,
    pub content: String,
    depth: u32,
    images: usize,
}

impl OldPage {
    pub fn new(height: f32) -> OldPage {
        OldPage { height, ..Default::default() }
    }
    fn y(&self, y: f32) -> f32 {
        self.height - y
    }
    fn op(&mut self, s: &str) {
        self.content.push_str(s);
        self.content.push('\n');
    }
    pub fn save(&mut self) {
        self.depth += 1;
        self.content.push_str("q\n");
    }
    pub fn restore(&mut self) {
        if self.depth > 0 {
            self.depth -= 1;
            self.content.push_str("Q\n");
        }
    }
    pub fn clip_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let s = format!("{} {} {} {} re W n", n(x), n(self.y(y + h)), n(w.max(0.0)), n(h.max(0.0)));
        self.op(&s);
    }
    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgb) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let s = format!("{}\n{} {} {} {} re f", old_rgb_op(color, false), n(x), n(self.y(y + h)), n(w), n(h));
        self.op(&s);
    }
    pub fn rect_stroke(&mut self, x: f32, y: f32, w: f32, h: f32, width: f32, color: Rgb) {
        let s = format!("{}\n{} w [] 0 d\n{} {} {} {} re S", old_rgb_op(color, true), n(width), n(x), n(self.y(y + h)), n(w), n(h));
        self.op(&s);
    }
    pub fn stroke_line_dashed(&mut self, start: Point, end: Point, width: f32, color: Rgb, dash: &[f32]) {
        let (x0, y0, x1, y1) = (start.x, start.y, end.x, end.y);
        let d: Vec<String> = dash.iter().take(8).map(|v| n(v.max(0.0))).collect();
        let s = format!(
            "{}\n{} w [{}] 0 d\n{} {} m {} {} l S",
            old_rgb_op(color, true),
            n(width),
            d.join(" "),
            n(x0),
            n(self.y(y0)),
            n(x1),
            n(self.y(y1))
        );
        self.op(&s);
    }
    pub fn polyline(&mut self, pts: &[[f32; 2]], width: f32, color: Rgb, dash: &[f32]) {
        if pts.len() < 2 {
            return;
        }
        let d: Vec<String> = dash.iter().take(8).map(|v| n(v.max(0.0))).collect();
        let mut s = format!("{}\n{} w [{}] 0 d 1 j\n", old_rgb_op(color, true), n(width), d.join(" "));
        self.path(&mut s, pts, false);
        s.push_str(" S");
        self.op(&s);
    }
    fn path(&self, s: &mut String, pts: &[[f32; 2]], close: bool) {
        for (i, p) in pts.iter().enumerate() {
            let _ = write!(s, "{} {} {} ", n(p[0]), n(self.y(p[1])), if i == 0 { "m" } else { "l" });
        }
        if close {
            s.push('h');
        }
    }
    pub fn polygon(&mut self, pts: &[[f32; 2]], fill: Option<Rgb>, stroke: Option<(Rgb, f32)>) {
        if pts.len() < 2 || (fill.is_none() && stroke.is_none()) {
            return;
        }
        let mut s = String::new();
        if let Some(f) = fill {
            s.push_str(&old_rgb_op(f, false));
            s.push('\n');
        }
        if let Some((c, w)) = stroke {
            let _ = writeln!(s, "{}\n{} w [] 0 d 1 j", old_rgb_op(c, true), n(w));
        }
        self.path(&mut s, pts, true);
        s.push_str(match (fill.is_some(), stroke.is_some()) {
            (true, true) => " B",
            (true, false) => " f",
            _ => " S",
        });
        self.op(&s);
    }
    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, fill: Option<Rgb>, stroke: Option<(Rgb, f32)>) {
        if r <= 0.0 || (fill.is_none() && stroke.is_none()) {
            return;
        }
        let y = self.y(cy);
        let mut s = String::new();
        if let Some(f) = fill {
            s.push_str(&old_rgb_op(f, false));
            s.push('\n');
        }
        if let Some((c, w)) = stroke {
            let _ = writeln!(s, "{}\n{} w [] 0 d", old_rgb_op(c, true), n(w));
        }
        const K: f32 = 0.552_284_8;
        let k = r * K;
        let _ = write!(
            s,
            "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c h",
            n(cx + r),
            n(y),
            n(cx + r),
            n(y + k),
            n(cx + k),
            n(y + r),
            n(cx),
            n(y + r),
            n(cx - k),
            n(y + r),
            n(cx - r),
            n(y + k),
            n(cx - r),
            n(y),
            n(cx - r),
            n(y - k),
            n(cx - k),
            n(y - r),
            n(cx),
            n(y - r),
            n(cx + k),
            n(y - r),
            n(cx + r),
            n(y - k),
            n(cx + r),
            n(y)
        );
        s.push_str(match (fill.is_some(), stroke.is_some()) {
            (true, true) => " B",
            (true, false) => " f",
            _ => " S",
        });
        self.op(&s);
    }
    pub fn text_rotated(&mut self, origin: Point, size: f32, font: Font, color: Rgb, text: &str, angle: f32) {
        let (x, y) = (origin.x, origin.y);
        let fi = font.index();
        let (sn, cs) = angle.sin_cos();
        let s = format!(
            "BT\n{}\n/F{} {} Tf\n{} {} {} {} {} {} Tm\n{} Tj\nET",
            old_rgb_op(color, false),
            fi + 1,
            n(size),
            n(cs),
            n(-sn),
            n(sn),
            n(cs),
            n(x),
            n(self.y(y)),
            pdf_string(text)
        );
        self.op(&s);
    }
    pub fn image(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.images += 1;
        let s = format!("q {} 0 0 {} {} {} cm /Im{} Do Q", n(w), n(h), n(x), n(self.y(y + h)), self.images);
        self.op(&s);
    }
}

/// The effective state at one painting operator. Fields that cannot influence the operator
/// (the stroke state of a fill, the fill of a stroke) are left empty.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Paint {
    pub op: String,
    /// Path construction / text operands since the previous painting operator.
    pub geometry: String,
    pub fill: String,
    pub stroke: String,
    pub width: String,
    pub dash: String,
    pub join: String,
    /// Clip paths in effect (innermost last).
    pub clip: Vec<String>,
}

#[derive(Default, Debug)]
pub struct Interp {
    pub paints: Vec<Paint>,
    pub max_depth: usize,
    /// Number of `q` operators.
    pub qs: usize,
    /// Number of colour/width/dash/join operators.
    pub style_ops: usize,
    pub balanced: bool,
}

fn tokens(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = content.chars();
    while let Some(c) = chars.next() {
        match c {
            '(' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                let mut s = String::from("(");
                while let Some(d) = chars.next() {
                    s.push(d);
                    if d == '\\' {
                        if let Some(e) = chars.next() {
                            s.push(e);
                        }
                    } else if d == ')' {
                        break;
                    }
                }
                out.push(s);
            }
            '[' | ']' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                out.push(c.to_string());
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[derive(Clone)]
struct State {
    fill: String,
    stroke: String,
    width: String,
    dash: String,
    join: String,
    clip: Vec<String>,
}

/// Interprets a content stream: tracks q/Q, colours, width, dash, join and clips.
pub fn interpret(content: &str) -> Interp {
    let mut st = State { fill: "0 0 0".into(), stroke: "0 0 0".into(), width: "1".into(), dash: String::new(), join: "0".into(), clip: Vec::new() };
    let mut stack: Vec<State> = Vec::new();
    let mut r = Interp { balanced: true, ..Default::default() };
    let mut operands: Vec<String> = Vec::new();
    let mut path: Vec<String> = Vec::new();
    let mut pending_clip = false;
    for t in tokens(content) {
        let is_op = t.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '*') && !t.starts_with('/');
        if !is_op {
            operands.push(t);
            continue;
        }
        let ops = std::mem::take(&mut operands);
        match t.as_str() {
            "q" => {
                stack.push(st.clone());
                r.qs += 1;
                r.max_depth = r.max_depth.max(stack.len());
            }
            "Q" => match stack.pop() {
                Some(s) => st = s,
                None => r.balanced = false,
            },
            "rg" => {
                st.fill = ops.join(" ");
                r.style_ops += 1;
            }
            "RG" => {
                st.stroke = ops.join(" ");
                r.style_ops += 1;
            }
            "w" => {
                st.width = ops.join(" ");
                r.style_ops += 1;
            }
            "d" => {
                // `[a b] 0` -> `a b`
                st.dash = ops.iter().take_while(|o| *o != "]").skip(1).cloned().collect::<Vec<_>>().join(" ");
                r.style_ops += 1;
            }
            "j" => {
                st.join = ops.join(" ");
                r.style_ops += 1;
            }
            "re" | "m" | "l" | "c" | "h" | "cm" | "Tf" | "Tm" => {
                path.push(format!("{} {}", ops.join(" "), t));
            }
            "BT" | "ET" => {}
            "W" => pending_clip = true,
            "n" => {
                if pending_clip {
                    st.clip.push(path.join(" "));
                    pending_clip = false;
                }
                path.clear();
            }
            "Tj" => {
                path.push(format!("{} {}", ops.join(" "), t));
                r.paints.push(Paint { op: t.clone(), geometry: path.join(" "), fill: st.fill.clone(), clip: st.clip.clone(), ..Default::default() });
                path.clear();
            }
            "f" | "S" | "B" | "Do" => {
                let mut p = Paint { op: t.clone(), geometry: path.join(" "), clip: st.clip.clone(), ..Default::default() };
                if t == "f" || t == "B" {
                    p.fill = st.fill.clone();
                }
                if t == "S" || t == "B" {
                    p.stroke = st.stroke.clone();
                    p.width = st.width.clone();
                    p.dash = st.dash.clone();
                    p.join = st.join.clone();
                }
                r.paints.push(p);
                path.clear();
            }
            _ => {}
        }
    }
    r.balanced &= stack.is_empty();
    r
}
