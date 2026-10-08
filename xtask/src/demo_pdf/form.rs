//! The appended "Interactive Form" page: layout drawn with standard fonts plus a
//! full AcroForm whose every widget carries its own appearance streams.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};

use super::annots::{Annotator, LinkTarget, Rect};
use super::pdf::{Content, Font, Fonts, Rgb, colors, form_xobject, name, text};

const PAGE_W: f32 = 612.0;
const PAGE_H: f32 = 792.0;
const LEFT: f32 = 61.2;
const RIGHT: f32 = PAGE_W - 61.2;
const COL2: f32 = 316.0;
const DA_TEXT: &str = "/Helv 10 Tf 0.086 0.129 0.231 rg";
const DA_ZAPF: &str = "/ZaDb 0 Tf 0.086 0.129 0.231 rg";

/// Result of building the form page.
pub struct FormPage {
    pub page: ObjectId,
    /// Top-level fields for `/AcroForm /Fields`.
    pub fields: Vec<ObjectId>,
}

/// Inputs the form page needs from the rest of the document.
pub struct FormInputs<'a> {
    pub fonts: &'a Fonts,
    pub pages_root: ObjectId,
    pub contents_page: ObjectId,
    pub print_only_ocg: ObjectId,
    pub folio: &'a str,
}

struct Builder<'a> {
    doc: &'a mut Document,
    fonts: &'a Fonts,
    page: ObjectId,
    widgets: Vec<ObjectId>,
    fields: Vec<ObjectId>,
}

