//! Synthesis: a page's typed regions turned into the document structure an editable copy is
//! built from.
//!
//! [`crate::page_layout`] answers what each block of the page is; synthesis answers what the
//! document's paragraphs are: their kind (body text, a numbered item, a point label, a heading,
//! a caption, a signature, an exhibit label, a running header) and their text — with a typed
//! list number stripped, so a writer can make a real numbered list instead of printing `1.`.
//!
//! Geometry is carried from the lines, in their own pixels: an indent is a distance from the
//! page's body margin, and a line spacing is a multiple of the line's height, so both survive a
//! change of resolution.
//!
//! The rules are deterministic; the reference implementation scores some of these decisions with
//! trained models, and ours are rules instead:
//!
//! - a line opening `1. ` is a numbered item, and the typed prefix is stripped;
//! - `POINT II` opens a point, and the all-caps line after it is that point's heading; point
//!   headings are lettered A, B, C … as siblings of one list, not one list per heading;
//! - an all-caps line at least 1.2 times the body's line height (`HEADING_SCALE`), centred in
//!   the text column, is a section heading, and adjacent heading lines of one size are one
//!   heading (a heading that wrapped);
//! - a line carrying `Dated:`, `Sworn to` or `By:` is a signature, and an `Exhibit A` label is
//!   an exhibit; a header or footer region is a header/footer block and a caption region is a
//!   caption;
//! - a paragraph whose lines all start with a number is several list items, not one: a numbered
//!   line never merges into the item above it, and an item only absorbs lines that do not
//!   number.
//!
//! Reading order is the regions' own [`Region::order`]. A caller that wants one lettered list
//! across a document passes the document's regions in order, so the letters run on.

use crate::Line;
use crate::page_layout::{Region, RegionKind, is_exhibit_label, is_list_item, is_signature_line};

/// The decimal numbering definition a [`ParagraphKind::Numbered`] item uses: arabic numerals
/// (1, 2, 3 …), which is how legal documents number their paragraphs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Decimal;

/// What a synthesized paragraph is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParagraphKind {
    /// Flowing body text: the default for everything no other rule claims.
    Body,
    /// A numbered list item in the [`Decimal`] scheme; its number is on [`Paragraph::number`].
    Numbered(Decimal),
    /// A point's label (`POINT II`), which opens the point whose heading follows it.
    PointLabel,
    /// A point's heading, lettered as one sibling list across the paragraphs given (A, B, C …).
    PointHeading { letter: char },
    /// A section heading; `level` is its document level (2 is the legal exemplars' Heading 2).
    SectionHeading { level: u8 },
    /// The party/caption block at the top of a legal page.
    Caption,
    /// A signature block: a `Dated:`, `Sworn to` or `By:` line.
    Signature,
    /// An exhibit label, or the region an exhibit label heads.
    Exhibit,
    /// A running header or footer.
    HeaderFooterBlock,
}

/// One paragraph of the synthesized document.
#[derive(Clone, Debug, PartialEq)]
pub struct Paragraph {
    pub kind: ParagraphKind,
    /// The paragraph's text: its lines joined, a numbered item's typed prefix removed.
    pub text: String,
    /// A numbered item's number; `None` for every other kind.
    pub number: Option<u32>,
    /// Whether the paragraph is set in bold. A recogniser reports no font weight, so this is the
    /// emphasis the kind implies: headings and point labels are bold (the capitals and the size
    /// are what the reading can see of it), everything else is not.
    pub bold: bool,
    /// The paragraph's left edge as a distance from the page's body margin, in the lines' pixels.
    pub indent_left: f32,
    /// The first line's extra indent on top of [`Paragraph::indent_left`]; negative for a
    /// hanging indent (a list marker outdented from the wrapped text).
    pub indent_first: f32,
    /// The distance between the paragraph's line tops divided by its line height: 1.0 is single
    /// spacing and 2.0 double. A one-line paragraph reports 1.0, the spacing it cannot show.
    pub line_spacing: f32,
}

/// A heading's line height relative to the body's: at least this much taller, which is the
/// reference behaviour's "bigger than body text" test (1.2).
const HEADING_SCALE: f32 = 1.2;

