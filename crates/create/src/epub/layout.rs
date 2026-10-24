//! Laying a book's blocks out on pages. Paragraphs break into lines at the opportunities UAX #14
//! gives (`unicode-linebreak`), are reordered for right-to-left text by UAX #9 (`unicode-bidi`),
//! and are aligned or justified; blocks stack with collapsing vertical margins and move to the next
//! page when they don't fit. Images are scaled to the text width, tables get equal columns, and a
//! fixed-layout document becomes one page of its viewport's size.

use std::collections::HashMap;

use unicode_linebreak::BreakOpportunity;

use super::dom::{Align, Block, PT_PER_PX, Run, Style, VAlign};
use super::text::{FaceId, Fonts, Glyph};

/// Margins of a reflowed page, in points.
pub const MARGIN: f64 = 54.0;

/// Something drawn on a page. Coordinates are PDF's (points, y up from the bottom).
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// Glyphs of one face, size and colour, each at its own position (the baseline's).
    Glyphs { face: FaceId, size: f64, color: [f64; 3], glyphs: Vec<(Glyph, [f64; 2])> },
    /// A filled rectangle `[x, y, w, h]` (underlines, rules).
    Fill { rect: [f64; 4], color: [f64; 3] },
    /// A stroked rectangle (table cells).
    Stroke { rect: [f64; 4], width: f64, color: [f64; 3] },
    /// An image (a container path) in `[x, y, w, h]`.
    Image { path: String, rect: [f64; 4] },
}

/// A laid-out page.
#[derive(Clone, Debug, Default)]
pub struct Page {
    pub size: (f64, f64),
    pub items: Vec<Item>,
    /// Link rectangles `[x0, y0, x1, y1]` and targets (container paths or URLs).
    pub links: Vec<([f64; 4], String)>,
}

/// Glyphs of one face, size and colour, each at (x, rise).
type GlyphRun = (FaceId, f64, [f64; 3], Vec<(Glyph, [f64; 2])>);
/// An underline or strike-through: x, width, offset from the baseline (up), thickness, colour.
type Mark = (f64, f64, f64, f64, [f64; 3]);

/// A line of a paragraph, relative to its own top-left (y down).
#[derive(Clone, Debug, Default)]
struct Line {
    height: f64,
    /// The baseline, from the line's top.
    baseline: f64,
    pieces: Vec<GlyphRun>,
    marks: Vec<Mark>,
    links: Vec<(f64, f64, String)>,
}

/// One shaped piece of a paragraph: glyphs of one face, size and direction.
#[derive(Clone, Debug)]
struct Piece {
    face: FaceId,
    run: usize,
    level: u8,
    glyphs: Vec<Glyph>,
    /// Width in points.
    width: f64,
    /// How many of its glyphs are spaces (justification).
    spaces: usize,
}

/// A segment between two break opportunities.
#[derive(Clone, Debug, Default)]
struct Segment {
    pieces: Vec<Piece>,
    width: f64,
    /// Width of the white space at its end (not counted at the end of a line).
    trailing: f64,
    mandatory: bool,
}

/// Where each id ended up: page index and y (PDF coordinates).
pub type Anchors = HashMap<String, (usize, f64)>;

pub struct Layout<'a> {
    pub fonts: &'a mut Fonts,
    pub pages: Vec<Page>,
    pub anchors: Anchors,
    pub notes: Vec<String>,
    /// The natural size of an image, in points.
    image_size: &'a mut dyn FnMut(&str) -> Option<(f64, f64)>,
    paper: (f64, f64),
    margin: f64,
    /// Distance from the top of the content area.
    y: f64,
    /// The collapsing margin waiting above the next block.
    pending: f64,
    /// Whether anything was placed on the current page.
    used: bool,
}

fn effective_size(run: &Run) -> f64 {
    match run.style.valign {
        VAlign::Baseline => run.style.size,
        _ => run.style.size * 0.75,
    }
}

impl<'a> Layout<'a> {
    pub fn new(fonts: &'a mut Fonts, paper: (f64, f64), image_size: &'a mut dyn FnMut(&str) -> Option<(f64, f64)>) -> Self {
        Layout {
            fonts,
            pages: Vec::new(),
            anchors: HashMap::new(),
            notes: Vec::new(),
            image_size,
            paper,
            margin: MARGIN,
            y: 0.0,
            pending: 0.0,
            used: false,
        }
    }

