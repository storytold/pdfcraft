//! Changing what a page shows: list its objects, move and resize them, rewrite and restyle text,
//! add text and images, delete, and cover an area. Boxes are `[x0, y0, x1, y1]` in the displayed
//! page's points from its top-left, y down, so a page drawn at a scale can pass on what the person
//! touched without doing the page geometry itself. A failure is a message, never a panic.

use std::sync::Arc;

use pdfcraft_engine::{
    AddedContent, AddedText, BlockStyle, Edit, FontFamily, ImageEdit, NewAnnotation, ObjectKind, ObjectTarget, Shape, Style, TextAlign,
};
use serde_json::Value;

use crate::core::Document;

/// Which inventory an object comes from: text the page already had, an image it already had, or
/// something added with [`Document::add_text`] or [`Document::add_image`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Added,
    Text,
    Image,
}

impl Kind {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "added" => Ok(Self::Added),
            "text" => Ok(Self::Text),
            "image" => Ok(Self::Image),
            other => Err(format!("unknown object kind {other:?}")),
        }
    }

    fn engine(self) -> ObjectKind {
        match self {
            Self::Added => ObjectKind::Added,
            Self::Text => ObjectKind::Text,
            Self::Image => ObjectKind::Image,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub kind: Kind,
    pub index: usize,
}

impl Target {
    fn engine(self) -> ObjectTarget {
        ObjectTarget { kind: self.kind.engine(), index: self.index }
    }
}

/// Parses `[{"kind": "text", "index": 3}, ...]`.
pub fn parse_targets(json: &str) -> Result<Vec<Target>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let list = v.as_array().ok_or("targets are a list")?;
    list.iter()
        .map(|t| {
            let kind = Kind::parse(t["kind"].as_str().ok_or("a target has a kind")?)?;
            let index = usize::try_from(t["index"].as_u64().ok_or("a target has an index")?).map_err(|_| "the index is too large".to_string())?;
            Ok(Target { kind, index })
        })
        .collect()
}

/// How text looks, as far as the caller says; what it leaves out stays as it is. Parsed from
/// `{"family": "helvetica"|"times"|"courier", "bold": true, "italic": false, "size": 14,
/// "color": [r, g, b] (0 to 1), "align": "left"|"center"|"right"|"justify"}`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextStyle {
    pub family: Option<FontFamily>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub size: Option<f64>,
    pub color: Option<[f64; 3]>,
    pub align: Option<TextAlign>,
}

impl TextStyle {
    pub fn parse(json: &str) -> Result<Self, String> {
        if json.trim().is_empty() {
            return Ok(Self::default());
        }
        let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let family = match v["family"].as_str() {
            None => None,
            Some("helvetica") => Some(FontFamily::Helvetica),
            Some("times") => Some(FontFamily::Times),
            Some("courier") => Some(FontFamily::Courier),
            Some(other) => return Err(format!("unknown font family {other:?}")),
        };
        let align = match v["align"].as_str() {
            None => None,
            Some("left") => Some(TextAlign::Left),
            Some("center") => Some(TextAlign::Center),
            Some("right") => Some(TextAlign::Right),
            Some("justify") => Some(TextAlign::Justify),
            Some(other) => return Err(format!("unknown alignment {other:?}")),
        };
        let size = match v["size"].as_f64() {
            Some(s) if s.is_finite() && (1.0..=400.0).contains(&s) => Some(s),
            Some(_) => return Err("the size is 1 to 400 points".into()),
            None => None,
        };
        let color = match v.get("color").filter(|c| !c.is_null()) {
            None => None,
            Some(c) => {
                let parts: Vec<f64> = c.as_array().ok_or("a color is [r, g, b]")?.iter().filter_map(Value::as_f64).collect();
                match parts.as_slice() {
                    [r, g, b] if parts.iter().all(|x| (0.0..=1.0).contains(x)) => Some([*r, *g, *b]),
                    _ => return Err("a color is [r, g, b], each from 0 to 1".into()),
                }
            }
        };
        Ok(Self { family, bold: v["bold"].as_bool(), italic: v["italic"].as_bool(), size, color, align })
    }

    fn apply_to(&self, mut t: AddedText) -> AddedText {
        if let Some(f) = self.family {
            t.family = f;
        }
        if let Some(b) = self.bold {
            t.bold = b;
        }
        if let Some(i) = self.italic {
            t.italic = i;
        }
        if let Some(s) = self.size {
            t.size = s;
        }
        if let Some(c) = self.color {
            t.color = c;
        }
        if let Some(a) = self.align {
            t.align = a;
        }
        t
    }
}