/// Two lines are set in one size when their heights agree within 5 % of their mean — the
/// reference behaviour's `|h1 − h2| × 40 ≤ h1 + h2`.
const HEIGHT_AGREEMENT: f32 = 40.0;

/// A line is centred when its centre is within this share of the text column's width from the
/// column's centre.
const CENTRE_SHARE: f32 = 0.10;

/// A line ending more than this many line heights before the column's right edge ends its
/// paragraph: the next line opens a new one.
const SHORT_LINE: f32 = 0.75;

/// Lines more than this many line heights apart are separate paragraphs even when their kinds
/// would merge (a blank line between them).
const LINE_GAP: f32 = 1.2;

/// Synthesizes `regions` — one page's, or a document's in reading order — into paragraphs.
pub fn synthesize(regions: &[Region]) -> Vec<Paragraph> {
    synthesize_with_rects(regions).into_iter().map(|(paragraph, _)| paragraph).collect()
}

/// [`synthesize`], with each paragraph's source region's box. Every paragraph comes from
/// exactly one region (flowing merges lines within a region, never across regions), so the
/// box is exact — an exporter need not guess geometry by zipping indices, which scrambles
/// reading order as soon as two regions merge or split.
pub fn synthesize_with_rects(regions: &[Region]) -> Vec<(Paragraph, crate::page_layout::Rect)> {
    let mut ordered: Vec<&Region> = regions.iter().collect();
    ordered.sort_by_key(|region| region.order);
    let lines: Vec<Vec<Placed>> = ordered.iter().map(|region| placed_lines(&region.lines)).collect();
    let frame = Frame::of(&ordered, &lines);
    let mut builder = Builder {
        frame: &frame,
        rect: crate::page_layout::Rect::new(0.0, 0.0, 0.0, 0.0),
        out: Vec::new(),
        rects: Vec::new(),
        after_point_label: false,
        sections: Vec::new(),
    };
    // Flowing text is grouped across region boundaries: layout splits regions on gutters and
    // width changes, and treating each as its own paragraph shatters wrapped body text into
    // one-line fragments. Block regions (headers, captions, signatures, exhibits) still end the
    // flowing run before them.
    let mut pending: Vec<Placed> = Vec::new();
    let mut pending_rect: Option<crate::page_layout::Rect> = None;
    for (region, placed) in ordered.iter().zip(&lines) {
        if block_kind(region.kind).is_some() {
            if !pending.is_empty() {
                builder.rect = pending_rect.take().unwrap_or(region.rect);
                builder.flowing(&pending);
                pending.clear();
            }
            builder.rect = region.rect;
            builder.add_region(region.kind, placed);
        } else {
            pending.extend(placed.iter().cloned());
            pending_rect = Some(match pending_rect {
                Some(rect) => rect.union(region.rect),
                None => region.rect,
            });
        }
    }
    if !pending.is_empty() {
        if let Some(rect) = pending_rect {
            builder.rect = rect;
        }
        builder.flowing(&pending);
    }
    let mut paragraphs = builder.out;
    assign_levels(&mut paragraphs, &builder.sections);
    letter_points(&mut paragraphs);
    paragraphs.into_iter().zip(builder.rects).collect()
}