    fn content(&self) -> (f64, f64) {
        (self.paper.0 - 2.0 * self.margin, self.paper.1 - 2.0 * self.margin)
    }

    /// Start a new page.
    pub fn new_page(&mut self) {
        // A page nothing was drawn on is used again (a chapter whose only picture couldn't be read
        // leaves no blank page); ids that point at it stay valid.
        let reuse = self.pages.last().is_some_and(|p| !self.used && p.items.is_empty() && p.links.is_empty() && p.size == self.paper);
        if !reuse {
            self.pages.push(Page { size: self.paper, ..Page::default() });
        }
        self.y = 0.0;
        self.pending = 0.0;
        self.used = false;
    }

    fn page(&mut self) -> &mut Page {
        if self.pages.is_empty() {
            self.new_page();
        }
        let last = self.pages.len() - 1;
        &mut self.pages[last]
    }

    /// The PDF y of a distance from the content area's top.
    fn pdf_y(&self, y: f64) -> f64 {
        self.paper.1 - self.margin - y
    }

    /// Make room for `h` points (after the pending margin), turning the page when they don't fit.
    fn room(&mut self, h: f64) {
        if self.pages.is_empty() {
            self.new_page();
        }
        let gap = if self.used { self.pending } else { 0.0 };
        if self.used && self.y + gap + h > self.content().1 + 0.01 {
            self.new_page();
        } else {
            self.y += gap;
        }
        self.pending = 0.0;
    }

    fn anchor(&mut self, ids: &[String]) {
        if self.pages.is_empty() {
            self.new_page();
        }
        let at = (self.pages.len() - 1, self.pdf_y(self.y + if self.used { self.pending } else { 0.0 }));
        for id in ids {
            self.anchors.entry(id.clone()).or_insert(at);
        }
    }

    /// Lay out `blocks` in reading order.
    pub fn blocks(&mut self, blocks: &[Block]) {
        for b in blocks {
            self.block(b);
        }
    }

    fn block(&mut self, b: &Block) {
        let (cw, _) = self.content();
        match b {
            Block::Text { runs, style, anchors, .. } => {
                self.anchor(anchors);
                let x0 = style.inset[0] + style.margin[3];
                let width = (cw - x0 - style.inset[1] - style.margin[1]).max(cw * 0.25);
                let lines = self.paragraph(runs, style, width);
                for line in lines {
                    self.room(line.height);
                    let top = self.y;
                    self.place(&line, self.margin + x0, top);
                    self.y += line.height;
                    self.used = true;
                }
            }
            Block::Image { path, style, anchors, .. } => {
                self.pending = self.pending.max(style.margin[0]);
                self.anchor(anchors);
                let Some((nw, nh)) = (self.image_size)(path) else {
                    let webp = path.to_ascii_lowercase().ends_with(".webp");
                    self.notes.push(if webp {
                        format!("{path}: WebP pictures aren't supported yet; it was left out")
                    } else {
                        format!("{path}: the picture could not be read and was left out")
                    });
                    return;
                };
                let (cw, ch) = self.content();
                let x0 = style.inset[0] + style.margin[3];
                let avail = (cw - x0 - style.inset[1] - style.margin[1]).max(cw * 0.25);
                let (mut w, mut h) = match style.size {
                    [Some(w), Some(h)] => (w, h),
                    [Some(w), None] => (w, w * nh / nw.max(0.01)),
                    [None, Some(h)] => (h * nw / nh.max(0.01), h),
                    [None, None] => (nw, nh),
                };
                let scale = (avail / w.max(0.01)).min(ch / h.max(0.01)).min(1.0);
                w *= scale;
                h *= scale;
                self.room(h);
                let x = self.margin
                    + x0
                    + match style.align {
                        Align::Left | Align::Start if w < avail - 1.0 && !style.rtl => (avail - w) / 2.0,
                        Align::Right | Align::End => avail - w,
                        _ => (avail - w) / 2.0,
                    };
                let y = self.pdf_y(self.y + h);
                self.page().items.push(Item::Image { path: path.clone(), rect: [x, y, w, h] });
                self.y += h;
                self.used = true;
                self.pending = style.margin[2];
            }
            Block::Rule { style } => {
                self.pending = self.pending.max(style.margin[0].max(6.0));
                self.room(1.0);
                let x0 = style.inset[0] + style.margin[3];
                let y = self.pdf_y(self.y + 0.5);
                let w = cw - x0 - style.inset[1] - style.margin[1];
                let x = self.margin + x0;
                self.page().items.push(Item::Fill { rect: [x, y, w.max(1.0), 0.75], color: [0.6; 3] });
                self.y += 1.0;
                self.used = true;
                self.pending = style.margin[2].max(6.0);
            }
            Block::Table { rows, style, anchors } => {
                self.anchor(anchors);
                self.table(rows, style);
            }
            Block::PageBreak => {
                if self.used {
                    self.new_page();
                }
            }
            Block::Gap(g) => self.pending = self.pending.max(*g),
            Block::Anchor(ids) => self.anchor(ids),
            Block::Positioned { blocks, .. } => self.blocks(blocks),
        }
    }