pub fn build(doc: &mut Document, annotator: &mut Annotator, inputs: FormInputs<'_>) -> FormPage {
    let page = doc.new_object_id();
    let mut b = Builder { doc, fonts: inputs.fonts, page, widgets: Vec::new(), fields: Vec::new() };
    let mut c = Content::new();
    draw_page_chrome(&mut c, inputs.folio);

    // --- Contact -------------------------------------------------------------
    section(&mut c, 612.0, "Contact");
    let row1 = 575.0;
    label(&mut c, LEFT, row1 + 24.0, "Full name");
    b.text_field("name", "Full name", Rect::new(LEFT, row1, 296.0, row1 + 20.0), "Ada Typesetter", None, 0, |_| {});
    label(&mut c, COL2, row1 + 24.0, "Email");
    b.text_field("email", "Email address", Rect::new(COL2, row1, RIGHT, row1 + 20.0), "ada@example.com", None, 0, |_| {});

    let row2 = 531.0;
    label(&mut c, LEFT, row2 + 24.0, "Date  (AFDate_FormatEx yyyy-mm-dd)");
    b.text_field("date", "Date", Rect::new(LEFT, row2, 296.0, row2 + 20.0), "2026-09-30", None, 0, |d| {
        d.set("AA", js_actions("AFDate_FormatEx(\"yyyy-mm-dd\");", "AFDate_KeystrokeEx(\"yyyy-mm-dd\");"));
    });
    label(&mut c, COL2, row2 + 24.0, "Amount  (AFNumber_Format, right-aligned)");
    b.text_field("amount", "Amount in US dollars", Rect::new(COL2, row2, RIGHT, row2 + 20.0), "1234.5", Some("$1,234.50"), 2, |d| {
        d.set("AA", js_actions("AFNumber_Format(2, 0, 0, 0, \"$\", true);", "AFNumber_Keystroke(2, 0, 0, 0, \"$\", true);"));
    });

    label(&mut c, LEFT, 510.0, "Comments  (multiline)");
    let comments = "PdfCraft renders this field from its own appearance stream, so it looks \
                    the same in every viewer. Edit it to see your viewer regenerate the text.";
    b.text_field("comments", "Comments", Rect::new(LEFT, 446.0, RIGHT, 506.0), comments, None, 0, |d| {
        d.set("Ff", 4096);
    });

    // --- Preferences -----------------------------------------------------------
    section(&mut c, 418.0, "Preferences");
    let checks =
        [("subscribe", "Subscribe to release notes", true), ("license", "I have read the licence", true), ("stickers", "Send me stickers", false)];
    for (i, (field, caption, on)) in checks.into_iter().enumerate() {
        let y = 382.0 - i as f32 * 20.0;
        b.checkbox(field, caption, Rect::xywh(LEFT, y, 12.0, 12.0), on);
        c.fill_color(colors::INK).text(Font::Helv, 10.0, LEFT + 20.0, y + 2.5, caption);
    }
    label(&mut c, LEFT, 398.0, "Checkboxes");
    label(&mut c, COL2, 398.0, "Preferred output  (radio group)");
    let radios = [("PDFA", "PDF/A  -  archival"), ("PDFUA", "PDF/UA  -  accessible"), ("PDFX", "PDF/X  -  print exchange")];
    b.radio_group("output", "Preferred output profile", &radios, "PDFUA", &mut c);

    label(&mut c, LEFT, 320.0, "Paper size  (combo box)");
    b.combo("paper", "Paper size", Rect::new(LEFT, 296.0, 296.0, 316.0), &["Letter", "Legal", "Tabloid", "A4", "A3"], "Letter");
    label(&mut c, COL2, 320.0, "Features of interest  (multi-select list)");
    b.list_box(
        "interests",
        "Features of interest",
        Rect::new(COL2, 258.0, RIGHT, 316.0),
        &["Rendering", "Forms", "Annotations", "Signatures", "Accessibility"],
        &[1, 2],
    );

    // --- Actions & signature ----------------------------------------------------
    section(&mut c, 232.0, "Actions & signature");
    label(&mut c, LEFT, 212.0, "Push button  (JavaScript action)");
    b.push_button(
        "hello",
        "Run JavaScript: app.alert",
        Rect::new(LEFT, 184.0, 250.0, 206.0),
        "app.alert(\"Hello from the PdfCraft showcase! This alert is a JavaScript action on a push button.\", 3);",
    );
    c.fill_color(colors::INK_3).text(Font::Helv, 7.0, LEFT, 172.0, "JavaScript: shows app.alert(...) in viewers that run document scripts.");
    label(&mut c, COL2, 212.0, "Signature  (unsigned /Sig field)");
    b.signature("signature", "Approver signature", Rect::new(COL2, 150.0, RIGHT, 206.0));

    // --- Print-only note on its own optional-content layer ------------------------
    c.raw("/OC /OCNotes BDC");
    c.fill_color(Rgb::hex(0xfbeccc)).rounded_rect(LEFT, 98.0, 250.0 - LEFT, 56.0, 4.0).fill();
    c.fill_color(Rgb::hex(0x8a5a00));
    c.tracked_text(Font::HelvBold, 6.5, LEFT + 8.0, 142.0, 1.2, "PRINT-ONLY NOTES LAYER");
    c.text(Font::Helv, 8.0, LEFT + 8.0, 129.0, "Hidden on screen by default. Turn on the");
    c.text(Font::Helv, 8.0, LEFT + 8.0, 118.0, "\u{201c}Print-only notes\u{201d} layer, or print, to see it.");
    c.text(Font::Helv, 8.0, LEFT + 8.0, 107.0, "Office use: batch 2026-A, reviewed.");
    c.raw("EMC");

    // --- Links -----------------------------------------------------------------
    let link_y = 118.0;
    c.fill_color(colors::INK_2).text(Font::Helv, 9.0, COL2, link_y + 12.0, "Links");
    c.fill_color(colors::BLUE).text(Font::Helv, 10.0, COL2, link_y - 2.0, "Back to the contents");
    let back_w = Font::Helv.width("Back to the contents", 10.0);
    let visit = "https://example.com/";
    c.fill_color(colors::BLUE).text(Font::Helv, 10.0, COL2, link_y - 18.0, visit);
    let visit_w = Font::Helv.width(visit, 10.0);

    // --- Page object -------------------------------------------------------------
    let mut annots = b.widgets.clone();
    let fields = b.fields.clone();
    annots.push(annotator.link(
        b.doc,
        page,
        Rect::new(COL2 - 1.0, link_y - 5.0, COL2 + back_w + 1.0, link_y + 9.0),
        LinkTarget::Page(inputs.contents_page),
    ));
    annots.push(annotator.link(b.doc, page, Rect::new(COL2 - 1.0, link_y - 21.0, COL2 + visit_w + 1.0, link_y - 7.0), LinkTarget::Uri(visit)));
    annots.push(annotator.simple_note(
        b.doc,
        page,
        RIGHT + 14.0,
        700.0,
        "Every widget on this page has /AP normal appearances, so NeedAppearances is false.",
    ));

    let content_id = b.doc.add_object(Stream::new(Dictionary::new(), c.into_bytes()));
    let resources = dictionary! {
        "Font" => inputs.fonts.dict(),
        "Properties" => dictionary! { "OCNotes" => inputs.print_only_ocg },
    };
    let page_dict = dictionary! {
        "Type" => "Page",
        "Parent" => inputs.pages_root,
        "MediaBox" => vec![0.into(), 0.into(), PAGE_W.into(), PAGE_H.into()],
        "Contents" => content_id,
        "Resources" => resources,
        "Annots" => annots.into_iter().map(Object::Reference).collect::<Vec<_>>(),
        "Tabs" => "R",
    };
    b.doc.objects.insert(page, Object::Dictionary(page_dict));
    FormPage { page, fields }
}