/// One line of a page: its text and the box its words occupy.
#[derive(Clone, Debug, PartialEq)]
struct Placed {
    text: String,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl Placed {
    /// The line's text and box; `None` for a line with no usable word (no text, or a box that is
    /// not a box: those are left out rather than allowed to poison the page's geometry).
    fn of(line: &Line) -> Option<Placed> {
        let text = line.text();
        if text.trim().is_empty() {
            return None;
        }
        let mut placed = Placed { text, left: f32::INFINITY, top: f32::INFINITY, right: f32::NEG_INFINITY, bottom: f32::NEG_INFINITY };
        for word in &line.words {
            let [left, top, right, bottom] = word.rect;
            if ![left, top, right, bottom].iter().all(|value| value.is_finite()) {
                continue;
            }
            placed.left = placed.left.min(left);
            placed.top = placed.top.min(top);
            placed.right = placed.right.max(right);
            placed.bottom = placed.bottom.max(bottom);
        }
        (placed.right > placed.left && placed.bottom > placed.top).then_some(placed)
    }

    fn height(&self) -> f32 {
        self.bottom - self.top
    }

    fn centre(&self) -> f32 {
        (self.left + self.right) / 2.0
    }
}

/// The usable lines of a region, with their boxes measured from their words.
fn placed_lines(lines: &[Line]) -> Vec<Placed> {
    lines.iter().filter_map(Placed::of).collect()
}

/// The page's body text column and its dominant line height, measured from the flowing text.
struct Frame {
    body_height: f32,
    left: f32,
    right: f32,
}

impl Frame {
    /// The frame of `regions`, from the text regions' lines where there are any (a signature or
    /// caption block can sit indented, and a header or footer need not sit in the column at all).
    fn of(regions: &[&Region], lines: &[Vec<Placed>]) -> Frame {
        let mut body: Vec<&Placed> = Vec::new();
        let mut other: Vec<&Placed> = Vec::new();
        for (region, placed) in regions.iter().zip(lines) {
            if matches!(region.kind, RegionKind::Header | RegionKind::Footer) {
                continue;
            }
            let flowing = matches!(region.kind, RegionKind::Text | RegionKind::ListItem | RegionKind::Table | RegionKind::Other);
            for line in placed {
                other.push(line);
                if flowing {
                    body.push(line);
                }
            }
        }
        let measured = if body.is_empty() { other } else { body };
        let heights: Vec<f32> = measured.iter().map(|line| line.height()).collect();
        Frame {
            body_height: median(&heights),
            left: measured.iter().map(|line| line.left).reduce(f32::min).unwrap_or(0.0),
            right: measured.iter().map(|line| line.right).reduce(f32::max).unwrap_or(0.0),
        }
    }

    fn width(&self) -> f32 {
        (self.right - self.left).max(0.0)
    }

    fn centre(&self) -> f32 {
        (self.left + self.right) / 2.0
    }

    /// Whether the line is set larger than the body text.
    fn enlarged(&self, line: &Placed) -> bool {
        self.body_height > 0.0 && line.height() >= HEADING_SCALE * self.body_height
    }

    /// Whether the line is centred in the text column.
    fn centred(&self, line: &Placed) -> bool {
        self.width() > 0.0 && (line.centre() - self.centre()).abs() <= CENTRE_SHARE * self.width()
    }

    /// Whether the line ends its paragraph: it stops well short of the column's right edge.
    /// The slack is a SHARE of the column, not a fixed number of points: recognised boxes vary
    /// by more than a fixed slack allows, and a fixed one makes every wrapped line look final.
    fn short(&self, line: &Placed) -> bool {
        self.width() > 0.0 && line.right < self.left + SHORT_LINE * self.width()
    }
}

/// The middle of `values` (the mean of the two middles for an even count); 0.0 for none.
fn median(values: &[f32]) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    let len = sorted.len();
    if len == 0 {
        return 0.0;
    }
    if len % 2 == 1 {
        return sorted.get(len / 2).copied().unwrap_or(0.0);
    }
    let lower = sorted.get(len / 2 - 1).copied().unwrap_or(0.0);
    let upper = sorted.get(len / 2).copied().unwrap_or(0.0);
    (lower + upper) / 2.0
}

/// What a line is on its own, before lines are grouped into paragraphs.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Role {
    Numbered(u32),
    PointLabel,
    Signature,
    Exhibit,
    /// An enlarged, centred, all-caps heading.
    Heading,
    /// All caps and longer than one word, but not enlarged: a heading set in body size, or a
    /// label. Body text never merges into it.
    Label,
    Plain,
}

/// What `line` is, by its text and then by its geometry against the page's frame.
fn role_of(line: &Placed, frame: &Frame) -> Role {
    if let Some((number, _)) = decimal_marker(&line.text) {
        return Role::Numbered(number);
    }
    if point_label(&line.text) {
        return Role::PointLabel;
    }
    if is_signature_line(&line.text) {
        return Role::Signature;
    }
    if is_exhibit_label(line.text.trim_start()) {
        return Role::Exhibit;
    }
    if !heading_shaped(&line.text) {
        return Role::Plain;
    }
    if frame.enlarged(line) && frame.centred(line) { Role::Heading } else { Role::Label }
}