    /// Draw `line` with its top-left at (`x`, distance `top` from the content area's top).
    fn place(&mut self, line: &Line, x: f64, top: f64) {
        let base = self.pdf_y(top + line.baseline);
        self.place_at(line, x, base);
    }

    fn place_at(&mut self, line: &Line, x: f64, base: f64) {
        let page = self.page();
        for (face, size, color, glyphs) in &line.pieces {
            let glyphs = glyphs.iter().map(|(g, [gx, rise])| (g.clone(), [x + gx, base + rise])).collect();
            page.items.push(Item::Glyphs { face: *face, size: *size, color: *color, glyphs });
        }
        for (mx, w, off, t, color) in &line.marks {
            page.items.push(Item::Fill { rect: [x + mx, base + off, *w, *t], color: *color });
        }
        for (x0, x1, target) in &line.links {
            page.links.push(([x + x0, base - line.baseline * 0.25, x + x1, base + (line.height - line.baseline)], target.clone()));
        }
    }

    /// Shape `runs` into pieces between break opportunities.
    fn segments(&mut self, runs: &[Run], style: &Style) -> Vec<Segment> {
        let mut text = String::new();
        let mut owner = Vec::new();
        for (i, r) in runs.iter().enumerate() {
            text.push_str(&r.text);
            owner.extend(std::iter::repeat_n(i, r.text.len()));
        }
        // Bidi levels by byte; all left-to-right when nothing asks otherwise.
        let rtl_text = style.rtl || text.chars().any(|c| matches!(u32::from(c), 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF));
        let levels: Vec<u8> = if rtl_text {
            let base = if style.rtl { unicode_bidi::Level::rtl() } else { unicode_bidi::Level::ltr() };
            let info = unicode_bidi::BidiInfo::new(&text, Some(base));
            info.levels.iter().map(|l| l.number()).collect()
        } else {
            vec![0; text.len()]
        };
        let mut out = Vec::new();
        let mut start = 0;
        for (end, op) in unicode_linebreak::linebreaks(&text) {
            let seg_text = &text[start..end];
            let mut seg = Segment { mandatory: op == BreakOpportunity::Mandatory && end < text.len(), ..Segment::default() };
            // Pieces: one face, run and level each.
            let mut piece_start = start;
            let mut current: Option<(FaceId, usize, u8)> = None;
            let mut pieces: Vec<(FaceId, usize, u8, usize, usize)> = Vec::new();
            for (i, c) in seg_text.char_indices() {
                let at = start + i;
                if c == '\n' || c == '\r' || c == '\u{ad}' {
                    if let Some((f, r, l)) = current.take() {
                        pieces.push((f, r, l, piece_start, at));
                    }
                    piece_start = at + c.len_utf8();
                    continue;
                }
                let run = owner[at];
                // Marks and joiners stay with the character before them.
                let face = if current.is_some() && (unicode_mark(c) || matches!(c, '\u{200c}' | '\u{200d}')) {
                    current.map(|x| x.0)
                } else {
                    self.fonts.face(c, &runs[run].style)
                };
                let Some(face) = face else {
                    self.fonts.miss(c);
                    if let Some((f, r, l)) = current.take() {
                        pieces.push((f, r, l, piece_start, at));
                    }
                    piece_start = at + c.len_utf8();
                    continue;
                };
                let key = (face, run, levels[at]);
                if current != Some(key) {
                    if let Some((f, r, l)) = current {
                        pieces.push((f, r, l, piece_start, at));
                    }
                    current = Some(key);
                    piece_start = at;
                }
            }
            if let Some((f, r, l)) = current {
                pieces.push((f, r, l, piece_start, end));
            }
            for (face, run, level, a, b) in pieces {
                let t = &text[a..b];
                if t.is_empty() {
                    continue;
                }
                let size = effective_size(&runs[run]);
                let glyphs = self.fonts.shape(face, t, level % 2 == 1);
                let width = glyphs.iter().map(|g| g.advance).sum::<f64>() * size;
                let spaces = glyphs.iter().filter(|g| g.text == " " || g.text == "\u{a0}").count();
                seg.width += width;
                seg.pieces.push(Piece { face, run, level, glyphs, width, spaces });
            }
            let trimmed = seg_text.trim_end_matches([' ', '\n', '\r', '\u{3000}']);
            if trimmed.len() < seg_text.len() && !style.pre {
                // The trailing spaces' width: shape them alone in the last piece's face.
                if let Some(last) = seg.pieces.last() {
                    let size = effective_size(&runs[last.run]);
                    let n = seg_text[trimmed.len()..].chars().filter(|c| *c == ' ' || *c == '\u{3000}').count();
                    let one = self.fonts.shape(last.face, " ", false).first().map_or(0.25, |g| g.advance);
                    seg.trailing = (one * size * n as f64).min(seg.width);
                }
            }
            out.push(seg);
            start = end;
        }
        out
    }