/// Background, running head, title block and folio, matching the Chrome-printed pages.
fn draw_page_chrome(c: &mut Content, folio: &str) {
    c.fill_color(colors::PAPER).rect(0.0, 0.0, PAGE_W, PAGE_H).fill();
    c.fill_color(colors::INK_3);
    c.tracked_text(Font::Helv, 6.5, LEFT, 757.0, 1.2, "PDFCRAFT SHOWCASE");
    let right = "10  \u{00b7}  INTERACTIVE FORM";
    let w = Font::Helv.width(right, 6.5) + 1.2 * (right.chars().count() as f32 - 1.0);
    c.tracked_text(Font::Helv, 6.5, RIGHT - w, 757.0, 1.2, right);
    c.stroke_color(colors::RULE).line_width(0.5).move_to(LEFT, 751.0).line_to(RIGHT, 751.0).stroke();

    c.fill_color(colors::ACCENT).tracked_text(Font::HelvBold, 7.5, LEFT, 722.0, 1.8, "CHAPTER TEN");
    c.fill_color(colors::INK).text(Font::Times, 34.0, LEFT, 686.0, "Interactive");
    let w = Font::Times.width("Interactive", 34.0) + 12.0;
    c.fill_color(colors::ACCENT).text(Font::TimesItalic, 34.0, LEFT + w, 686.0, "Form");
    c.fill_color(colors::INK_2);
    c.text(Font::TimesItalic, 13.0, LEFT, 660.0, "This page was not printed by Chrome: the xtask writes its content stream and");
    c.text(Font::TimesItalic, 13.0, LEFT, 644.0, "an AcroForm directly. Every widget ships its own appearance streams.");

    c.fill_color(colors::INK_3).text(Font::Helv, 8.0, LEFT, 32.0, "Interactive Form");
    let w = Font::HelvBold.width(folio, 9.0);
    c.fill_color(colors::INK).text(Font::HelvBold, 9.0, RIGHT - w, 32.0, folio);
}

fn section(c: &mut Content, y: f32, title: &str) {
    c.fill_color(colors::INK).tracked_text(Font::HelvBold, 8.0, LEFT, y, 1.3, &title.to_uppercase());
    c.stroke_color(colors::RULE).line_width(0.5).move_to(LEFT, y - 6.0).line_to(RIGHT, y - 6.0).stroke();
}

fn label(c: &mut Content, x: f32, y: f32, s: &str) {
    c.fill_color(colors::INK_3).tracked_text(Font::Helv, 6.5, x, y, 0.6, &s.to_uppercase());
}

fn js(code: &str) -> Dictionary {
    dictionary! { "S" => "JavaScript", "JS" => Object::string_literal(code) }
}

/// Format (/F) and keystroke (/K) additional actions.
fn js_actions(format: &str, keystroke: &str) -> Dictionary {
    dictionary! { "F" => js(format), "K" => js(keystroke) }
}

/// Field background and border shared by every widget.
fn field_box(c: &mut Content, w: f32, h: f32) {
    c.fill_color(colors::FIELD).stroke_color(colors::INK_3).line_width(0.75).rect(0.375, 0.375, w - 0.75, h - 0.75).fill_stroke();
}