/// A box that is a real rectangle and a finite one.
fn checked(rect: [f64; 4]) -> Result<[f64; 4], String> {
    if rect.iter().all(|v| v.is_finite() && v.abs() < 1e7) && rect[2] > rect[0] && rect[3] > rect[1] {
        Ok(rect)
    } else {
        Err("a box is [x0, y0, x1, y1] with x0 < x1 and y0 < y1".into())
    }
}

fn to_f32(r: [f64; 4]) -> [f32; 4] {
    r.map(|v| v as f32)
}

fn to_f64(r: [f32; 4]) -> [f64; 4] {
    r.map(f64::from)
}

/// A text style as the JSON [`TextStyle::parse`] reads back.
fn style_json(family: FontFamily, bold: bool, italic: bool, size: f64, color: [f64; 3], align: Option<TextAlign>) -> Value {
    serde_json::json!({
        "family": match family { FontFamily::Times => "times", FontFamily::Courier => "courier", _ => "helvetica" },
        "bold": bold,
        "italic": italic,
        "size": size,
        "color": color,
        "align": align.map(|a| match a { TextAlign::Center => "center", TextAlign::Right => "right", TextAlign::Justify => "justify", TextAlign::Left => "left" }),
    })
}

/// The words to write back when only the look or the width of a paragraph changes. Wrapped lines of one
/// paragraph are joined and told apart from the source by a trailing space, so the engine lays the whole
/// paragraph out again; a contents list (dotted leaders between a title and its page) keeps its rows.
fn reflowed(block: &pdfcraft_engine::TextBlock) -> String {
    let leaders = block.text.contains("....") || block.text.contains(". . . .");
    if leaders { block.editor_text.clone() } else { format!("{} ", block.text.trim_end()) }
}

fn family_of(base_font: &str) -> FontFamily {
    if base_font.contains("Times") {
        FontFamily::Times
    } else if base_font.contains("Courier") {
        FontFamily::Courier
    } else {
        FontFamily::Helvetica
    }
}

impl Document {
    /// A displayed box (from the top-left, y down) in the engine's display space for added items:
    /// the same page as shown, but measured up from the bottom-left.
    fn to_display_space(&self, page: usize, view: [f64; 4]) -> Result<[f64; 4], String> {
        let height = f64::from(self.page_info(page)?.height);
        Ok([view[0], height - view[3], view[2], height - view[1]])
    }

    pub(crate) fn page_info_ref(&self, page: usize) -> Result<&pdfcraft_render::PageInfo, String> {
        self.page_info(page)
    }

    fn page_info(&self, page: usize) -> Result<&pdfcraft_render::PageInfo, String> {
        self.doc()?.info.pages.get(page).ok_or_else(|| format!("page {} does not exist", page.saturating_add(1)))
    }