/// Whether `line` continues the paragraph `prev` belongs to.
fn continues(prev: &Placed, line: &Placed, frame: &Frame) -> bool {
    match (role_of(prev, frame), role_of(line, frame)) {
        (Role::Numbered(_) | Role::Plain, Role::Plain) => !frame.short(prev) && close(prev, line),
        (Role::Heading | Role::Label, Role::Heading | Role::Label) => {
            !is_list_item(&prev.text) && !is_list_item(&line.text) && font_agrees(prev.height(), line.height()) && close(prev, line)
        }
        _ => false,
    }
}

/// Whether two lines are a line spacing apart: close enough to be one paragraph (a blank line
/// between them opens a new one).
fn close(prev: &Placed, line: &Placed) -> bool {
    let height = line.height();
    let gap = line.top - prev.bottom;
    height > 0.0 && gap >= -height && gap <= LINE_GAP * height
}

/// Whether two line heights are one font's: within 5 % of their mean.
fn font_agrees(first: f32, second: f32) -> bool {
    (first - second).abs() * HEIGHT_AGREEMENT <= first + second
}

/// Whether a line reads as a heading or a label rather than a sentence: every letter is upper
/// case and it is longer than one word. A recogniser reports no font weight, so capitals are the
/// emphasis an all-caps line was set in.
fn heading_shaped(text: &str) -> bool {
    let mut letters = 0usize;
    for c in text.chars() {
        if c.is_alphabetic() {
            letters += 1;
            if !c.is_uppercase() {
                return false;
            }
        }
    }
    letters > 0 && text.split_whitespace().count() >= 2
}

/// The number and the remaining text of a line opening with a typed decimal list marker
/// (`12. `): the prefix a real numbering definition replaces.
fn decimal_marker(text: &str) -> Option<(u32, &str)> {
    let text = text.trim_start();
    let digits = text.chars().take_while(|c| c.is_ascii_digit()).count();
    let number: u32 = text.get(..digits)?.parse().ok()?;
    let after = text.get(digits..)?.strip_prefix('.')?;
    let body = after.trim_start();
    (body.len() < after.len() && !body.is_empty()).then_some((number, body))
}

/// Whether a line opens with a point label: `POINT` and a roman numeral (`POINT II`).
fn point_label(text: &str) -> bool {
    let mut words = text.split_whitespace();
    let Some(head) = words.next() else { return false };
    let Some(numeral) = words.next() else { return false };
    head.eq_ignore_ascii_case("POINT") && is_roman_number(numeral.trim_end_matches(['.', ':', ',']))
}

/// Whether `token` is a roman numeral up to 999, which is how points are numbered.
fn is_roman_number(token: &str) -> bool {
    let token = token.to_ascii_uppercase();
    !token.is_empty() && (1..=999).any(|n| to_roman(n) == token)
}

/// `n` in roman numerals.
fn to_roman(mut n: u32) -> String {
    const STEPS: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (value, glyph) in STEPS {
        while n >= value {
            out.push_str(glyph);
            n -= value;
        }
    }
    out
}

/// The paragraph kind a whole region's lines take, when the region is a block rather than
/// flowing text.
fn block_kind(kind: RegionKind) -> Option<ParagraphKind> {
    match kind {
        RegionKind::Header | RegionKind::Footer => Some(ParagraphKind::HeaderFooterBlock),
        RegionKind::Caption => Some(ParagraphKind::Caption),
        RegionKind::Signature => Some(ParagraphKind::Signature),
        RegionKind::Exhibit => Some(ParagraphKind::Exhibit),
        RegionKind::Text | RegionKind::ListItem | RegionKind::Table | RegionKind::Picture | RegionKind::Other => None,
    }
}

/// A run's lines as one paragraph's text.
fn run_text(lines: &[Placed]) -> String {
    lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join(" ")
}

/// A numbered item's text with the typed number removed from its first line: the number is
/// carried by [`Paragraph::number`], not printed into the text.
fn numbered_text(lines: &[Placed]) -> String {
    let Some(first) = lines.first() else { return String::new() };
    let mut out = decimal_marker(&first.text).map_or_else(|| first.text.clone(), |(_, body)| body.to_string());
    for line in lines.get(1..).unwrap_or_default() {
        out.push(' ');
        out.push_str(&line.text);
    }
    out
}

