//! Annotations with hand-built appearance streams.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};

use super::pdf::{Content, Font, Fonts, Rgb, Stamp, colors, floats, form_xobject, name, rect, text};

/// An axis-aligned rectangle in PDF user space.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Rect { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) }
    }
    pub fn xywh(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect::new(x, y, x + w, y + h)
    }
    pub fn w(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn h(&self) -> f32 {
        self.y1 - self.y0
    }
    pub fn obj(&self) -> Object {
        rect(self.x0, self.y0, self.x1, self.y1)
    }
    /// Parse a PDF `/Rect` array.
    pub fn from_obj(obj: &Object) -> Option<Self> {
        let a = obj.as_array().ok()?;
        let v: Vec<f32> = a.iter().filter_map(|o| o.as_float().ok()).collect();
        (v.len() == 4).then(|| Rect::new(v[0], v[1], v[2], v[3]))
    }
}

/// Every annotation kind the showcase exercises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
    Caret,
    Note,
    FreeText,
    Line,
    Square,
    Circle,
    Polygon,
    PolyLine,
    Ink,
    Stamp,
    Attachment,
}

impl Kind {
    /// Parse the `<kind>` segment of a marker URI.
    pub fn from_marker(s: &str) -> Option<Self> {
        Some(match s {
            "highlight" => Kind::Highlight,
            "underline" => Kind::Underline,
            "strikeout" => Kind::StrikeOut,
            "squiggly" => Kind::Squiggly,
            "caret" => Kind::Caret,
            "note" => Kind::Note,
            "freetext" => Kind::FreeText,
            "line" => Kind::Line,
            "square" => Kind::Square,
            "circle" => Kind::Circle,
            "polygon" => Kind::Polygon,
            "polyline" => Kind::PolyLine,
            "ink" => Kind::Ink,
            "stamp" => Kind::Stamp,
            "attach" => Kind::Attachment,
            _ => return None,
        })
    }
}

const AUTHOR: &str = "PdfCraft Reviewer";
const AUTHOR_2: &str = "Layout Editor";

/// Builds annotation dictionaries (and their appearance XObjects) for one document.
pub struct Annotator {
    fonts: Fonts,
    now: Stamp,
    seq: u32,
}

/// A built annotation: the main dictionary plus any extra annotation objects
/// (popups, replies) that must also be listed in the page's `/Annots`.
pub struct Built {
    pub dict: Dictionary,
    pub extra: Vec<ObjectId>,
}

impl Annotator {
    pub fn new(fonts: Fonts, now: Stamp) -> Self {
        Annotator { fonts, now, seq: 0 }
    }

    /// Common markup-annotation entries.
    fn base(&mut self, subtype: &str, r: Rect, color: Rgb, author: &str, contents: &str, page: ObjectId) -> Dictionary {
        self.seq += 1;
        let when = self.now.plus_minutes(i64::from(self.seq));
        dictionary! {
            "Type" => "Annot",
            "Subtype" => name(subtype),
            "Rect" => r.obj(),
            "P" => page,
            "F" => 4,
            "C" => color.array(),
            "T" => text(author),
            "Contents" => text(contents),
            "Subj" => text(subtype),
            "M" => Object::string_literal(when.pdf.clone()),
            "CreationDate" => Object::string_literal(when.pdf),
            "NM" => Object::string_literal(format!("pdfcraft-showcase-{:03}-{}", self.seq, subtype.to_lowercase())),
        }
    }

    fn set_ap(&self, doc: &mut Document, dict: &mut Dictionary, r: Rect, content: Content, resources: Dictionary) {
        let id = form_xobject(doc, r.w(), r.h(), content, resources);
        dict.set("AP", dictionary! { "N" => id });
    }

    fn multiply_resources(&self) -> Dictionary {
        dictionary! {
            "ExtGState" => dictionary! { "Mult" => dictionary! { "Type" => "ExtGState", "BM" => "Multiply" } },
        }
    }