    /// The page's editable objects as JSON: for each its `kind` and `index` (what the other calls
    /// take), its `type` (`text`, `image` or `form`), `text` when it has some, `view`, the box where
    /// it is drawn, and for text its `style` (family, bold, italic, size, color, align).
    pub fn objects_json(&self, page: usize) -> Result<String, String> {
        let info = self.page_info(page)?;
        let objects = self.doc()?.editable_objects(page)?;
        let blocks = self.doc()?.text_blocks(page);
        let list: Vec<Value> = objects
            .iter()
            .map(|o| {
                let [a, b] = [info.user_to_view(o.rect[0] as f32, o.rect[1] as f32), info.user_to_view(o.rect[2] as f32, o.rect[3] as f32)];
                let style = match o.target.kind {
                    ObjectKind::Text => blocks
                        .get(o.target.index)
                        .map(|b| style_json(family_of(&b.base_font), b.bold, b.italic, b.size, b.color, self.align_of(page, b))),
                    ObjectKind::Added => match self.added_content(page, o.target.index) {
                        Ok(AddedContent::Text(t)) => Some(style_json(t.family, t.bold, t.italic, t.size, t.color, Some(t.align))),
                        _ => None,
                    },
                    ObjectKind::Image => None,
                };
                serde_json::json!({
                    "kind": match o.target.kind { ObjectKind::Added => "added", ObjectKind::Text => "text", ObjectKind::Image => "image" },
                    "index": o.target.index,
                    "type": o.content_type,
                    "text": o.text,
                    "view": [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])],
                    "style": style,
                })
            })
            .collect();
        serde_json::to_string(&list).map_err(|e| e.to_string())
    }

    /// Moves objects by (`dx`, `dy`) displayed points: text, images and added items together.
    pub fn move_objects(&mut self, page: usize, targets: &[Target], dx: f64, dy: f64) -> Result<(), String> {
        if !(dx.is_finite() && dy.is_finite()) {
            return Err("the move must be finite".into());
        }
        let offset = self.doc()?.object_move_offset(page, [dx, dy])?;
        self.apply(Edit::MoveObjects { page, objects: targets.iter().map(|t| t.engine()).collect(), offset })
    }

    /// The added item's content, to change one part of it.
    fn added_content(&self, page: usize, index: usize) -> Result<AddedContent, String> {
        self.doc()?
            .added()
            .iter()
            .filter(|a| a.page == page)
            .nth(index)
            .map(|a| a.content.clone())
            .ok_or_else(|| format!("page {} has no added item {}", page.saturating_add(1), index.saturating_add(1)))
    }

    /// Gives an object a new box. An added item or an image takes it as it is; a paragraph of
    /// existing text keeps its size and rewraps to the new width.
    pub fn resize_object(&mut self, page: usize, target: Target, view: [f64; 4]) -> Result<(), String> {
        let view = checked(view)?;
        match target.kind {
            Kind::Added => {
                let rect = self.to_display_space(page, view)?;
                let content = match self.added_content(page, target.index)? {
                    AddedContent::Text(mut t) => {
                        t.rect = rect;
                        AddedContent::Text(t)
                    }
                    AddedContent::Image(mut i) => {
                        i.rect = rect;
                        AddedContent::Image(i)
                    }
                };
                self.apply(Edit::UpdateContent { page, index: target.index, content })
            }
            Kind::Image => {
                let user = to_f64(self.page_info(page)?.view_rect_to_user(to_f32(view)));
                self.apply(Edit::EditPageImage { page, index: target.index, change: ImageEdit::Move(user) })
            }
            Kind::Text => {
                let user = self.page_info(page)?.view_rect_to_user(to_f32(view));
                let block = self.doc()?.text_blocks(page).into_iter().nth(target.index).ok_or("that paragraph no longer exists")?;
                let style = BlockStyle { width: Some(f64::from(user[2] - user[0])), ..BlockStyle::default() };
                self.apply(Edit::EditTextBlock { page, block: target.index, text: reflowed(&block), style })
            }
        }
    }

    /// How a paragraph's lines sit against each other, read from where its words start and end: flush
    /// left, justified, flush right or centred. A single line, or lines that fit none of those, say nothing.
    fn align_of(&self, page: usize, block: &pdfcraft_engine::TextBlock) -> Option<TextAlign> {
        let lines = self.doc().map(|d| d.text_lines(page)).ok()?;
        let mut ends: Vec<(f64, f64)> = Vec::new();
        for line in block.lines.iter().filter_map(|i| lines.get(*i)) {
            let left = line.ink.iter().map(|r| r[0]).fold(f64::INFINITY, f64::min);
            let right = line.ink.iter().map(|r| r[2]).fold(f64::NEG_INFINITY, f64::max);
            if left.is_finite() && right.is_finite() {
                ends.push((left, right));
            }
        }
        // the last line of a paragraph is short whatever the alignment
        if ends.len() >= 3 {
            ends.pop();
        }
        if ends.len() < 2 {
            return None;
        }
        const TOLERANCE: f64 = 1.5;
        let (min_left, max_right) = ends.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(l, r), (a, b)| (l.min(*a), r.max(*b)));
        let mid = ends.iter().map(|(a, b)| (a + b) / 2.0).sum::<f64>() / ends.len() as f64;
        if ends.iter().all(|(a, _)| (a - min_left).abs() <= TOLERANCE) {
            // lines that also end together (all but the last) are justified
            let justified = ends.iter().all(|(_, b)| (b - max_right).abs() <= TOLERANCE);
            Some(if justified { TextAlign::Justify } else { TextAlign::Left })
        } else if ends.iter().all(|(_, b)| (b - max_right).abs() <= TOLERANCE) {
            Some(TextAlign::Right)
        } else if ends.iter().all(|(a, b)| ((a + b) / 2.0 - mid).abs() <= TOLERANCE) {
            Some(TextAlign::Center)
        } else {
            None
        }
    }

    /// How wide a paragraph's words run, in user space. Its box also spans the trailing space a rewrite
    /// leaves, which would make the box a space wider with every restyle.
    fn ink_width(&self, page: usize, block: &pdfcraft_engine::TextBlock) -> f64 {
        let lines = self.doc().map(|d| d.text_lines(page)).unwrap_or_default();
        let mut span: Option<(f64, f64)> = None;
        for line in block.lines.iter().filter_map(|i| lines.get(*i)) {
            for ink in &line.ink {
                span = Some(span.map_or((ink[0], ink[2]), |(a, b)| (a.min(ink[0]), b.max(ink[2]))));
            }
        }
        span.map_or(block.rect[2] - block.rect[0], |(a, b)| b - a)
    }

    /// Turns a picture by quarter turns clockwise (negative for anticlockwise) about its centre.
    /// An added picture swaps its width and height on an odd turn, so it keeps its place.
    pub fn rotate_object(&mut self, page: usize, target: Target, quarter_turns: i32) -> Result<(), String> {
        let turns = quarter_turns.rem_euclid(4);
        match target.kind {
            Kind::Image => self.apply(Edit::EditPageImage { page, index: target.index, change: ImageEdit::Rotate(turns) }),
            Kind::Added => match self.added_content(page, target.index)? {
                AddedContent::Image(mut i) => {
                    // the engine counts quarter turns anticlockwise
                    i.rotation = ((i64::from(i.rotation) + i64::from(4 - turns)).rem_euclid(4)) as u8;
                    if turns % 2 == 1 {
                        let [x0, y0, x1, y1] = i.rect;
                        let (cx, cy, hw, hh) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0) / 2.0, (y1 - y0) / 2.0);
                        i.rect = [cx - hh, cy - hw, cx + hh, cy + hw];
                    }
                    self.apply(Edit::UpdateContent { page, index: target.index, content: AddedContent::Image(i) })
                }
                AddedContent::Text(_) => Err("only a picture can be turned".into()),
            },
            Kind::Text => Err("only a picture can be turned".into()),
        }
    }

    /// Adds a text box over `view` (the width wraps the text; the height follows it).
    pub fn add_text(&mut self, page: usize, view: [f64; 4], text: &str, style: &TextStyle) -> Result<(), String> {
        let rect = self.to_display_space(page, checked(view)?)?;
        let added = style.apply_to(AddedText { rect, text: text.to_string(), ..AddedText::default() });
        self.apply(Edit::AddText { page, text: added })
    }

    /// Adds a picture (PNG, JPEG, TIFF, GIF or BMP) over `view`, or at its natural size in the middle
    /// of the page when there is no box.
    pub fn add_image(&mut self, page: usize, name: &str, bytes: &[u8], view: Option<[f64; 4]>) -> Result<(), String> {
        let rect = view.map(checked).transpose()?.map(|v| self.to_display_space(page, v)).transpose()?;
        self.apply(Edit::AddImage { page, rect, name: name.to_string(), bytes: Arc::new(bytes.to_vec()) })
    }

    /// Removes an object. Existing text is emptied, an existing image is taken out of the page.
    pub fn delete_object(&mut self, page: usize, target: Target) -> Result<(), String> {
        match target.kind {
            Kind::Added => self.apply(Edit::DeleteContent { page, index: target.index }),
            Kind::Image => self.apply(Edit::EditPageImage { page, index: target.index, change: ImageEdit::Delete }),
            Kind::Text => self.apply(Edit::EditTextBlock { page, block: target.index, text: String::new(), style: BlockStyle::default() }),
        }
    }

    /// Rewrites the text of an object and/or changes how it looks (`text` `None` keeps the words).
    pub fn set_text(&mut self, page: usize, target: Target, text: Option<&str>, style: &TextStyle) -> Result<(), String> {
        match target.kind {
            Kind::Added => match self.added_content(page, target.index)? {
                AddedContent::Text(mut t) => {
                    if let Some(words) = text {
                        t.text = words.to_string();
                    }
                    self.apply(Edit::UpdateContent { page, index: target.index, content: AddedContent::Text(style.apply_to(t)) })
                }
                AddedContent::Image(_) => Err("that is a picture, not text".into()),
            },
            Kind::Text => {
                let block = self.doc()?.text_blocks(page).into_iter().nth(target.index).ok_or("that paragraph no longer exists")?;
                // The paragraph's own rows (with a break between them) take the engine's row-keeping path, which cuts
                // a restyled block into lines; its words joined, and changed by a trailing space, lay the block out again.
                let words = text.map_or_else(|| reflowed(&block), str::to_string);
                // A family, bold and italic go together in the engine's style; fill what is not said from the paragraph.
                let wants_face = style.family.is_some() || style.bold.is_some() || style.italic.is_some();
                let family = wants_face.then(|| {
                    (
                        style.family.unwrap_or_else(|| family_of(&block.base_font)),
                        style.bold.unwrap_or(block.bold),
                        style.italic.unwrap_or(block.italic),
                    )
                });
                let edit = BlockStyle {
                    family,
                    size: style.size,
                    color: style.color,
                    align: style.align,
                    width: Some(self.ink_width(page, &block)),
                    ..BlockStyle::default()
                };
                self.apply(Edit::EditTextBlock { page, block: target.index, text: words, style: edit })
            }
            Kind::Image => Err("a picture has no text".into()),
        }
    }

    /// Paints a white box over `view`, covering what is under it (a page mark, not an erasure: the
    /// covered content is still in the file).
    pub fn cover(&mut self, page: usize, view: [f64; 4]) -> Result<(), String> {
        let user = to_f64(self.page_info(page)?.view_rect_to_user(to_f32(checked(view)?)));
        let white = [1.0, 1.0, 1.0];
        let shape = Shape::Rectangle { rect: user };
        let style = Style { color: white, opacity: 1.0, width: 0.0, fill: Some(white) };
        self.apply(Edit::AddAnnotation(NewAnnotation { page, shape, style, contents: String::new(), author: String::new() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let body = "BT /F1 24 Tf 20 150 Td (Hello there) Tj ET";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    fn png() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 30, 30, 255])).write_to(&mut out, image::ImageFormat::Png).expect("encodes");
        out.into_inner()
    }

    fn open() -> Document {
        Document::open(&fixture(), None, || 1_700_000_000).expect("opens")
    }

    fn objects(doc: &Document) -> Vec<Value> {
        serde_json::from_str(&doc.objects_json(0).expect("objects")).expect("json")
    }

    fn view_of(o: &Value) -> Vec<f64> {
        o["view"].as_array().expect("view").iter().map(|v| v.as_f64().expect("number")).collect()
    }

    #[test]
    fn lists_the_text_a_page_already_has() {
        let doc = open();
        let list = objects(&doc);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["kind"], "text");
        assert_eq!(list[0]["text"], "Hello there");
        assert_eq!(list[0]["style"]["family"], "helvetica");
        assert_eq!(list[0]["style"]["bold"], false);
        assert!((list[0]["style"]["size"].as_f64().expect("size") - 24.0).abs() < 0.5);
        let v = view_of(&list[0]);
        assert!(v[0] >= 0.0 && v[2] <= 200.0 && v[1] < v[3] && v[3] <= 300.0, "inside the page: {v:?}");
    }

    #[test]
    fn adds_text_and_restyles_it() {
        let mut doc = open();
        doc.add_text(0, [20.0, 40.0, 180.0, 80.0], "Added words", &TextStyle { size: Some(18.0), bold: Some(true), ..TextStyle::default() })
            .expect("adds");
        let added: Vec<Value> = objects(&doc).into_iter().filter(|o| o["kind"] == "added").collect();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0]["text"], "Added words");
        // it lands where the box was asked for: 40 points from the top of the page, not from the bottom
        let v = view_of(&added[0]);
        assert!((v[1] - 40.0).abs() < 8.0, "near the top edge asked for: {v:?}");
        assert!((v[0] - 20.0).abs() < 8.0, "near the left edge asked for: {v:?}");
        assert_eq!(added[0]["style"]["bold"], true);
        assert_eq!(added[0]["style"]["size"], 18.0);
        let target = Target { kind: Kind::Added, index: 0 };
        doc.set_text(
            0,
            target,
            Some("Changed words"),
            &TextStyle { color: Some([0.8, 0.0, 0.0]), align: Some(TextAlign::Center), ..TextStyle::default() },
        )
        .expect("restyles");
        let added: Vec<Value> = objects(&doc).into_iter().filter(|o| o["kind"] == "added").collect();
        assert_eq!(added[0]["text"], "Changed words");
        assert!(doc.add_text(0, [10.0, 10.0, 5.0, 5.0], "x", &TextStyle::default()).is_err(), "an inside-out box");
        doc.undo().expect("undoes the restyle");
        let added: Vec<Value> = objects(&doc).into_iter().filter(|o| o["kind"] == "added").collect();
        assert_eq!(added[0]["text"], "Added words");
    }

    #[test]
    fn moves_resizes_and_deletes_what_was_added() {
        let mut doc = open();
        doc.add_text(0, [20.0, 40.0, 120.0, 70.0], "Box", &TextStyle::default()).expect("adds");
        doc.add_image(0, "pic.png", &png(), Some([100.0, 200.0, 160.0, 260.0])).expect("adds an image");
        let list = objects(&doc);
        let image = list.iter().find(|o| o["type"] == "image").expect("the image is listed");
        assert_eq!(image["kind"], "added");
        let before = view_of(image);
        assert!((before[1] - 200.0).abs() < 2.0 && (before[3] - 260.0).abs() < 2.0, "the picture is where it was put: {before:?}");
        let target = Target { kind: Kind::Added, index: image["index"].as_u64().expect("index") as usize };

        doc.move_objects(0, &[target], 10.0, 20.0).expect("moves");
        let moved = view_of(objects(&doc).iter().find(|o| o["type"] == "image").expect("still there"));
        assert!(
            (moved[0] - before[0] - 10.0).abs() < 0.01 && (moved[1] - before[1] - 20.0).abs() < 0.01,
            "moved by the drag: {before:?} to {moved:?}"
        );

        doc.resize_object(0, target, [100.0, 200.0, 190.0, 290.0]).expect("resizes");
        let sized = view_of(objects(&doc).iter().find(|o| o["type"] == "image").expect("still there"));
        assert!((sized[2] - sized[0] - 90.0).abs() < 0.5, "90 wide: {sized:?}");
        assert!((sized[1] - 200.0).abs() < 1.0 && (sized[3] - 290.0).abs() < 1.0, "and 90 tall, where asked: {sized:?}");

        doc.delete_object(0, target).expect("deletes");
        assert!(objects(&doc).iter().all(|o| o["type"] != "image"));
        assert!(doc.move_objects(0, &[target], 1.0, 1.0).is_err(), "it no longer exists");
        assert!(doc.move_objects(0, &[target], f64::NAN, 1.0).is_err());
    }

    #[test]
    fn turns_an_added_picture() {
        let mut doc = open();
        doc.add_image(0, "pic.png", &png(), Some([100.0, 200.0, 180.0, 240.0])).expect("adds an image");
        let image = objects(&doc).into_iter().find(|o| o["type"] == "image").expect("listed");
        let target = Target { kind: Kind::Added, index: image["index"].as_u64().expect("index") as usize };
        let before = view_of(&image);
        doc.rotate_object(0, target, 1).expect("turns");
        let after = view_of(objects(&doc).iter().find(|o| o["type"] == "image").expect("still there"));
        assert!((after[2] - after[0] - (before[3] - before[1])).abs() < 0.5, "wide as it was tall: {before:?} {after:?}");
        assert!(((after[0] + after[2]) - (before[0] + before[2])).abs() < 0.5, "same centre: {before:?} {after:?}");
        doc.rotate_object(0, target, -1).expect("turns back");
        let back = view_of(objects(&doc).iter().find(|o| o["type"] == "image").expect("still there"));
        assert!((back[2] - back[0] - (before[2] - before[0])).abs() < 0.5, "as it was: {before:?} {back:?}");
        doc.add_text(0, [20.0, 40.0, 120.0, 70.0], "Box", &TextStyle::default()).expect("adds");
        let text = objects(&doc).into_iter().find(|o| o["type"] == "text").expect("text");
        let t = Target { kind: Kind::Added, index: text["index"].as_u64().expect("index") as usize };
        assert!(doc.rotate_object(0, t, 1).is_err(), "text does not turn");
    }

    #[test]
    fn a_huge_page_or_item_number_is_an_error_not_a_wrap() {
        let mut doc = open();
        let target = Target { kind: Kind::Added, index: usize::MAX };
        assert!(doc.resize_object(0, target, [0.0, 0.0, 10.0, 10.0]).is_err());
        assert!(doc.resize_object(usize::MAX, target, [0.0, 0.0, 10.0, 10.0]).is_err());
        assert!(doc.set_text(usize::MAX, target, Some("x"), &TextStyle::default()).is_err());
    }

    #[test]
    fn moves_and_restyles_existing_text() {
        let mut doc = open();
        let text = Target { kind: Kind::Text, index: 0 };
        let before = view_of(&objects(&doc)[0]);
        doc.move_objects(0, &[text], 0.0, 30.0).expect("moves the paragraph");
        let after = view_of(&objects(&doc)[0]);
        assert!((after[1] - before[1] - 30.0).abs() < 0.5, "30 points lower: {before:?} to {after:?}");
        doc.set_text(
            0,
            text,
            None,
            &TextStyle { bold: Some(true), italic: Some(true), size: Some(30.0), family: Some(FontFamily::Times), ..TextStyle::default() },
        )
        .expect("restyles");
        let blocks: Vec<Value> = serde_json::from_str(&doc.text_blocks_json(0).expect("blocks")).expect("json");
        // the larger size rewraps the paragraph to its box, so compare the words, not the line breaks
        assert_eq!(blocks[0]["text"].as_str().expect("text").replace('\n', " ").trim_end(), "Hello there");
        assert_eq!(blocks[0]["bold"], true);
        assert_eq!(blocks[0]["italic"], true);
        assert!((blocks[0]["size"].as_f64().expect("size") - 30.0).abs() < 0.5);
        doc.delete_object(0, text).expect("empties the paragraph");
        assert!(!doc.text_blocks_json(0).expect("blocks").contains("Hello"), "the words are gone");
    }

    #[test]
    fn bold_keeps_the_typeface_of_existing_text() {
        let times = String::from_utf8(fixture()).expect("ASCII").replace("/BaseFont /Helvetica", "/BaseFont /Times-Roman").into_bytes();
        let mut doc = Document::open(&times, None, || 1_700_000_000).expect("opens");
        let text = Target { kind: Kind::Text, index: 0 };
        assert_eq!(objects(&doc)[0]["style"]["family"], "times");
        doc.set_text(0, text, None, &TextStyle { bold: Some(true), ..TextStyle::default() }).expect("bolds");
        let blocks: Vec<Value> = serde_json::from_str(&doc.text_blocks_json(0).expect("blocks")).expect("json");
        assert_eq!(blocks[0]["bold"], true);
        assert!(blocks[0]["font"].as_str().expect("font").contains("Times"), "still Times: {}", blocks[0]["font"]);
    }

    #[test]
    fn covers_an_area_with_white() {
        let mut doc = open();
        let before = doc.render(0, 1.0, None).expect("renders");
        doc.cover(0, [0.0, 0.0, 200.0, 300.0]).expect("covers");
        let after = doc.render(0, 1.0, None).expect("renders");
        assert!(before.rgba.chunks(4).any(|p| p[..3] != [255, 255, 255]), "the text showed");
        assert!(after.rgba.chunks(4).all(|p| p[..3] == [255, 255, 255]), "now all white");
        assert!(doc.cover(0, [5.0, 5.0, 5.0, 9.0]).is_err());
    }

    #[test]
    fn rejects_bad_requests() {
        let mut doc = open();
        assert!(Kind::parse("shape").is_err());
        assert!(parse_targets("{}").is_err());
        assert!(parse_targets(r#"[{"kind": "text"}]"#).is_err());
        assert_eq!(parse_targets(r#"[{"kind": "image", "index": 2}]"#).expect("parses"), vec![Target { kind: Kind::Image, index: 2 }]);
        assert!(TextStyle::parse(r#"{"size": 0}"#).is_err());
        assert!(TextStyle::parse(r#"{"color": [2, 0, 0]}"#).is_err());
        assert!(TextStyle::parse(r#"{"family": "comic"}"#).is_err());
        assert_eq!(TextStyle::parse("").expect("empty is fine"), TextStyle::default());
        assert!(doc.objects_json(3).is_err());
        assert!(doc.set_text(0, Target { kind: Kind::Image, index: 0 }, None, &TextStyle::default()).is_err());
        assert!(doc.add_image(0, "bad.png", b"not a picture", None).is_err());
        assert!(doc.resize_object(0, Target { kind: Kind::Added, index: 5 }, [1.0, 1.0, 9.0, 9.0]).is_err());
    }
}