    /// Set a paragraph in lines of `width` points.
    fn paragraph(&mut self, runs: &[Run], style: &Style, width: f64) -> Vec<Line> {
        let segments = self.segments(runs, style);
        let mut lines: Vec<Vec<Segment>> = vec![Vec::new()];
        let mut used = 0.0;
        let mut forced = vec![false];
        for seg in segments {
            let avail = width - if lines.len() == 1 { style.indent.max(0.0) } else { 0.0 };
            let fits = used + seg.width - seg.trailing <= avail + 0.01;
            if !fits && !lines.last().is_none_or(Vec::is_empty) {
                lines.push(Vec::new());
                forced.push(false);
                used = 0.0;
            }
            if seg.width - seg.trailing > avail + 0.01 && lines.last().is_some_and(Vec::is_empty) {
                // Longer than a whole line: split it glyph by glyph.
                for part in split(seg, avail) {
                    if !lines.last().is_none_or(Vec::is_empty) {
                        lines.push(Vec::new());
                        forced.push(false);
                    }
                    used = part.width;
                    let m = part.mandatory;
                    if let Some(l) = lines.last_mut() {
                        l.push(part);
                    }
                    if m {
                        if let Some(f) = forced.last_mut() {
                            *f = true;
                        }
                        lines.push(Vec::new());
                        forced.push(false);
                        used = 0.0;
                    }
                }
                continue;
            }
            used += seg.width;
            let m = seg.mandatory;
            if let Some(l) = lines.last_mut() {
                l.push(seg);
            }
            if m {
                if let Some(f) = forced.last_mut() {
                    *f = true;
                }
                lines.push(Vec::new());
                forced.push(false);
                used = 0.0;
            }
        }
        if lines.last().is_some_and(Vec::is_empty) && lines.len() > 1 {
            lines.pop();
            forced.pop();
        }
        let n = lines.len();
        lines.into_iter().enumerate().map(|(i, segs)| self.line(runs, style, segs, width, i == 0, i + 1 == n || forced[i])).collect()
    }