    /// Build the annotation of `kind` whose geometry is derived from `r`.
    /// `self_id` is the object id the returned dictionary will be stored under.
    pub fn build(&mut self, doc: &mut Document, page: ObjectId, self_id: ObjectId, kind: Kind, r: Rect) -> Built {
        let mut extra = Vec::new();
        let dict = match kind {
            Kind::Highlight | Kind::Underline | Kind::StrikeOut | Kind::Squiggly => self.text_markup(doc, page, kind, r),
            Kind::Caret => self.caret(doc, page, r),
            Kind::Note => {
                let (dict, more) = self.sticky_note(doc, page, self_id, r);
                extra = more;
                dict
            }
            Kind::FreeText => self.free_text(doc, page, r),
            Kind::Line => self.line(doc, page, r),
            Kind::Square | Kind::Circle => self.shape(doc, page, kind, r),
            Kind::Polygon | Kind::PolyLine => self.poly(doc, page, kind, r),
            Kind::Ink => self.ink(doc, page, r),
            Kind::Stamp => self.stamp(doc, page, r),
            Kind::Attachment => self.attachment(doc, page, r),
        };
        Built { dict, extra }
    }

    fn text_markup(&mut self, doc: &mut Document, page: ObjectId, kind: Kind, r: Rect) -> Dictionary {
        let (subtype, color, contents) = match kind {
            Kind::Highlight => ("Highlight", colors::HIGHLIGHT, "Key phrase - keep this wording."),
            Kind::Underline => ("Underline", colors::BLUE, "Please cite a source for this claim."),
            Kind::StrikeOut => ("StrikeOut", colors::ACCENT, "Redundant; suggest deleting."),
            _ => ("Squiggly", colors::TEAL, "Check the spelling here."),
        };
        let mut dict = self.base(subtype, r, color, AUTHOR, contents, page);
        // QuadPoints order used by every mainstream viewer: TL, TR, BL, BR.
        dict.set("QuadPoints", floats(&[r.x0, r.y1, r.x1, r.y1, r.x0, r.y0, r.x1, r.y0]));
        dict.set("CA", 1.0);

        let (w, h) = (r.w(), r.h());
        let mut c = Content::new();
        let mut res = Dictionary::new();
        match kind {
            Kind::Highlight => {
                res = self.multiply_resources();
                c.gs("Mult").fill_color(color).rect(0.0, 0.0, w, h).fill();
            }
            Kind::Underline => {
                let t = (h / 16.0).max(0.8);
                c.stroke_color(color).line_width(t).move_to(0.0, h * 0.17).line_to(w, h * 0.17).stroke();
            }
            Kind::StrikeOut => {
                let t = (h / 16.0).max(0.8);
                c.stroke_color(color).line_width(t).move_to(0.0, h * 0.44).line_to(w, h * 0.44).stroke();
            }
            _ => {
                let (base, amp, period) = (h * 0.12, 1.4, 4.0);
                c.stroke_color(color).line_width(0.9).round_caps().move_to(0.0, base);
                let mut x = 0.0;
                let mut up = true;
                while x < w {
                    x = (x + period / 2.0).min(w);
                    c.line_to(x, if up { base + amp } else { base - amp });
                    up = !up;
                }
                c.stroke();
            }
        }
        self.set_ap(doc, &mut dict, r, c, res);
        dict
    }