/// The paragraphs being built, in reading order, and the state the next line needs.
struct Builder<'a> {
    frame: &'a Frame,
    rect: crate::page_layout::Rect,
    rects: Vec<crate::page_layout::Rect>,
    out: Vec<Paragraph>,
    /// The paragraph just built was a point label, so the next heading is that point's heading.
    after_point_label: bool,
    /// The section headings and the line height each was recognised at, for level assignment.
    sections: Vec<(usize, f32)>,
}

impl Builder<'_> {
    /// Adds one region's lines: a block region line by line, a flowing one grouped into
    /// paragraphs.
    fn add_region(&mut self, kind: RegionKind, lines: &[Placed]) {
        let Some(block) = block_kind(kind) else {
            self.flowing(lines);
            return;
        };
        // A block ends whatever point label came before it, even when it holds no lines.
        self.after_point_label = false;
        for line in lines {
            self.push(block, line.text.clone(), None, std::slice::from_ref(line));
        }
    }

    /// Groups flowing lines into paragraphs: a line continues the paragraph before it when
    /// [`continues`] says so.
    fn flowing(&mut self, lines: &[Placed]) {
        let mut start = 0usize;
        for index in 1..lines.len() {
            let (Some(prev), Some(line)) = (lines.get(index - 1), lines.get(index)) else { continue };
            if !continues(prev, line, self.frame) {
                self.run(lines, start, index);
                start = index;
            }
        }
        self.run(lines, start, lines.len());
    }

    /// Pushes the paragraph the lines `start..end` make.
    fn run(&mut self, lines: &[Placed], start: usize, end: usize) {
        let Some(run) = lines.get(start..end) else { return };
        let Some(first) = run.first() else { return };
        let height = first.height();
        let (kind, number, text) = match role_of(first, self.frame) {
            Role::Numbered(number) => (ParagraphKind::Numbered(Decimal), Some(number), numbered_text(run)),
            Role::PointLabel => (ParagraphKind::PointLabel, None, run_text(run)),
            Role::Signature => (ParagraphKind::Signature, None, run_text(run)),
            Role::Exhibit => (ParagraphKind::Exhibit, None, run_text(run)),
            Role::Heading | Role::Label => (self.heading_kind(), None, run_text(run)),
            Role::Plain => (ParagraphKind::Body, None, run_text(run)),
        };
        self.push(kind, text, number, run);
        if matches!(kind, ParagraphKind::SectionHeading { .. }) {
            self.sections.push((self.out.len().saturating_sub(1), height));
        }
    }

    /// The kind a heading line makes: a point's heading when it follows a point label (so the
    /// points are lettered A, B, C … as siblings), a section heading otherwise.
    fn heading_kind(&mut self) -> ParagraphKind {
        if std::mem::take(&mut self.after_point_label) {
            ParagraphKind::PointHeading { letter: 'A' }
        } else {
            ParagraphKind::SectionHeading { level: 2 }
        }
    }

    fn push(&mut self, kind: ParagraphKind, text: String, number: Option<u32>, lines: &[Placed]) {
        let (indent_left, indent_first, line_spacing) = geometry(self.frame, lines);
        let bold = matches!(kind, ParagraphKind::SectionHeading { .. } | ParagraphKind::PointHeading { .. } | ParagraphKind::PointLabel);
        self.out.push(Paragraph { kind, text, number, bold, indent_left, indent_first, line_spacing });
        self.rects.push(self.rect);
        self.after_point_label = kind == ParagraphKind::PointLabel;
    }
}

/// The paragraph's indents and line spacing, measured from its lines.
fn geometry(frame: &Frame, lines: &[Placed]) -> (f32, f32, f32) {
    let left = lines.iter().map(|line| line.left).reduce(f32::min).unwrap_or(frame.left);
    let first = lines.first().map_or(left, |line| line.left);
    ((left - frame.left).max(0.0), first - left, line_spacing(lines))
}