    fn line(&mut self, runs: &[Run], style: &Style, segs: Vec<Segment>, width: f64, first: bool, last: bool) -> Line {
        let indent = if first { style.indent } else { 0.0 };
        let avail = width - indent.max(0.0);
        let trailing = segs.last().map_or(0.0, |s| s.trailing);
        let mut pieces: Vec<Piece> = segs.into_iter().flat_map(|s| s.pieces).collect();
        // Visual order (UAX #9 L2): reverse runs of pieces at each level from the highest down.
        let max = pieces.iter().map(|p| p.level).max().unwrap_or(0);
        let min_odd = pieces.iter().map(|p| p.level).filter(|l| l % 2 == 1).min().unwrap_or(max + 1);
        let mut level = max;
        while level >= min_odd && level > 0 {
            let mut i = 0;
            while i < pieces.len() {
                if pieces[i].level >= level {
                    let j = (i..pieces.len()).find(|&k| pieces[k].level < level).unwrap_or(pieces.len());
                    pieces[i..j].reverse();
                    i = j;
                } else {
                    i += 1;
                }
            }
            level -= 1;
        }
        let natural: f64 = pieces.iter().map(|p| p.width).sum::<f64>() - trailing;
        let max_size = pieces.iter().map(|p| effective_size(&runs[p.run])).fold(0.0, f64::max).max(runs.first().map_or(12.0, |r| r.style.size) * 0.5);
        let height = (max_size * style.line_height).max(max_size);
        let baseline = (height - max_size) / 2.0 + max_size * 0.8;
        let rtl = style.rtl;
        let align = match (style.align, rtl) {
            (Align::Start, false) | (Align::End, true) | (Align::Left, _) => Align::Left,
            (Align::Start, true) | (Align::End, false) | (Align::Right, _) => Align::Right,
            (a, _) => a,
        };
        let spaces: usize = pieces.iter().map(|p| p.spaces).sum::<usize>().saturating_sub(usize::from(trailing > 0.0));
        let stretch = if align == Align::Justify && !last && spaces > 0 && natural < avail { (avail - natural) / spaces as f64 } else { 0.0 };
        let mut x = indent.max(0.0)
            + match align {
                Align::Right => (avail - natural).max(0.0),
                Align::Center => ((avail - natural) / 2.0).max(0.0),
                _ => 0.0,
            };
        if rtl && indent > 0.0 && align != Align::Left {
            x -= indent;
        }
        let mut line = Line { height, baseline, ..Line::default() };
        for p in pieces {
            let run = &runs[p.run];
            let size = effective_size(run);
            let rise = match run.style.valign {
                VAlign::Super => run.style.size * 0.33,
                VAlign::Sub => -run.style.size * 0.2,
                VAlign::Baseline => 0.0,
            };
            let start = x;
            let mut glyphs = Vec::with_capacity(p.glyphs.len());
            for g in p.glyphs {
                let gx = x + g.offset[0] * size;
                let gy = rise + g.offset[1] * size;
                x += g.advance * size;
                if stretch > 0.0 && (g.text == " " || g.text == "\u{a0}") {
                    x += stretch;
                }
                glyphs.push((g, [gx, gy]));
            }
            let w = x - start;
            if run.style.underline {
                line.marks.push((start, w, rise - size * 0.12, (size * 0.06).max(0.4), run.style.color));
            }
            if run.style.strike {
                line.marks.push((start, w, rise + size * 0.28, (size * 0.06).max(0.4), run.style.color));
            }
            if let Some(target) = &run.link {
                match line.links.last_mut() {
                    Some(l) if l.2 == *target && (l.1 - start).abs() < 0.5 => l.1 = x,
                    _ => line.links.push((start, x, target.clone())),
                }
            }
            match line.pieces.last_mut() {
                Some(last) if last.0 == p.face && last.1 == size && last.2 == run.style.color => last.3.extend(glyphs),
                _ => line.pieces.push((p.face, size, run.style.color, glyphs)),
            }
        }
        line
    }