    fn caret(&mut self, doc: &mut Document, page: ObjectId, anchor: Rect) -> Dictionary {
        let cx = (anchor.x0 + anchor.x1) / 2.0;
        let r = Rect::new(cx - 6.0, anchor.y0 + 1.0, cx + 6.0, anchor.y0 + anchor.h() * 0.62);
        let mut dict = self.base("Caret", r, colors::BLUE, AUTHOR, "Insert: \"automatically\"", page);
        dict.set("Sy", "None");
        dict.set("RD", floats(&[0.0, 0.0, 0.0, 0.0]));
        let (w, h) = (r.w(), r.h());
        let mut c = Content::new();
        c.fill_color(colors::BLUE)
            .move_to(0.0, 0.0)
            .curve_to(w * 0.35, h * 0.2, w * 0.45, h * 0.6, w * 0.5, h)
            .curve_to(w * 0.55, h * 0.6, w * 0.65, h * 0.2, w, 0.0)
            .close()
            .fill();
        self.set_ap(doc, &mut dict, r, c, Dictionary::new());
        dict
    }

    fn note_icon(&self, doc: &mut Document, r: Rect, fill: Rgb) -> ObjectId {
        let (w, h) = (r.w(), r.h());
        let mut c = Content::new();
        c.fill_color(fill).stroke_color(colors::INK).line_width(0.8).rounded_rect(0.5, 0.5, w - 1.0, h - 1.0, 3.0).fill_stroke();
        c.line_width(1.0).round_caps();
        for i in 1..=3 {
            let y = h - 0.5 - (h - 1.0) * i as f32 / 4.2;
            let x1 = if i == 3 { w * 0.55 } else { w * 0.78 };
            c.move_to(w * 0.22, y).line_to(x1, y);
        }
        c.stroke();
        form_xobject(doc, w, h, c, Dictionary::new())
    }