/// The distance between a paragraph's line tops divided by its line height: 1.0 is single
/// spacing. A one-line paragraph reports 1.0, the spacing it cannot show.
fn line_spacing(lines: &[Placed]) -> f32 {
    let steps = lines.len().saturating_sub(1);
    let (Some(first), Some(last)) = (lines.first(), lines.last()) else { return 1.0 };
    if steps == 0 {
        return 1.0;
    }
    let height = lines.iter().map(|line| line.height()).sum::<f32>() / lines.len() as f32;
    if !(height.is_finite() && height > 0.0) {
        return 1.0;
    }
    (last.top - first.top) / steps as f32 / height
}

/// Gives each section heading the level its size clusters into.
fn assign_levels(paragraphs: &mut [Paragraph], sections: &[(usize, f32)]) {
    let heights: Vec<f32> = sections.iter().map(|(_, height)| *height).collect();
    let levels = heading_levels(&heights);
    for (index, height) in sections {
        if let Some(Paragraph { kind: ParagraphKind::SectionHeading { level }, .. }) = paragraphs.get_mut(*index) {
            *level = level_for(&levels, *height);
        }
    }
}

/// One level per distinct heading size: the tallest is level 2 (the legal exemplars' Heading 2)
/// and each smaller size one level deeper, capped at Word's nine.
fn heading_levels(heights: &[f32]) -> Vec<(f32, u8)> {
    let mut sorted = heights.to_vec();
    sorted.sort_by(|a, b| b.total_cmp(a));
    let mut clusters: Vec<(f32, u8)> = Vec::new();
    for height in sorted {
        let same_cluster = clusters.last().is_some_and(|(representative, _)| font_agrees(*representative, height));
        if same_cluster {
            continue;
        }
        let level = (2 + clusters.len()).min(9) as u8;
        clusters.push((height, level));
    }
    clusters
}

/// The level of a heading of `height`: the cluster it falls in, or 2 when it matches none.
fn level_for(levels: &[(f32, u8)], height: f32) -> u8 {
    levels.iter().find(|(representative, _)| font_agrees(*representative, height)).map_or(2, |(_, level)| *level)
}