    fn table(&mut self, rows: &[Vec<Vec<Block>>], style: &Style) {
        let (cw, ch) = self.content();
        let x0 = style.inset[0] + style.margin[3];
        let width = (cw - x0 - style.inset[1] - style.margin[1]).max(cw * 0.25);
        let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
        let col_w = width / cols as f64;
        let pad = 3.0;
        self.pending = self.pending.max(6.0);
        for row in rows {
            // Each cell's lines.
            let mut cells: Vec<Vec<Line>> = Vec::new();
            for cell in row {
                let mut lines = Vec::new();
                for b in cell {
                    if let Block::Text { runs, style, .. } = b {
                        lines.extend(self.paragraph(runs, style, (col_w - 2.0 * pad).max(6.0)));
                    }
                }
                cells.push(lines);
            }
            let h = cells.iter().map(|ls| ls.iter().map(|l| l.height).sum::<f64>()).fold(0.0, f64::max) + 2.0 * pad;
            if h > ch {
                // Taller than a page: the cells' text one after another.
                self.notes.push("a table row taller than a page was set as plain paragraphs".into());
                for lines in cells {
                    for line in lines {
                        self.room(line.height);
                        let top = self.y;
                        self.place(&line, self.margin + x0, top);
                        self.y += line.height;
                        self.used = true;
                    }
                }
                continue;
            }
            self.room(h);
            let top = self.y;
            for (i, lines) in cells.iter().enumerate() {
                let cx = self.margin + x0 + i as f64 * col_w;
                let mut y = top + pad;
                for line in lines {
                    self.place(line, cx + pad, y);
                    y += line.height;
                }
                let rect = [cx, self.pdf_y(top + h), col_w, h];
                self.page().items.push(Item::Stroke { rect, width: 0.5, color: [0.55; 3] });
            }
            self.y += h;
            self.used = true;
        }
        self.pending = 6.0;
    }

    /// A fixed-layout document: one page of the viewport's size (`viewport` in CSS pixels). A
    /// page that is one picture fills it; positioned boxes go where they say; anything else is
    /// reflowed on pages of that size (noted).
    pub fn fixed(&mut self, blocks: &[Block], viewport: (f64, f64), name: &str) {
        let size = (viewport.0 * PT_PER_PX, viewport.1 * PT_PER_PX);
        let saved = (self.paper, self.margin);
        self.paper = (size.0.clamp(72.0, 14_400.0), size.1.clamp(72.0, 14_400.0));
        self.margin = 0.0;
        self.new_page();
        let content: Vec<&Block> = blocks.iter().filter(|b| !matches!(b, Block::Gap(_) | Block::Anchor(_) | Block::PageBreak)).collect();
        let ids: Vec<String> = blocks
            .iter()
            .flat_map(|b| match b {
                Block::Anchor(ids) => ids.clone(),
                Block::Text { anchors, .. } | Block::Image { anchors, .. } | Block::Table { anchors, .. } => anchors.clone(),
                _ => Vec::new(),
            })
            .collect();
        self.anchor(&ids);
        if let [Block::Image { path, .. }] = content.as_slice() {
            if let Some((w, h)) = (self.image_size)(path) {
                let (pw, ph) = self.paper;
                let s = (pw / w.max(0.01)).min(ph / h.max(0.01));
                let (w, h) = (w * s, h * s);
                self.page().items.push(Item::Image { path: path.clone(), rect: [(pw - w) / 2.0, (ph - h) / 2.0, w, h] });
            }
        } else if content.iter().all(|b| matches!(b, Block::Positioned { .. } | Block::Image { .. })) && !content.is_empty() {
            for b in content {
                match b {
                    Block::Positioned { rect, blocks } => self.positioned(*rect, blocks),
                    Block::Image { path, style, .. } => {
                        let [x, y] = [style.absolute.and_then(|a| a[0]).unwrap_or(0.0), style.absolute.and_then(|a| a[1]).unwrap_or(0.0)];
                        let (pw, ph) = self.paper;
                        let natural = (self.image_size)(path).unwrap_or((pw, ph));
                        let (w, h) = (style.size[0].unwrap_or(natural.0), style.size[1].unwrap_or(natural.1));
                        self.page().items.push(Item::Image { path: path.clone(), rect: [x, ph - y - h, w, h] });
                    }
                    _ => {}
                }
            }
        } else {
            self.notes.push(format!("{name}: a fixed-layout page that is not one picture or positioned text was reflowed on pages of its size"));
            self.margin = 18.0;
            self.blocks(blocks);
        }
        self.paper = saved.0;
        self.margin = saved.1;
        self.used = true;
    }