fn mk(extra: Dictionary) -> Dictionary {
    let mut d = dictionary! { "BC" => colors::INK_3.array(), "BG" => colors::FIELD.array() };
    d.extend(&extra);
    d
}

impl Builder<'_> {
    fn widget_base(&self, r: Rect) -> Dictionary {
        dictionary! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "Rect" => r.obj(),
            "P" => self.page,
            "F" => 4,
            "BS" => dictionary! { "W" => 1, "S" => "S" },
        }
    }

    fn xobject(&mut self, w: f32, h: f32, c: Content) -> ObjectId {
        form_xobject(self.doc, w, h, c, self.fonts.resources())
    }

    fn add_field(&mut self, dict: Dictionary) -> ObjectId {
        let id = self.doc.add_object(dict);
        self.widgets.push(id);
        self.fields.push(id);
        id
    }

    /// A text field; `display` overrides the appearance text (for formatted values),
    /// `quadding` is 0 left, 1 centre, 2 right.
    #[allow(clippy::too_many_arguments)]
    fn text_field(
        &mut self,
        field: &str,
        tooltip: &str,
        r: Rect,
        value: &str,
        display: Option<&str>,
        quadding: i64,
        customise: impl FnOnce(&mut Dictionary),
    ) {
        let (w, h) = (r.w(), r.h());
        let mut d = self.widget_base(r);
        d.set("FT", "Tx");
        d.set("T", text(field));
        d.set("TU", text(tooltip));
        d.set("V", text(value));
        d.set("DA", Object::string_literal(DA_TEXT));
        d.set("Q", quadding);
        d.set("MK", mk(Dictionary::new()));
        customise(&mut d);
        let multiline = d.get(b"Ff").and_then(Object::as_i64).is_ok_and(|ff| ff & 4096 != 0);

        let shown = display.unwrap_or(value);
        let mut c = Content::new();
        field_box(&mut c, w, h);
        c.raw("/Tx BMC").save().rect(1.0, 1.0, w - 2.0, h - 2.0).clip();
        c.fill_color(colors::INK);
        if multiline {
            for (i, line) in wrap(shown, Font::Helv, 10.0, w - 8.0).iter().enumerate() {
                c.text(Font::Helv, 10.0, 4.0, h - 13.0 - 12.0 * i as f32, line);
            }
        } else {
            let tw = Font::Helv.width(shown, 10.0);
            let x = match quadding {
                1 => (w - tw) / 2.0,
                2 => w - 4.0 - tw,
                _ => 4.0,
            };
            c.text(Font::Helv, 10.0, x, (h - 7.0) / 2.0, shown);
        }
        c.restore().raw("EMC");
        let ap = self.xobject(w, h, c);
        d.set("AP", dictionary! { "N" => ap });
        self.add_field(d);
    }

    fn check_appearances(&mut self, w: f32, h: f32, on_state: &str, round: bool) -> Dictionary {
        let mut on = Content::new();
        let mut off = Content::new();
        for c in [&mut on, &mut off] {
            c.fill_color(colors::FIELD).stroke_color(colors::INK_3).line_width(0.75);
            if round {
                c.ellipse(w / 2.0, h / 2.0, w / 2.0 - 0.5, h / 2.0 - 0.5).fill_stroke();
            } else {
                c.rect(0.375, 0.375, w - 0.75, h - 0.75).fill_stroke();
            }
        }
        if round {
            on.fill_color(colors::ACCENT).ellipse(w / 2.0, h / 2.0, w * 0.25, h * 0.25).fill();
        } else {
            // ZapfDingbats "4" is a check mark (glyph a20, width 760).
            let size = h * 0.8;
            on.fill_color(colors::ACCENT).text(Font::ZapfDingbats, size, (w - 0.76 * size) / 2.0, h * 0.2, "4");
        }
        let on_id = self.xobject(w, h, on);
        let off_id = self.xobject(w, h, off);
        let mut states = Dictionary::new();
        states.set(on_state, on_id);
        states.set("Off", off_id);
        states
    }

    fn checkbox(&mut self, field: &str, tooltip: &str, r: Rect, checked: bool) {
        let state = if checked { "Yes" } else { "Off" };
        let mut d = self.widget_base(r);
        d.set("FT", "Btn");
        d.set("T", text(field));
        d.set("TU", text(tooltip));
        d.set("V", name(state));
        d.set("AS", name(state));
        d.set("DA", Object::string_literal(DA_ZAPF));
        d.set("MK", mk(dictionary! { "CA" => Object::string_literal("4") }));
        let n = self.check_appearances(r.w(), r.h(), "Yes", false);
        d.set("AP", dictionary! { "N" => n });
        self.add_field(d);
    }

    fn radio_group(&mut self, field: &str, tooltip: &str, options: &[(&str, &str)], selected: &str, c: &mut Content) {
        let parent = self.doc.new_object_id();
        let mut kids = Vec::new();
        for (i, (export, caption)) in options.iter().enumerate() {
            let y = 382.0 - i as f32 * 20.0;
            let r = Rect::xywh(COL2, y, 12.0, 12.0);
            let state = if *export == selected { *export } else { "Off" };
            let mut d = self.widget_base(r);
            d.set("Parent", parent);
            d.set("AS", name(state));
            d.set("MK", mk(dictionary! { "CA" => Object::string_literal("l") }));
            d.set("DA", Object::string_literal(DA_ZAPF));
            let n = self.check_appearances(12.0, 12.0, export, true);
            d.set("AP", dictionary! { "N" => n });
            let id = self.doc.add_object(d);
            self.widgets.push(id);
            kids.push(Object::Reference(id));
            c.fill_color(colors::INK).text(Font::Helv, 10.0, COL2 + 20.0, y + 2.5, caption);
        }
        self.doc.objects.insert(
            parent,
            Object::Dictionary(dictionary! {
                "FT" => "Btn",
                "Ff" => 49152, // Radio | NoToggleToOff
                "T" => text(field),
                "TU" => text(tooltip),
                "V" => name(selected),
                "Kids" => kids,
            }),
        );
        self.fields.push(parent);
    }

    fn combo(&mut self, field: &str, tooltip: &str, r: Rect, options: &[&str], value: &str) {
        let (w, h) = (r.w(), r.h());
        let mut d = self.widget_base(r);
        d.set("FT", "Ch");
        d.set("Ff", 131_072); // Combo
        d.set("T", text(field));
        d.set("TU", text(tooltip));
        d.set("Opt", options.iter().map(|o| text(o)).collect::<Vec<_>>());
        d.set("V", text(value));
        d.set("DA", Object::string_literal(DA_TEXT));
        d.set("MK", mk(Dictionary::new()));
        let mut c = Content::new();
        field_box(&mut c, w, h);
        let bw = h;
        c.fill_color(colors::RULE).rect(w - bw, 0.75, bw - 0.75, h - 1.5).fill();
        c.fill_color(colors::INK)
            .move_to(w - bw / 2.0 - 4.0, h / 2.0 + 2.0)
            .line_to(w - bw / 2.0 + 4.0, h / 2.0 + 2.0)
            .line_to(w - bw / 2.0, h / 2.0 - 3.0)
            .close()
            .fill();
        c.raw("/Tx BMC").save().rect(1.0, 1.0, w - bw - 2.0, h - 2.0).clip();
        c.fill_color(colors::INK).text(Font::Helv, 10.0, 4.0, (h - 7.0) / 2.0, value);
        c.restore().raw("EMC");
        let ap = self.xobject(w, h, c);
        d.set("AP", dictionary! { "N" => ap });
        self.add_field(d);
    }

    fn list_box(&mut self, field: &str, tooltip: &str, r: Rect, options: &[&str], selected: &[usize]) {
        let (w, h) = (r.w(), r.h());
        let mut d = self.widget_base(r);
        d.set("FT", "Ch");
        d.set("Ff", 2_097_152); // MultiSelect
        d.set("T", text(field));
        d.set("TU", text(tooltip));
        d.set("Opt", options.iter().map(|o| text(o)).collect::<Vec<_>>());
        d.set("V", selected.iter().map(|&i| text(options[i])).collect::<Vec<_>>());
        d.set("I", selected.iter().map(|&i| Object::from(i as i64)).collect::<Vec<_>>());
        d.set("TI", 0);
        d.set("DA", Object::string_literal(DA_TEXT));
        d.set("MK", mk(Dictionary::new()));
        let row = 13.5;
        let mut c = Content::new();
        field_box(&mut c, w, h);
        c.raw("/Tx BMC").save().rect(1.0, 1.0, w - 2.0, h - 2.0).clip();
        for (i, option) in options.iter().enumerate() {
            let top = h - 1.5 - row * i as f32;
            if selected.contains(&i) {
                c.fill_color(Rgb::hex(0xd6e2f8)).rect(1.0, top - row, w - 2.0, row).fill();
            }
            c.fill_color(colors::INK).text(Font::Helv, 10.0, 5.0, top - row + 3.8, option);
        }
        c.restore().raw("EMC");
        let ap = self.xobject(w, h, c);
        d.set("AP", dictionary! { "N" => ap });
        self.add_field(d);
    }

    fn push_button(&mut self, field: &str, caption: &str, r: Rect, script: &str) {
        let (w, h) = (r.w(), r.h());
        let mut d = self.widget_base(r);
        d.set("FT", "Btn");
        d.set("Ff", 65_536); // Pushbutton
        d.set("T", text(field));
        d.set("TU", text("Runs a JavaScript app.alert"));
        d.set("DA", Object::string_literal("/HeBo 9 Tf 1 g"));
        d.set("H", "P");
        d.set("MK", dictionary! { "BG" => colors::INK.array(), "CA" => text(caption) });
        d.set("A", js(script));
        let mut appearances = Dictionary::new();
        for (key, fill) in [("N", colors::INK), ("D", colors::ACCENT)] {
            let mut c = Content::new();
            c.fill_color(fill).rounded_rect(0.0, 0.0, w, h, 4.0).fill();
            let tw = Font::HelvBold.width(caption, 9.0);
            c.fill_color(colors::WHITE).text(Font::HelvBold, 9.0, (w - tw) / 2.0, (h - 6.5) / 2.0, caption);
            let id = self.xobject(w, h, c);
            appearances.set(key, id);
        }
        d.set("AP", appearances);
        self.add_field(d);
    }

    fn signature(&mut self, field: &str, tooltip: &str, r: Rect) {
        let (w, h) = (r.w(), r.h());
        let mut d = self.widget_base(r);
        d.set("FT", "Sig");
        d.set("T", text(field));
        d.set("TU", text(tooltip));
        d.set("DA", Object::string_literal(DA_TEXT));
        d.set("MK", mk(Dictionary::new()));
        let mut c = Content::new();
        c.fill_color(colors::FIELD).stroke_color(colors::BLUE).line_width(1.0).dash(3.0, 2.0).rect(0.5, 0.5, w - 1.0, h - 1.0).fill_stroke();
        c.raw("[] 0 d").stroke_color(colors::INK_3).line_width(0.5).move_to(24.0, 16.0).line_to(w - 12.0, 16.0).stroke();
        c.fill_color(colors::BLUE).text(Font::HelvBold, 12.0, 10.0, 19.0, "\u{00d7}");
        c.fill_color(colors::INK_3).text(Font::Helv, 7.0, 24.0, 6.0, "Sign here  -  unsigned signature field");
        let ap = self.xobject(w, h, c);
        d.set("AP", dictionary! { "N" => ap });
        self.add_field(d);
    }
}

/// Greedy word wrap using the standard-font metrics.
fn wrap(s: &str, font: Font, size: f32, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if font.width(&candidate, size) > width && !line.is_empty() {
            lines.push(std::mem::replace(&mut line, word.to_string()));
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The `/AcroForm` dictionary for `fields`.
pub fn acroform(fonts: &Fonts, fields: &[ObjectId]) -> Dictionary {
    dictionary! {
        "Fields" => fields.iter().map(|&id| Object::Reference(id)).collect::<Vec<_>>(),
        "DR" => dictionary! { "Font" => fonts.dict() },
        "DA" => Object::string_literal("/Helv 0 Tf 0 g"),
        "NeedAppearances" => false,
    }
}