/// Letters the point headings A, B, C … as one sibling list across the paragraphs given.
fn letter_points(paragraphs: &mut [Paragraph]) {
    let mut next = 0usize;
    for paragraph in paragraphs {
        if let ParagraphKind::PointHeading { letter } = &mut paragraph.kind {
            *letter = (b'A' + (next % 26) as u8) as char;
            next += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Word;
    use crate::page_layout::Rect;

    /// The body column of the fixture pages: a US Letter page's text block, in image pixels.
    const LEFT: f32 = 100.0;
    const BODY: f32 = 12.0;
    /// A body line wide enough to set the column's right edge.
    const BODY_LINE: &str = "Plaintiff submits this memorandum of law in support of the";

    /// `text` as one line of words starting at (`left`, `top`), `height` pixels tall, each word
    /// about as wide as its letters, as a recogniser reports them.
    fn words_at(text: &str, left: f32, top: f32, height: f32) -> Vec<Word> {
        let mut out = Vec::new();
        let mut x = left;
        for word in text.split_whitespace() {
            let width = word.chars().count() as f32 * height * 0.55;
            out.push(Word::new(word, [x, top, x + width, top + height], "ocrs"));
            x += width + height * 0.3;
        }
        out
    }

    /// The right edge of a line of words.
    fn right_edge(words: &[Word]) -> f32 {
        words.iter().map(|word| word.rect[2]).fold(0.0, f32::max)
    }

    /// `text` as a line centred on the column that ends at `column_right`.
    fn centred(text: &str, top: f32, height: f32, column_right: f32) -> Vec<Word> {
        let mut words = words_at(text, 0.0, top, height);
        let shift = LEFT + (column_right - LEFT - right_edge(&words)) / 2.0;
        for word in &mut words {
            word.rect[0] += shift;
            word.rect[2] += shift;
        }
        words
    }

    fn line_box(line: &Line) -> Rect {
        line.words
            .iter()
            .map(|word| Rect::new(word.rect[0], word.rect[1], word.rect[2], word.rect[3]))
            .reduce(Rect::union)
            .unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0))
    }

    /// One region of a fixture page, with its box measured from its lines.
    fn region(kind: RegionKind, order: usize, lines: Vec<Vec<Word>>) -> Region {
        let lines: Vec<Line> = lines.into_iter().map(|words| Line { words }).collect();
        let rect = lines.iter().map(line_box).reduce(Rect::union).unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
        Region { kind, rect, lines, order }
    }

    /// A first page of a New York filing: a court stamp, the caption, body text, a section
    /// heading, a numbered paragraph, a point and its heading, body text, a signature block and
    /// an exhibit label. No client document: every word is written here.
    fn legal_regions() -> Vec<Region> {
        let body = words_at(BODY_LINE, LEFT, 240.0, BODY);
        let right = right_edge(&body);
        vec![
            region(RegionKind::Header, 0, vec![words_at("RECEIVED NYSCEF: 07/22/2026", LEFT, 16.0, BODY)]),
            region(
                RegionKind::Caption,
                1,
                vec![
                    words_at("SUPREME COURT OF THE STATE OF NEW YORK", LEFT, 72.0, BODY),
                    words_at("COUNTY OF NEW YORK", LEFT, 90.0, BODY),
                    words_at("JOHN DOE,", LEFT, 108.0, BODY),
                    words_at("Plaintiff,", 144.0, 126.0, BODY),
                    words_at("- against -", LEFT, 144.0, BODY),
                    words_at("JANE ROE,", LEFT, 162.0, BODY),
                    words_at("Defendant.", 144.0, 180.0, BODY),
                ],
            ),
            region(RegionKind::Text, 2, vec![body, words_at("of the motion and asks the Court to grant it.", LEFT, 258.0, BODY)]),
            region(RegionKind::Text, 3, vec![centred("THE COMPLAINT FAILS TO STATE A CLAIM", 300.0, 18.0, right)]),
            region(RegionKind::ListItem, 4, vec![words_at("1. First point of the argument", LEFT, 340.0, BODY)]),
            region(RegionKind::Text, 5, vec![words_at("POINT II", LEFT, 370.0, BODY)]),
            region(RegionKind::Text, 6, vec![centred("THE COURT LACKS JURISDICTION", 390.0, 18.0, right)]),
            region(RegionKind::Text, 7, vec![words_at("The complaint pleads no facts.", LEFT, 430.0, BODY)]),
            region(RegionKind::Signature, 8, vec![words_at("Dated: July 22, 2026", LEFT, 500.0, BODY)]),
            region(RegionKind::Exhibit, 9, vec![words_at("Exhibit A", LEFT, 560.0, BODY)]),
        ]
    }

    #[test]
    fn legal_lines_become_lists_headings_and_blocks() {
        let paragraphs = synthesize(&legal_regions());
        assert!(matches!(paragraphs[0].kind, ParagraphKind::HeaderFooterBlock));
        assert!(paragraphs.iter().any(|p| matches!(p.kind, ParagraphKind::Numbered(_))));
        assert!(paragraphs.iter().any(|p| matches!(p.kind, ParagraphKind::PointLabel)));
        assert!(paragraphs.iter().any(|p| matches!(p.kind, ParagraphKind::SectionHeading { level: 2 })));
        assert!(paragraphs.iter().any(|p| matches!(p.kind, ParagraphKind::Signature)));
        assert!(!paragraphs.iter().any(|p| p.text.starts_with("1.")));
    }

    /// A paragraph in which every line starts with a number is several list items, not one.
    #[test]
    fn a_paragraph_whose_lines_all_number_is_several_list_items() {
        let region = region(
            RegionKind::Text,
            0,
            vec![
                words_at("1. The first allegation.", LEFT, 200.0, BODY),
                words_at("2. The second allegation.", LEFT, 218.0, BODY),
                words_at("3. The third allegation.", LEFT, 236.0, BODY),
            ],
        );
        let paragraphs = synthesize(&[region]);
        assert_eq!(paragraphs.len(), 3, "{paragraphs:?}");
        assert!(paragraphs.iter().all(|p| p.kind == ParagraphKind::Numbered(Decimal)), "{paragraphs:?}");
        let numbers: Vec<Option<u32>> = paragraphs.iter().map(|p| p.number).collect();
        assert_eq!(numbers, vec![Some(1), Some(2), Some(3)]);
        assert!(paragraphs.iter().all(|p| p.text.starts_with("The ")), "{paragraphs:?}");
    }

    /// A numbered item's wrapped lines are one item, not one item plus body text.
    #[test]
    fn a_numbered_item_keeps_its_wrapped_lines() {
        let region = region(
            RegionKind::ListItem,
            0,
            vec![
                words_at("1. The defendant failed to appear and", LEFT, 200.0, BODY),
                words_at("therefore the complaint stands.", LEFT, 218.0, BODY),
            ],
        );
        let paragraphs = synthesize(&[region]);
        assert_eq!(paragraphs.len(), 1, "{paragraphs:?}");
        assert_eq!(paragraphs[0].kind, ParagraphKind::Numbered(Decimal));
        assert_eq!(paragraphs[0].text, "The defendant failed to appear and therefore the complaint stands.");
        assert_eq!(paragraphs[0].number, Some(1));
    }

    /// The point headings are one lettered list (A, B …), not one list per heading.
    #[test]
    fn point_headings_are_siblings_of_one_lettered_list() {
        let regions = vec![
            region(RegionKind::Text, 0, vec![words_at("POINT I", LEFT, 200.0, BODY)]),
            region(RegionKind::Text, 1, vec![words_at("THE FIRST ASSIGNMENT OF ERROR", LEFT, 220.0, BODY)]),
            region(RegionKind::Text, 2, vec![words_at("POINT II", LEFT, 260.0, BODY)]),
            region(RegionKind::Text, 3, vec![words_at("THE SECOND ASSIGNMENT OF ERROR", LEFT, 280.0, BODY)]),
        ];
        let paragraphs = synthesize(&regions);
        assert_eq!(paragraphs.iter().filter(|p| p.kind == ParagraphKind::PointLabel).count(), 2, "{paragraphs:?}");
        let letters: Vec<char> = paragraphs
            .iter()
            .filter_map(|p| match p.kind {
                ParagraphKind::PointHeading { letter } => Some(letter),
                _ => None,
            })
            .collect();
        assert_eq!(letters, vec!['A', 'B'], "{paragraphs:?}");
    }

    /// A region's block kind is carried to each of its lines, whatever the text reads as.
    #[test]
    fn blocks_keep_their_region_kind() {
        let paragraphs = synthesize(&legal_regions());
        for kind in [ParagraphKind::Caption, ParagraphKind::Exhibit] {
            assert!(paragraphs.iter().any(|p| p.kind == kind), "{kind:?} missing: {paragraphs:?}");
        }
    }

    #[test]
    fn geometry_is_carried_from_the_lines() {
        let region = region(
            RegionKind::Text,
            0,
            vec![words_at(BODY_LINE, LEFT + 30.0, 200.0, BODY), words_at("of the motion and asks the Court to grant it.", LEFT, 218.0, BODY)],
        );
        let paragraphs = synthesize(&[region]);
        assert_eq!(paragraphs.len(), 1, "{paragraphs:?}");
        let paragraph = &paragraphs[0];
        assert!((paragraph.indent_left - 0.0).abs() < 0.01, "{paragraph:?}");
        assert!((paragraph.indent_first - 30.0).abs() < 0.01, "{paragraph:?}");
        assert!((paragraph.line_spacing - 1.5).abs() < 0.01, "{paragraph:?}");
    }

    /// Headings are levelled by their size: the tallest is level 2, each smaller size one deeper.
    #[test]
    fn heading_levels_follow_the_heading_sizes() {
        let body = words_at(BODY_LINE, LEFT, 200.0, BODY);
        let right = right_edge(&body);
        let regions = vec![
            region(RegionKind::Text, 0, vec![body]),
            region(RegionKind::Text, 1, vec![centred("MEMORANDUM OF LAW IN SUPPORT", 240.0, 24.0, right)]),
            region(RegionKind::Text, 2, vec![centred("THE COMPLAINT FAILS TO STATE A CLAIM", 300.0, 18.0, right)]),
        ];
        let paragraphs = synthesize(&regions);
        let levels: Vec<u8> = paragraphs
            .iter()
            .filter_map(|p| match p.kind {
                ParagraphKind::SectionHeading { level } => Some(level),
                _ => None,
            })
            .collect();
        assert_eq!(levels, vec![2, 3], "{paragraphs:?}");
    }
}