    /// A positioned box: its paragraphs set from (left, top) in its width, without turning pages.
    fn positioned(&mut self, rect: [Option<f64>; 4], blocks: &[Block]) {
        let (pw, ph) = self.paper;
        let left = rect[0].unwrap_or(0.0);
        let mut top = rect[1].unwrap_or(0.0);
        let width = rect[2].unwrap_or(pw - left).max(12.0);
        for b in blocks {
            match b {
                Block::Text { runs, style, .. } => {
                    for line in self.paragraph(runs, style, width) {
                        let base = ph - top - line.baseline;
                        self.place_at(&line, left, base);
                        top += line.height;
                    }
                }
                Block::Image { path, style, .. } => {
                    if let Some((w, h)) = (self.image_size)(path) {
                        let (w, h) = (style.size[0].unwrap_or(w), style.size[1].unwrap_or(h));
                        self.page().items.push(Item::Image { path: path.clone(), rect: [left, ph - top - h, w, h] });
                        top += h;
                    }
                }
                Block::Positioned { rect, blocks } => self.positioned(*rect, blocks),
                _ => {}
            }
        }
    }
}

/// Whether `c` is a combining mark (it joins the character before it).
fn unicode_mark(c: char) -> bool {
    matches!(u32::from(c), 0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05C7 | 0x0610..=0x061A | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06ED
        | 0x0900..=0x0903 | 0x093A..=0x094F | 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

/// Split a segment wider than `width` into parts that fit (at glyph boundaries).
fn split(seg: Segment, width: f64) -> Vec<Segment> {
    let mut out: Vec<Segment> = vec![Segment::default()];
    let mandatory = seg.mandatory;
    for p in seg.pieces {
        let per_em = if p.glyphs.is_empty() { 0.0 } else { p.width / p.glyphs.iter().map(|g| g.advance).sum::<f64>().max(1e-9) };
        let empty = Piece { glyphs: Vec::new(), width: 0.0, spaces: 0, ..p.clone() };
        let mut current = empty.clone();
        for g in p.glyphs {
            let w = g.advance * per_em;
            let used = out.last().map_or(0.0, |s| s.width) + current.width;
            if used + w > width && (current.width > 0.0 || out.last().is_some_and(|s| s.width > 0.0)) && !g.text.is_empty() {
                if let Some(last) = out.last_mut() {
                    last.width += current.width;
                    last.pieces.push(std::mem::replace(&mut current, empty.clone()));
                }
                out.push(Segment::default());
            }
            current.width += w;
            current.glyphs.push(g);
        }
        if let Some(last) = out.last_mut() {
            last.width += current.width;
            last.pieces.push(current);
        }
    }
    for s in &mut out {
        s.pieces.retain(|p| !p.glyphs.is_empty());
    }
    out.retain(|s| !s.pieces.is_empty());
    if let Some(last) = out.last_mut() {
        last.mandatory = mandatory;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::dom::{RunStyle, Style};
    use super::*;

    fn run(text: &str) -> Run {
        Run { text: text.into(), style: Style::default().run, link: None }
    }

    fn sizes(_: &str) -> Option<(f64, f64)> {
        Some((300.0, 150.0))
    }

    fn texts(page: &Page) -> Vec<String> {
        page.items
            .iter()
            .filter_map(|i| match i {
                Item::Glyphs { glyphs, .. } => Some(glyphs.iter().map(|(g, _)| g.text.as_str()).collect::<String>()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn paragraphs_wrap_justify_and_turn_pages() {
        let mut fonts = Fonts::new();
        let mut size = sizes;
        let mut l = Layout::new(&mut fonts, (300.0, 200.0), &mut size);
        let words = "The quick brown fox jumps over the lazy dog. ".repeat(20);
        let style = Style { align: Align::Justify, ..Style::default() };
        l.blocks(&[Block::Text { runs: vec![run(&words)], style, anchors: vec!["c.xhtml#p".into()], heading: None }]);
        assert!(l.pages.len() > 2, "{} pages", l.pages.len());
        assert_eq!(l.anchors.get("c.xhtml#p").map(|a| a.0), Some(0));
        // Every line inside the text width; justified lines end at the right edge.
        let right = 300.0 - MARGIN;
        let mut full = 0;
        for page in &l.pages {
            for item in &page.items {
                let Item::Glyphs { glyphs, size, .. } = item else { continue };
                let (g, [x, _]) = glyphs.iter().rev().find(|(g, _)| g.text.trim() != "").unwrap();
                let end = x + g.advance * size;
                assert!(end <= right + 0.5, "{end}");
                if (end - right).abs() < 0.5 {
                    full += 1;
                }
            }
        }
        assert!(full > 5, "justified lines reach the margin ({full})");
        let all: String = l.pages.iter().flat_map(texts).collect::<Vec<_>>().join(" ");
        assert_eq!(all.split_whitespace().count(), words.split_whitespace().count(), "no word lost");
    }

    #[test]
    fn right_to_left_text_runs_from_the_right() {
        let mut fonts = Fonts::new();
        let mut size = sizes;
        let mut l = Layout::new(&mut fonts, (400.0, 400.0), &mut size);
        let style = Style { rtl: true, ..Style::default() };
        // Hebrew needs a face; without craft-fonts it is noted as missing, and the Latin part
        // still lines up at the right.
        l.blocks(&[Block::Text { runs: vec![run("abc")], style, anchors: vec![], heading: None }]);
        let Item::Glyphs { glyphs, size, .. } = &l.pages[0].items[0] else { panic!() };
        let (g, [x, _]) = glyphs.last().unwrap();
        assert!((x + g.advance * size - (400.0 - MARGIN)).abs() < 0.5);
    }

    #[test]
    fn images_scale_to_the_width_and_tables_get_cells() {
        let mut fonts = Fonts::new();
        let mut size = |_: &str| Some((1200.0, 600.0));
        let mut l = Layout::new(&mut fonts, (612.0, 792.0), &mut size);
        let cell = |t: &str| vec![Block::Text { runs: vec![run(t)], style: Style::default(), anchors: vec![], heading: None }];
        l.blocks(&[
            Block::Image { path: "a.png".into(), alt: String::new(), style: Style::default(), anchors: vec![] },
            Block::Table { rows: vec![vec![cell("a"), cell("b")], vec![cell("c")]], style: Style::default(), anchors: vec![] },
        ]);
        let Item::Image { rect, .. } = &l.pages[0].items[0] else { panic!() };
        assert!((rect[2] - (612.0 - 2.0 * MARGIN)).abs() < 0.01 && (rect[3] - rect[2] / 2.0).abs() < 0.01);
        assert_eq!(l.pages[0].items.iter().filter(|i| matches!(i, Item::Stroke { .. })).count(), 3);
        let _ = RunStyle::clone(&Style::default().run);
    }

    #[test]
    fn a_chapter_that_draws_nothing_leaves_no_blank_page() {
        let mut fonts = Fonts::new();
        let mut size = |_: &str| None;
        let mut l = Layout::new(&mut fonts, (612.0, 792.0), &mut size);
        l.new_page();
        l.blocks(&[Block::Image { path: "cover.webp".into(), alt: String::new(), style: Style::default(), anchors: vec!["cover.xhtml".into()] }]);
        l.new_page();
        l.blocks(&[Block::Text { runs: vec![run("Chapter")], style: Style::default(), anchors: vec!["c1.xhtml".into()], heading: None }]);
        assert_eq!(l.pages.len(), 1);
        assert_eq!((l.anchors["cover.xhtml"].0, l.anchors["c1.xhtml"].0), (0, 0));
        assert!(l.notes[0].contains("WebP"), "{:?}", l.notes);
    }

    #[test]
    fn a_fixed_layout_picture_fills_its_page() {
        let mut fonts = Fonts::new();
        let mut size = |_: &str| Some((600.0, 800.0));
        let mut l = Layout::new(&mut fonts, (612.0, 792.0), &mut size);
        l.fixed(&[Block::Image { path: "p.jpg".into(), alt: String::new(), style: Style::default(), anchors: vec![] }], (1200.0, 1600.0), "p1.xhtml");
        assert_eq!(l.pages[0].size, (900.0, 1200.0));
        let Item::Image { rect, .. } = &l.pages[0].items[0] else { panic!() };
        assert_eq!(*rect, [0.0, 0.0, 900.0, 1200.0]);
    }
}