    fn sticky_note(&mut self, doc: &mut Document, page: ObjectId, self_id: ObjectId, r: Rect) -> (Dictionary, Vec<ObjectId>) {
        let r = Rect::xywh(r.x0, r.y1 - 20.0, 20.0, 20.0);
        let mut dict = self.base("Text", r, colors::GOLD, AUTHOR, "Sticky notes hold threaded discussions. This one has two replies.", page);
        dict.set("F", 28); // Print | NoZoom | NoRotate
        dict.set("Name", "Comment");
        dict.set("Open", false);
        let icon = self.note_icon(doc, r, Rgb::hex(0xffd866));
        dict.set("AP", dictionary! { "N" => icon });

        let popup = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Popup",
            "Parent" => self_id,
            "P" => page,
            "Rect" => Rect::new((r.x1 + 8.0).min(412.0), r.y1 - 110.0, (r.x1 + 8.0).min(412.0) + 190.0, r.y1).obj(),
            "Open" => false,
            "F" => 28,
        });
        dict.set("Popup", popup);

        let mut reply = self.base(
            "Text",
            r,
            colors::TEAL,
            AUTHOR_2,
            "Agreed - and the appearance stream draws the icon, so every viewer shows the same thing.",
            page,
        );
        reply.set("F", 28);
        reply.set("Name", "Comment");
        reply.set("IRT", self_id);
        reply.set("RT", "R");
        reply.set("AP", dictionary! { "N" => icon });
        let reply_id = doc.add_object(reply);

        let mut state = self.base("Text", r, colors::TEAL, AUTHOR, "Accepted set by PdfCraft Reviewer", page);
        state.set("F", 30); // hidden: review-state annotations are shown only in the comments list
        state.set("Name", "Comment");
        state.set("IRT", self_id);
        state.set("State", Object::string_literal("Accepted"));
        state.set("StateModel", Object::string_literal("Review"));
        state.set("AP", dictionary! { "N" => icon });
        let state_id = doc.add_object(state);

        (dict, vec![popup, reply_id, state_id])
    }

    /// A standalone sticky note (no replies) at the given top-left corner.
    pub fn simple_note(&mut self, doc: &mut Document, page: ObjectId, x: f32, y_top: f32, contents: &str) -> ObjectId {
        let r = Rect::xywh(x, y_top - 20.0, 20.0, 20.0);
        let mut dict = self.base("Text", r, colors::GOLD, AUTHOR, contents, page);
        dict.set("F", 28);
        dict.set("Name", "Note");
        dict.set("Open", false);
        let icon = self.note_icon(doc, r, Rgb::hex(0xffd866));
        dict.set("AP", dictionary! { "N" => icon });
        let id = doc.new_object_id();
        let popup = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Popup", "Parent" => id, "P" => page,
            "Rect" => Rect::new(x - 200.0, y_top - 100.0, x - 10.0, y_top).obj(), "Open" => false, "F" => 28,
        });
        dict.set("Popup", popup);
        doc.objects.insert(id, Object::Dictionary(dict));
        id
    }

    fn free_text(&mut self, doc: &mut Document, page: ObjectId, r: Rect) -> Dictionary {
        let lines = ["FreeText: typed straight onto", "the page, with a /DA and rich text."];
        let contents = lines.join(" ");
        let mut dict = self.base("FreeText", r, colors::FIELD, AUTHOR, &contents, page);
        dict.set("DA", Object::string_literal("/Helv 9 Tf 0.086 0.129 0.231 rg"));
        dict.set("DS", Object::string_literal("font: Helvetica,sans-serif 9.0pt; text-align:left; color:#16213B"));
        dict.set(
            "RC",
            text(
                "<?xml version=\"1.0\"?><body xmlns=\"http://www.w3.org/1999/xhtml\"><p><b>FreeText:</b> \
                 typed straight onto the page, with a /DA and <span style=\"color:#E4572E\">rich text</span>.</p></body>",
            ),
        );
        dict.set("Q", 0);
        dict.set("BS", dictionary! { "W" => 1, "S" => "S" });
        let (w, h) = (r.w(), r.h());
        let mut c = Content::new();
        c.fill_color(colors::FIELD).stroke_color(colors::BLUE).line_width(1.0).rect(0.5, 0.5, w - 1.0, h - 1.0).fill_stroke();
        c.save().rect(2.0, 2.0, w - 4.0, h - 4.0).clip();
        c.fill_color(colors::INK);
        c.text(Font::HelvBold, 9.0, 6.0, h - 15.0, "FreeText:");
        let lead = Font::HelvBold.width("FreeText: ", 9.0);
        c.text(Font::Helv, 9.0, 6.0 + lead, h - 15.0, "typed straight onto");
        c.text(Font::Helv, 9.0, 6.0, h - 27.0, "the page, with a /DA and");
        c.fill_color(colors::ACCENT).text(Font::Helv, 9.0, 6.0 + Font::Helv.width("the page, with a /DA and ", 9.0), h - 27.0, "rich text.");
        c.restore();
        self.set_ap(doc, &mut dict, r, c, self.fonts.resources());
        dict
    }

    fn line(&mut self, doc: &mut Document, page: ObjectId, r: Rect) -> Dictionary {
        let (x0, y0, x1, y1) = (r.x0 + 6.0, r.y0 + 8.0, r.x1 - 6.0, r.y1 - 8.0);
        let mut dict = self.base("Line", r, colors::INK, AUTHOR, "Line annotation with a circle start and closed arrow end.", page);
        dict.set("L", floats(&[x0, y0, x1, y1]));
        dict.set("LE", vec![name("Circle"), name("ClosedArrow")]);
        dict.set("IC", colors::ACCENT.array());
        dict.set("BS", dictionary! { "W" => 1.5, "S" => "S" });
        let (lx0, ly0, lx1, ly1) = (x0 - r.x0, y0 - r.y0, x1 - r.x0, y1 - r.y0);
        let (dx, dy) = (lx1 - lx0, ly1 - ly0);
        let len = (dx * dx + dy * dy).sqrt();
        let (ux, uy) = (dx / len, dy / len);
        let (al, aw) = (11.0, 4.5);
        let (bx, by) = (lx1 - ux * al, ly1 - uy * al);
        let mut c = Content::new();
        c.stroke_color(colors::INK).line_width(1.5).round_caps();
        c.move_to(lx0, ly0).line_to(bx, by).stroke();
        c.fill_color(colors::ACCENT).move_to(lx1, ly1).line_to(bx - uy * aw, by + ux * aw).line_to(bx + uy * aw, by - ux * aw).close().fill_stroke();
        c.ellipse(lx0, ly0, 3.0, 3.0).fill_stroke();
        self.set_ap(doc, &mut dict, r, c, Dictionary::new());
        dict
    }

    fn shape(&mut self, doc: &mut Document, page: ObjectId, kind: Kind, r: Rect) -> Dictionary {
        let (subtype, stroke, fill, contents) = if kind == Kind::Square {
            ("Square", colors::TEAL, Rgb::hex(0xd8efe9), "Square annotation, dashed border, tinted interior.")
        } else {
            ("Circle", colors::ACCENT, Rgb::hex(0xfbeccc), "Circle annotation with interior colour.")
        };
        let bw = 2.0;
        let mut dict = self.base(subtype, r, stroke, AUTHOR, contents, page);
        dict.set("IC", fill.array());
        dict.set("CA", 0.85);
        let (w, h) = (r.w(), r.h());
        let mut c = Content::new();
        c.gs("A").stroke_color(stroke).fill_color(fill).line_width(bw);
        if kind == Kind::Square {
            dict.set("BS", dictionary! { "W" => bw, "S" => "D", "D" => vec![Object::from(4), Object::from(2)] });
            c.dash(4.0, 2.0).rect(bw / 2.0, bw / 2.0, w - bw, h - bw).fill_stroke();
        } else {
            dict.set("BS", dictionary! { "W" => bw, "S" => "S" });
            c.ellipse(w / 2.0, h / 2.0, w / 2.0 - bw / 2.0, h / 2.0 - bw / 2.0).fill_stroke();
        }
        let res = dictionary! {
            "ExtGState" => dictionary! { "A" => dictionary! { "Type" => "ExtGState", "CA" => 0.85, "ca" => 0.85 } },
        };
        self.set_ap(doc, &mut dict, r, c, res);
        dict
    }

    fn poly(&mut self, doc: &mut Document, page: ObjectId, kind: Kind, r: Rect) -> Dictionary {
        let (w, h) = (r.w(), r.h());
        let local: Vec<(f32, f32)> = if kind == Kind::Polygon {
            // An irregular hexagon.
            vec![(0.5, 0.95), (0.93, 0.7), (0.85, 0.18), (0.45, 0.05), (0.08, 0.28), (0.1, 0.72)]
        } else {
            vec![(0.06, 0.2), (0.3, 0.8), (0.5, 0.35), (0.72, 0.9), (0.94, 0.45)]
        }
        .into_iter()
        .map(|(fx, fy)| (4.0 + fx * (w - 8.0), 4.0 + fy * (h - 8.0)))
        .collect();
        let vertices: Vec<f32> = local.iter().flat_map(|&(x, y)| [r.x0 + x, r.y0 + y]).collect();

        let (subtype, stroke, contents) = if kind == Kind::Polygon {
            ("Polygon", colors::BLUE, "Polygon annotation (six vertices).")
        } else {
            ("PolyLine", colors::INK, "PolyLine annotation ending in an open arrow.")
        };
        let mut dict = self.base(subtype, r, stroke, AUTHOR, contents, page);
        dict.set("Vertices", floats(&vertices));
        dict.set("BS", dictionary! { "W" => 2, "S" => "S" });
        let mut c = Content::new();
        c.stroke_color(stroke).line_width(2.0).raw("1 j");
        c.move_to(local[0].0, local[0].1);
        for &(x, y) in &local[1..] {
            c.line_to(x, y);
        }
        if kind == Kind::Polygon {
            let fill = Rgb::hex(0xdbe6fa);
            dict.set("IC", fill.array());
            c.close().fill_color(fill).fill_stroke();
        } else {
            dict.set("LE", vec![name("None"), name("OpenArrow")]);
            c.stroke();
            let (a, b) = (local[local.len() - 2], local[local.len() - 1]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt();
            let (ux, uy) = (dx / len, dy / len);
            let (bx, by) = (b.0 - ux * 9.0, b.1 - uy * 9.0);
            c.round_caps().move_to(bx - uy * 5.0, by + ux * 5.0).line_to(b.0, b.1).line_to(bx + uy * 5.0, by - ux * 5.0).stroke();
        }
        self.set_ap(doc, &mut dict, r, c, Dictionary::new());
        dict
    }

    fn ink(&mut self, doc: &mut Document, page: ObjectId, r: Rect) -> Dictionary {
        let (w, h) = (r.w(), r.h());
        // A looping, cursive "signature" (a prolate trochoid) and an underline swoosh.
        let loops = 6.0 * std::f32::consts::TAU;
        let scribble: Vec<(f32, f32)> = (0..=360)
            .map(|i| {
                let t = i as f32 / 360.0;
                let x = 10.0 + t * (w * 0.8) - 7.0 * (loops * t).sin();
                let y = h * 0.62 + h * 0.24 * (loops * t).cos() * (1.0 - 0.35 * t);
                (x, y)
            })
            .collect();
        let swoosh: Vec<(f32, f32)> = (0..=30)
            .map(|i| {
                let t = i as f32 / 30.0;
                (w * 0.1 + t * w * 0.85, h * 0.14 + 5.0 * (t * std::f32::consts::PI).sin())
            })
            .collect();
        let mut dict = self.base("Ink", r, colors::ACCENT, AUTHOR, "Freehand ink: two strokes.", page);
        let ink_list: Vec<Object> =
            [&scribble, &swoosh].iter().map(|pts| floats(&pts.iter().flat_map(|&(x, y)| [r.x0 + x, r.y0 + y]).collect::<Vec<_>>())).collect();
        dict.set("InkList", ink_list);
        dict.set("BS", dictionary! { "W" => 1.8, "S" => "S" });
        let mut c = Content::new();
        c.stroke_color(colors::ACCENT).line_width(1.8).round_caps();
        for pts in [&scribble, &swoosh] {
            c.move_to(pts[0].0, pts[0].1);
            for &(x, y) in &pts[1..] {
                c.line_to(x, y);
            }
            c.stroke();
        }
        self.set_ap(doc, &mut dict, r, c, Dictionary::new());
        dict
    }

    fn stamp(&mut self, doc: &mut Document, page: ObjectId, r: Rect) -> Dictionary {
        let green = Rgb::hex(0x1d7f4e);
        let mut dict = self.base("Stamp", r, green, AUTHOR, "Approved for the showcase.", page);
        dict.set("Name", "PdfCraftApproved");
        dict.set("CA", 0.92);
        let (w, h) = (r.w(), r.h());
        let mut c = Content::new();
        // Rotate slightly about the centre, scaled so the corners stay inside the box.
        let (sin, cos) = (-4.0f32).to_radians().sin_cos();
        let s = 0.9;
        let (cx, cy) = (w / 2.0, h / 2.0);
        c.cm(cos * s, sin * s, -sin * s, cos * s, cx - s * (cos * cx - sin * cy), cy - s * (sin * cx + cos * cy));
        c.stroke_color(green).line_width(2.5).rounded_rect(2.0, 2.0, w - 4.0, h - 4.0, 8.0).stroke();
        c.line_width(0.8).rounded_rect(6.0, 6.0, w - 12.0, h - 12.0, 5.0).stroke();
        c.fill_color(green);
        let title = "APPROVED";
        let size = 22.0;
        let tw = Font::HelvBold.width(title, size) + 3.0 * (title.len() as f32 - 1.0);
        c.tracked_text(Font::HelvBold, size, (w - tw) / 2.0, h * 0.42, 3.0, title);
        let date = format!("PDFCRAFT QA  \u{2022}  {}", &self.now.iso[..10]);
        let dw = Font::Helv.width(&date, 7.0) + 1.0 * (date.chars().count() as f32 - 1.0);
        c.tracked_text(Font::Helv, 7.0, (w - dw) / 2.0, h * 0.2, 1.0, &date);
        let res = self.fonts.resources();
        self.set_ap(doc, &mut dict, r, c, res);
        dict
    }

    fn attachment(&mut self, doc: &mut Document, page: ObjectId, r: Rect) -> Dictionary {
        let note = "Review notes for the PdfCraft showcase.\n\n\
                    - Page 9 exercises every common annotation type.\n\
                    - Each annotation carries its own appearance stream.\n";
        let fs = embedded_file(doc, "review-notes.txt", "text/plain", note.as_bytes(), "Notes attached to a comment", &self.now);
        let mut dict = self.base("FileAttachment", r, colors::BLUE, AUTHOR, "Attached: review-notes.txt", page);
        dict.set("FS", fs);
        dict.set("Name", "Paperclip");
        dict.set("F", 28);
        let (w, h) = (r.w(), r.h());
        let mut c = Content::new();
        // A paperclip: nested U-shaped loops.
        c.stroke_color(colors::BLUE).line_width(1.6).round_caps();
        let (l, rr) = (w * 0.25, w * 0.75);
        c.move_to(w * 0.62, h * 0.3)
            .line_to(w * 0.62, h * 0.75)
            .curve_to(w * 0.62, h * 0.92, w * 0.38, h * 0.92, w * 0.38, h * 0.75)
            .line_to(w * 0.38, h * 0.2)
            .curve_to(w * 0.38, h * 0.08, l, h * 0.02, l, h * 0.2)
            .line_to(l, h * 0.82)
            .curve_to(l, h * 1.02, rr, h * 1.02, rr, h * 0.82)
            .line_to(rr, h * 0.25)
            .stroke();
        self.set_ap(doc, &mut dict, r, c, Dictionary::new());
        dict
    }

    /// A link annotation (no border) to a page or a URI.
    pub fn link(&mut self, doc: &mut Document, page: ObjectId, r: Rect, target: LinkTarget) -> ObjectId {
        let mut dict = dictionary! {
            "Type" => "Annot",
            "Subtype" => "Link",
            "Rect" => r.obj(),
            "P" => page,
            "F" => 4,
            "Border" => vec![Object::from(0), Object::from(0), Object::from(0)],
            "H" => "I",
        };
        match target {
            LinkTarget::Page(p) => dict.set("Dest", vec![Object::Reference(p), name("XYZ"), Object::Null, Object::Null, Object::Null]),
            LinkTarget::Uri(uri) => dict.set("A", dictionary! { "S" => "URI", "URI" => Object::string_literal(uri) }),
        }
        doc.add_object(dict)
    }
}

pub enum LinkTarget<'a> {
    Page(ObjectId),
    Uri(&'a str),
}

/// Create an embedded file stream and its file specification; returns the filespec id.
pub fn embedded_file(doc: &mut Document, file_name: &str, mime: &str, data: &[u8], desc: &str, now: &Stamp) -> ObjectId {
    let stream = Stream::new(
        dictionary! {
            "Type" => "EmbeddedFile",
            "Subtype" => name(mime),
            "Params" => dictionary! {
                "Size" => data.len() as i64,
                "ModDate" => Object::string_literal(now.pdf.clone()),
                "CreationDate" => Object::string_literal(now.pdf.clone()),
            },
        },
        data.to_vec(),
    );
    let ef = doc.add_object(stream);
    doc.add_object(dictionary! {
        "Type" => "Filespec",
        "F" => Object::string_literal(file_name),
        "UF" => text(file_name),
        "Desc" => text(desc),
        "AFRelationship" => "Supplement",
        "EF" => dictionary! { "F" => ef, "UF" => ef },
    })
}
