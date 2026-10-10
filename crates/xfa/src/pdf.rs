//! Turn laid-out pages into PDF objects inside an open document: page dictionaries with content
//! streams (standard-14 fonts, WinAnsi), image XObjects and AcroForm widgets. Widget appearance
//! streams are left empty here; the forms layer regenerates them from the field dictionaries.

use std::collections::{BTreeMap, HashMap};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};
use pdfcraft_fonts::{literal, win_ansi};

use crate::XfaError;
use crate::layout::{Action, BorderShape, Form, Item, Page, Widget, WidgetKind};
use crate::model::Rect;
use crate::text::{Face, Family};

/// Private key on generated fields: the template's SOM path, so data can be put back.
pub const SOM_KEY: &[u8] = b"PCSom";
/// Private key on the AcroForm: what laying the form out produced, so a reopened file is not
/// laid out again.
pub const LAYOUT_KEY: &[u8] = b"PCXfaLayout";
/// Private key on generated push buttons whose template has a click script: the button has no
/// PDF action (the script is XFA's, not Acrobat JavaScript); PdfCraft runs the script by the
/// field's SOM path.
pub const CLICK_KEY: &[u8] = b"PCXfaClick";

/// Width and height of a JPEG from its first frame header (SOF0–SOF15, not the DNL/JPG/DAC/DHT
/// markers). `None` for anything else.
pub fn jpeg_size(data: &[u8]) -> Option<(u32, u32)> {
    jpeg_frame(data).map(|(w, h, _)| (w, h))
}

fn jpeg_frame(data: &[u8]) -> Option<(u32, u32, u8)> {
    jpeg_info(data).map(|(w, h, c, _)| (w, h, c))
}

/// The SHA-256 of a picture's bytes, to find its XObject again.
fn content_hash(data: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(data).into()
}

fn hex32(h: &[u8; 32]) -> Vec<u8> {
    h.iter().flat_map(|b| format!("{b:02x}").into_bytes()).collect()
}

fn parse_hex32(s: &[u8]) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    if s.len() != 64 {
        return None;
    }
    for (o, pair) in out.iter_mut().zip(s.as_chunks::<2>().0) {
        *o = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

/// Width, height, components and whether an Adobe APP14 segment is present (such CMYK JPEGs
/// store inverted values).
fn jpeg_info(data: &[u8]) -> Option<(u32, u32, u8, bool)> {
    if data.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2usize;
    let mut guard = 0;
    let mut adobe = false;
    while i + 4 <= data.len() && guard < 10_000 {
        guard += 1;
        if *data.get(i)? != 0xFF {
            i += 1;
            continue;
        }
        let marker = *data.get(i + 1)?;
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([*data.get(i + 2)?, *data.get(i + 3)?]) as usize;
        if marker == 0xEE && data.get(i + 4..i + 9) == Some(b"Adobe") {
            adobe = true;
        }
        let sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if sof {
            let h = u16::from_be_bytes([*data.get(i + 5)?, *data.get(i + 6)?]) as u32;
            let w = u16::from_be_bytes([*data.get(i + 7)?, *data.get(i + 8)?]) as u32;
            let comps = *data.get(i + 9)?;
            return (w > 0 && h > 0).then_some((w, h, comps, adobe));
        }
        i = i.checked_add(2)?.checked_add(len.max(2))?;
    }
    None
}

/// Format a number for content streams.
fn n(v: f64) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn rgb(c: [f64; 3]) -> String {
    format!("{} {} {}", n(c[0]), n(c[1]), n(c[2]))
}

fn rgb_array(c: [f64; 3]) -> Object {
    Object::Array(c.iter().map(|v| Object::Real(v.clamp(0.0, 1.0))).collect())
}

fn standard_font(base: &str) -> Object {
    let mut f = Dict::new();
    f.set(b"Type".to_vec(), Object::name("Font"));
    f.set(b"Subtype".to_vec(), Object::name("Type1"));
    f.set(b"BaseFont".to_vec(), Object::name(base));
    f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    Object::Dict(f)
}

/// Every face the content streams or `/DA` strings may name.
fn all_faces() -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    for fam in [Family::Helvetica, Family::Courier, Family::Times] {
        for (b, i) in [(false, false), (true, false), (false, true), (true, true)] {
            out.push((fam.resource_name(b, i), fam.base_font(b, i)));
        }
    }
    out
}

/// A PDF appearance-state name for a check button's on value.
fn state_name(on: &str) -> String {
    let s: String = on.trim().chars().map(|c| if c.is_whitespace() || "()<>[]{}/%#".contains(c) || c.is_control() { '_' } else { c }).collect();
    if s.is_empty() || s == "Off" { "Yes".into() } else { s }
}

fn empty_form(doc: &mut Document, w: f64, h: f64) -> Object {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array([0.0, 0.0, w, h].iter().map(|v| Object::Real(*v)).collect()));
    Object::Ref(doc.add(Object::Stream(Stream::from_raw(d, Vec::new()))))
}

fn js_action(js: &str) -> Object {
    let mut a = Dict::new();
    a.set(b"S".to_vec(), Object::name("JavaScript"));
    a.set(b"JS".to_vec(), Object::String(PdfString::literal(js.as_bytes().to_vec())));
    Object::Dict(a)
}

pub struct Written {
    pub pages: usize,
    pub fields: usize,
    pub warnings: Vec<String>,
}

/// On an image XObject written for a picture: the SHA-256 (hex) of the picture's bytes. A
/// re-layout (after every script event) finds the XObjects on the pages it replaces by this,
/// and reuses them instead of decoding and embedding the same picture again. Nothing is kept
/// in the catalog, and a match is by the picture's own hash, never by a map read from the file.
const IMAGE_SHA_KEY: &[u8] = b"PCXfaSHA256";
/// Most pages searched for earlier pictures.
const MAX_PAGES_SEARCHED: usize = 10_000;
/// Most pictures remembered.
const MAX_REMEMBERED_IMAGES: usize = 1000;

struct Emitter<'a> {
    doc: &'a mut Document,
    /// Image XObjects by content hash: those from earlier passes, then this one's.
    images: HashMap<[u8; 32], ObjRef>,
    fields: Vec<ObjRef>,
    radio_groups: HashMap<String, (ObjRef, Vec<ObjRef>)>,
    /// The on state selected in each radio group, from the data.
    radio_selected: HashMap<String, String>,
    field_count: usize,
    warnings: Vec<String>,
}

impl Emitter<'_> {
    fn da(face: &Face) -> Vec<u8> {
        format!("/{} {} Tf {} rg", face.resource_name(), n(face.size), rgb(face.color)).into_bytes()
    }

    fn widget(&mut self, w: &Widget, page: &Page, page_ref: ObjRef) -> Result<Option<ObjRef>, XfaError> {
        let r = w.rect;
        let (x0, y0, x1, y1) = (r.x, page.height - r.bottom(), r.right(), page.height - r.y);
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Annot"));
        d.set(b"Subtype".to_vec(), Object::name("Widget"));
        d.set(b"F".to_vec(), Object::Int(4));
        d.set(b"P".to_vec(), Object::Ref(page_ref));
        d.set(b"Rect".to_vec(), Object::Array([x0, y0, x1, y1].iter().map(|v| Object::Real(*v)).collect()));
        d.set(b"DA".to_vec(), Object::String(PdfString::literal(Self::da(&w.face))));
        if let Some(t) = &w.tooltip {
            d.set(b"TU".to_vec(), Object::String(PdfString::text(t)));
        }
        d.set(SOM_KEY.to_vec(), Object::String(PdfString::text(&w.som)));
        let mut mk = Dict::new();
        if let Some(c) = w.border.color
            && w.border.shape != BorderShape::None
        {
            mk.set(b"BC".to_vec(), rgb_array(c));
        }
        if let Some(c) = w.border.fill {
            mk.set(b"BG".to_vec(), rgb_array(c));
        }
        let mut bs = Dict::new();
        bs.set(b"W".to_vec(), Object::Real(if w.border.shape == BorderShape::None { 0.0 } else { w.border.width.clamp(0.1, 12.0) }));
        bs.set(b"S".to_vec(), Object::name(if w.border.shape == BorderShape::Underline { "U" } else { "S" }));
        d.set(b"BS".to_vec(), Object::Dict(bs));
        let mut ff: i64 = 0;
        if w.read_only {
            ff |= 1;
        }
        let mut top_level = true;
        match &w.kind {
            WidgetKind::Text | WidgetKind::Date(_) | WidgetKind::Password => {
                d.set(b"FT".to_vec(), Object::name("Tx"));
                if w.multiline {
                    ff |= 1 << 12;
                }
                if w.kind == WidgetKind::Password {
                    ff |= 1 << 13;
                }
                if let Some(m) = w.max_chars {
                    d.set(b"MaxLen".to_vec(), Object::Int(m.min(100_000) as i64));
                }
                let q = match w.h_align {
                    crate::model::HAlign::Center => 1,
                    crate::model::HAlign::Right => 2,
                    _ => 0,
                };
                if q != 0 {
                    d.set(b"Q".to_vec(), Object::Int(q));
                }
                if let Some(v) = &w.value {
                    d.set(b"V".to_vec(), Object::String(PdfString::text(v)));
                }
                if let Some(v) = &w.default {
                    d.set(b"DV".to_vec(), Object::String(PdfString::text(v)));
                }
                if let WidgetKind::Date(pattern) = &w.kind {
                    let mut aa = Dict::new();
                    aa.set(b"F".to_vec(), js_action(&format!("AFDate_FormatEx(\"{pattern}\");")));
                    aa.set(b"K".to_vec(), js_action(&format!("AFDate_KeystrokeEx(\"{pattern}\");")));
                    d.set(b"AA".to_vec(), Object::Dict(aa));
                }
            }
            WidgetKind::Choice { options, list_box, multi, editable } => {
                d.set(b"FT".to_vec(), Object::name("Ch"));
                if !list_box {
                    ff |= 1 << 17;
                    if *editable {
                        ff |= 1 << 18;
                    }
                }
                if *multi {
                    ff |= 1 << 21;
                }
                let opt: Vec<Object> = options
                    .iter()
                    .map(|(saved, shown)| {
                        if saved == shown {
                            Object::String(PdfString::text(shown))
                        } else {
                            Object::Array(vec![Object::String(PdfString::text(saved)), Object::String(PdfString::text(shown))])
                        }
                    })
                    .collect();
                d.set(b"Opt".to_vec(), Object::Array(opt));
                let q = match w.h_align {
                    crate::model::HAlign::Center => 1,
                    crate::model::HAlign::Right => 2,
                    _ => 0,
                };
                if q != 0 {
                    d.set(b"Q".to_vec(), Object::Int(q));
                }
                let values = |v: &str| -> Object {
                    let picked: Vec<&str> = if *multi { v.lines().filter(|l| !l.is_empty()).collect() } else { vec![v] };
                    match picked.as_slice() {
                        [one] => Object::String(PdfString::text(one)),
                        many => Object::Array(many.iter().map(|s| Object::String(PdfString::text(s))).collect()),
                    }
                };
                if let Some(v) = &w.value {
                    d.set(b"V".to_vec(), values(v));
                    // The selected indices (ISO 32000-2 Table 231): ascending, each once. Where
                    // two options save the same value the first is meant; the data can't say.
                    let picked: Vec<&str> = if *multi { v.lines().filter(|l| !l.is_empty()).collect() } else { vec![v.as_str()] };
                    let mut idx: Vec<usize> = picked.iter().filter_map(|p| options.iter().position(|(saved, _)| saved == p)).collect();
                    idx.sort_unstable();
                    idx.dedup();
                    if !idx.is_empty() {
                        d.set(b"I".to_vec(), Object::Array(idx.into_iter().map(|i| Object::Int(i as i64)).collect()));
                    }
                }
                if let Some(v) = &w.default {
                    // Already a saved value (the layout maps defaults as it maps data).
                    d.set(b"DV".to_vec(), values(v));
                }
            }
            WidgetKind::Signature => {
                d.set(b"FT".to_vec(), Object::name("Sig"));
            }
            WidgetKind::CheckBox { on, .. } => {
                d.set(b"FT".to_vec(), Object::name("Btn"));
                let on = state_name(on);
                let checked = w.value.is_some();
                d.set(b"V".to_vec(), Object::name(if checked { &on } else { "Off" }));
                d.set(b"AS".to_vec(), Object::name(if checked { &on } else { "Off" }));
                if w.default.as_deref().is_some_and(|dv| crate::data::is_on(dv, &on)) {
                    d.set(b"DV".to_vec(), Object::name(&on));
                }
                let mut nd = Dict::new();
                nd.set(on.into_bytes(), empty_form(self.doc, r.w, r.h));
                nd.set(b"Off".to_vec(), empty_form(self.doc, r.w, r.h));
                let mut ap = Dict::new();
                ap.set(b"N".to_vec(), Object::Dict(nd));
                d.set(b"AP".to_vec(), Object::Dict(ap));
                d.set(b"PCItems".to_vec(), Object::Array(w.items.iter().map(|i| Object::String(PdfString::text(i))).collect()));
            }
            WidgetKind::Radio { group, on, .. } => {
                top_level = false;
                let parent = match self.radio_groups.get(group) {
                    Some((p, _)) => *p,
                    None => {
                        let mut g = Dict::new();
                        g.set(b"FT".to_vec(), Object::name("Btn"));
                        g.set(b"Ff".to_vec(), Object::Int((1 << 15) | (1 << 14)));
                        g.set(b"T".to_vec(), Object::String(PdfString::text(group)));
                        g.set(b"V".to_vec(), Object::name("Off"));
                        g.set(b"DA".to_vec(), Object::String(PdfString::literal(Self::da(&w.face))));
                        g.set(b"Kids".to_vec(), Object::Array(Vec::new()));
                        // The group's SOM path: the button's without its last step.
                        let group_som = w.som.rsplit_once('.').map(|(g, _)| g.to_string()).unwrap_or_else(|| w.som.clone());
                        g.set(SOM_KEY.to_vec(), Object::String(PdfString::text(&group_som)));
                        let p = self.doc.add(Object::Dict(g));
                        self.radio_groups.insert(group.clone(), (p, Vec::new()));
                        self.fields.push(p);
                        self.field_count += 1;
                        p
                    }
                };
                d.set(b"Parent".to_vec(), Object::Ref(parent));
                let state = state_name(on);
                let chosen = w.value.is_some();
                d.set(b"AS".to_vec(), Object::name(if chosen { &state } else { "Off" }));
                if chosen {
                    self.radio_selected.insert(group.clone(), state.clone());
                }
                let mut nd = Dict::new();
                nd.set(state.into_bytes(), empty_form(self.doc, r.w, r.h));
                nd.set(b"Off".to_vec(), empty_form(self.doc, r.w, r.h));
                let mut ap = Dict::new();
                ap.set(b"N".to_vec(), Object::Dict(nd));
                d.set(b"AP".to_vec(), Object::Dict(ap));
                d.remove(b"DA");
            }
            WidgetKind::Button { caption } => {
                d.set(b"FT".to_vec(), Object::name("Btn"));
                ff |= 1 << 16;
                mk.set(b"CA".to_vec(), Object::String(PdfString::text(caption)));
                if let Some(Action::Script(_)) = &w.action {
                    // An XFA click script is no Acrobat JavaScript: no PDF action, which other
                    // viewers would run; PdfCraft finds the script by the field's SOM path.
                    d.set(CLICK_KEY.to_vec(), Object::Bool(true));
                } else if let Some(a) = &w.action {
                    let mut ad = Dict::new();
                    match a {
                        Action::Reset => {
                            ad.set(b"S".to_vec(), Object::name("ResetForm"));
                        }
                        Action::Print => {
                            ad.set(b"S".to_vec(), Object::name("Named"));
                            ad.set(b"N".to_vec(), Object::name("Print"));
                        }
                        Action::SaveAs => {
                            ad.set(b"S".to_vec(), Object::name("Named"));
                            ad.set(b"N".to_vec(), Object::name("SaveAs"));
                        }
                        Action::Url(u) => {
                            ad.set(b"S".to_vec(), Object::name("URI"));
                            ad.set(b"URI".to_vec(), Object::String(PdfString::literal(u.as_bytes().to_vec())));
                        }
                        Action::Script(_) => {}
                    }
                    d.set(b"A".to_vec(), Object::Dict(ad));
                }
            }
        }
        if !mk.is_empty() {
            d.set(b"MK".to_vec(), Object::Dict(mk));
        }
        if ff != 0 {
            d.set(b"Ff".to_vec(), Object::Int(ff));
        }
        if top_level {
            d.set(b"T".to_vec(), Object::String(PdfString::text(&w.name)));
        }
        let r = self.doc.add(Object::Dict(d));
        if top_level {
            self.fields.push(r);
            self.field_count += 1;
        } else if let WidgetKind::Radio { group, .. } = &w.kind
            && let Some((_, kids)) = self.radio_groups.get_mut(group)
        {
            kids.push(r);
        }
        Ok(Some(r))
    }

    /// An image XObject for a picture: JPEGs as they are (`DCTDecode`), PNGs and GIFs decoded
    /// to samples (`FlateDecode`, with a soft mask for transparency). `None`, with a warning,
    /// for anything else.
    fn image_xobject(&mut self, data: &[u8], content_type: &str) -> Option<ObjRef> {
        let hash = content_hash(data);
        if let Some(&r) = self.images.get(&hash)
            && let Object::Stream(s) = &*self.doc.get(r)
            && s.dict.name(b"Subtype") == Some(b"Image")
        {
            return Some(r);
        }
        let r = self.new_image_xobject(data, content_type, &hash)?;
        if self.images.len() < MAX_REMEMBERED_IMAGES {
            self.images.insert(hash, r);
        }
        Some(r)
    }

    fn new_image_xobject(&mut self, data: &[u8], content_type: &str, hash: &[u8; 32]) -> Option<ObjRef> {
        let what = if content_type.is_empty() { "picture" } else { content_type };
        let mut d = Dict::new();
        d.set(IMAGE_SHA_KEY.to_vec(), Object::String(PdfString::literal(hex32(hash))));
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        match crate::image::kind(data) {
            Some(crate::image::Kind::Jpeg) => {
                let Some((iw, ih, comps, adobe)) = jpeg_info(data) else {
                    self.warnings.push(format!("a {what} image is not a readable JPEG and is left out"));
                    return None;
                };
                d.set(b"Width".to_vec(), Object::Int(i64::from(iw)));
                d.set(b"Height".to_vec(), Object::Int(i64::from(ih)));
                d.set(
                    b"ColorSpace".to_vec(),
                    Object::name(match comps {
                        1 => "DeviceGray",
                        4 => "DeviceCMYK",
                        _ => "DeviceRGB",
                    }),
                );
                if comps == 4 && adobe {
                    // Adobe APP14 JPEGs store inverted CMYK.
                    d.set(b"Decode".to_vec(), Object::Array([1, 0, 1, 0, 1, 0, 1, 0].iter().map(|v| Object::Int(*v)).collect()));
                }
                d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
                Some(self.doc.add(Object::Stream(Stream::from_raw(d, data.to_vec()))))
            }
            Some(crate::image::Kind::Png | crate::image::Kind::Gif) => {
                let img = match crate::image::decode(data) {
                    Ok(img) => img,
                    Err(why) => {
                        self.warnings.push(why);
                        return None;
                    }
                };
                d.set(b"Width".to_vec(), Object::Int(i64::from(img.width)));
                d.set(b"Height".to_vec(), Object::Int(i64::from(img.height)));
                d.set(b"ColorSpace".to_vec(), Object::name(img.color_space));
                if let Some(alpha) = &img.alpha {
                    let mut m = Dict::new();
                    m.set(b"Type".to_vec(), Object::name("XObject"));
                    m.set(b"Subtype".to_vec(), Object::name("Image"));
                    m.set(b"Width".to_vec(), Object::Int(i64::from(img.width)));
                    m.set(b"Height".to_vec(), Object::Int(i64::from(img.height)));
                    m.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
                    m.set(b"BitsPerComponent".to_vec(), Object::Int(8));
                    let mr = self.doc.add(Object::Stream(Stream::flate(m, alpha)));
                    d.set(b"SMask".to_vec(), Object::Ref(mr));
                }
                Some(self.doc.add(Object::Stream(Stream::flate(d, &img.samples))))
            }
            None => {
                self.warnings.push(format!("a {what} image is not a JPEG, PNG or GIF and is left out"));
                None
            }
        }
    }

    fn page(&mut self, page: &Page, pages_ref: ObjRef) -> Result<ObjRef, XfaError> {
        let page_ref = self.doc.add(Object::Dict(Dict::new()));
        let mut content: Vec<u8> = Vec::new();
        let mut fonts: BTreeMap<&'static str, &'static str> = BTreeMap::new();
        let mut xobjects = Dict::new();
        let mut annots = Vec::new();
        let h = page.height;
        for (i, item) in page.items.iter().enumerate() {
            match item {
                Item::Fill { rect, color } => {
                    content.extend(format!("{} rg {} {} {} {} re f\n", rgb(*color), n(rect.x), n(h - rect.bottom()), n(rect.w), n(rect.h)).bytes());
                }
                Item::Line { from, to, width, color, dashed } => {
                    content.extend(
                        format!(
                            "q {} RG {} w {}{} {} m {} {} l S Q\n",
                            rgb(*color),
                            n(*width),
                            if *dashed { "[2 2] 0 d " } else { "" },
                            n(from.0),
                            n(h - from.1),
                            n(to.0),
                            n(h - to.1)
                        )
                        .bytes(),
                    );
                }
                Item::Text(s) => {
                    let name = s.face.resource_name();
                    fonts.insert(name, s.face.family.base_font(s.face.bold, s.face.italic));
                    content.extend(
                        format!("BT /{name} {} Tf {} rg 1 0 0 1 {} {} Tm ", n(s.face.size), rgb(s.face.color), n(s.x), n(h - s.baseline)).bytes(),
                    );
                    content.extend(literal(&win_ansi(&s.text)));
                    content.extend_from_slice(b" Tj ET\n");
                }
                Item::Image { rect, data, content_type } => {
                    let Some(r) = self.image_xobject(data, content_type) else { continue };
                    let name = format!("Im{i}");
                    xobjects.set(name.clone().into_bytes(), Object::Ref(r));
                    content.extend(format!("q {} 0 0 {} {} {} cm /{name} Do Q\n", n(rect.w), n(rect.h), n(rect.x), n(h - rect.bottom())).bytes());
                }
                Item::Widget(w) => {
                    if let Some(r) = self.widget(w, page, page_ref)? {
                        annots.push(Object::Ref(r));
                    }
                }
            }
        }
        let mut font_dict = Dict::new();
        for (name, base) in fonts {
            font_dict.set(name.as_bytes().to_vec(), standard_font(base));
        }
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Dict(font_dict));
        if !xobjects.is_empty() {
            res.set(b"XObject".to_vec(), Object::Dict(xobjects));
        }
        let mut cd = Dict::new();
        cd.set(b"Type".to_vec(), Object::name("XObject"));
        let contents = self.doc.add(Object::Stream(Stream::flate(Dict::new(), &content)));
        let _ = cd;
        let mut pd = Dict::new();
        pd.set(b"Type".to_vec(), Object::name("Page"));
        pd.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        pd.set(b"MediaBox".to_vec(), Object::Array([0.0, 0.0, page.width, page.height].iter().map(|v| Object::Real(*v)).collect()));
        pd.set(b"Resources".to_vec(), Object::Dict(res));
        pd.set(b"Contents".to_vec(), Object::Ref(contents));
        if !annots.is_empty() {
            pd.set(b"Annots".to_vec(), Object::Array(annots));
        }
        self.doc.set(page_ref, Object::Dict(pd));
        Ok(page_ref)
    }
}

/// Replace the document's pages with the laid-out form and add its fields to the AcroForm. The
/// XFA packets and everything else in the catalog stay.
/// The image XObjects earlier passes wrote for pictures, by the SHA-256 each records: those on
/// the pages under `pages` (the flat page list [`write_form`] writes).
fn earlier_images(doc: &Document, pages: ObjRef) -> HashMap<[u8; 32], ObjRef> {
    let mut out = HashMap::new();
    let kids = doc.get(pages).as_dict().and_then(|d| d.get(b"Kids").map(|k| doc.resolve(k))).and_then(|k| k.as_array().cloned()).unwrap_or_default();
    for page in kids.iter().filter_map(Object::as_ref).take(MAX_PAGES_SEARCHED) {
        let page = doc.get(page);
        let Some(resources) = page.as_dict().and_then(|p| p.get(b"Resources")).map(|r| doc.resolve(r)) else { continue };
        let Some(xobjects) = resources.as_dict().and_then(|r| r.get(b"XObject")).map(|x| doc.resolve(x)) else { continue };
        for r in xobjects.as_dict().into_iter().flat_map(|x| x.iter()).filter_map(|(_, v)| v.as_ref()) {
            if out.len() >= MAX_REMEMBERED_IMAGES {
                return out;
            }
            if let Object::Stream(s) = &*doc.get(r)
                && s.dict.name(b"Subtype") == Some(b"Image")
                && let Some(hash) = s.dict.get(IMAGE_SHA_KEY).and_then(Object::as_string).and_then(|h| parse_hex32(&h.bytes))
            {
                out.insert(hash, r);
            }
        }
    }
    out
}

pub fn write_form(doc: &mut Document, form: &Form) -> Result<Written, XfaError> {
    let catalog_ref = doc.root().ok_or_else(|| XfaError::Malformed("the document has no catalog".into()))?;
    let mut catalog = doc.get(catalog_ref).as_dict().cloned().ok_or_else(|| XfaError::Malformed("the catalog is not a dictionary".into()))?;
    let mut catalog_changed = false;
    let pages_ref = match catalog.get(b"Pages").and_then(Object::as_ref) {
        Some(r) if doc.get(r).as_dict().is_some() => r,
        _ => {
            let mut p = Dict::new();
            p.set(b"Type".to_vec(), Object::name("Pages"));
            let r = doc.add(Object::Dict(p));
            catalog.set(b"Pages".to_vec(), Object::Ref(r));
            catalog_changed = true;
            r
        }
    };
    // Pictures embedded by earlier passes, from the pages this pass replaces.
    let images = earlier_images(doc, pages_ref);
    let mut em = Emitter {
        doc,
        images,
        fields: Vec::new(),
        radio_groups: HashMap::new(),
        radio_selected: HashMap::new(),
        field_count: 0,
        warnings: Vec::new(),
    };
    let mut kids = Vec::new();
    for page in &form.pages {
        kids.push(Object::Ref(em.page(page, pages_ref)?));
    }
    let groups: Vec<(String, (ObjRef, Vec<ObjRef>))> = em.radio_groups.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (name, (parent, widgets)) in groups {
        let selected = em.radio_selected.get(&name).cloned();
        em.doc.update_dict(parent, |d| {
            d.set(b"Kids".to_vec(), Object::Array(widgets.iter().map(|r| Object::Ref(*r)).collect()));
            if let Some(s) = &selected {
                d.set(b"V".to_vec(), Object::name(s));
            }
        })?;
    }
    let Emitter { doc, fields, field_count, warnings, .. } = em;
    let count = kids.len();
    doc.update_dict(pages_ref, |d| {
        d.set(b"Type".to_vec(), Object::name("Pages"));
        d.set(b"Kids".to_vec(), Object::Array(kids));
        d.set(b"Count".to_vec(), Object::Int(count as i64));
        d.remove(b"Parent");
    })?;
    // AcroForm: keep what is there (the XFA packets above all), add our fields and fonts.
    let acro_ref = catalog.get(b"AcroForm").and_then(Object::as_ref);
    let mut acro = match catalog.get(b"AcroForm") {
        Some(o) => doc.resolve(o).as_dict().cloned().unwrap_or_default(),
        None => Dict::new(),
    };
    let mut all: Vec<Object> = acro.get(b"Fields").map(|f| doc.resolve(f)).and_then(|f| f.as_array().cloned()).unwrap_or_default();
    all.extend(fields.iter().map(|r| Object::Ref(*r)));
    acro.set(b"Fields".to_vec(), Object::Array(all));
    let mut dr = acro.get(b"DR").map(|d| doc.resolve(d)).and_then(|d| d.as_dict().cloned()).unwrap_or_default();
    let mut dr_fonts = dr.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    for (name, base) in all_faces() {
        if !dr_fonts.contains(name.as_bytes()) {
            dr_fonts.set(name.as_bytes().to_vec(), standard_font(base));
        }
    }
    if !dr_fonts.contains(b"ZaDb") {
        let mut z = Dict::new();
        z.set(b"Type".to_vec(), Object::name("Font"));
        z.set(b"Subtype".to_vec(), Object::name("Type1"));
        z.set(b"BaseFont".to_vec(), Object::name("ZapfDingbats"));
        dr_fonts.set(b"ZaDb".to_vec(), Object::Dict(z));
    }
    dr.set(b"Font".to_vec(), Object::Dict(dr_fonts));
    acro.set(b"DR".to_vec(), Object::Dict(dr));
    if !acro.contains(b"DA") {
        acro.set(b"DA".to_vec(), Object::String(PdfString::literal(b"/Helv 0 Tf 0 g".to_vec())));
    }
    let mut mark = Dict::new();
    mark.set(b"Pages".to_vec(), Object::Int(count as i64));
    mark.set(b"Fields".to_vec(), Object::Int(field_count as i64));
    mark.set(b"Warnings".to_vec(), Object::Array(warnings.iter().map(|w| Object::String(PdfString::text(w))).collect()));
    acro.set(LAYOUT_KEY.to_vec(), Object::Dict(mark));
    match acro_ref {
        Some(r) => doc.set(r, Object::Dict(acro)),
        None => {
            catalog.set(b"AcroForm".to_vec(), Object::Dict(acro));
            catalog_changed = true;
        }
    }
    if catalog_changed {
        doc.set(catalog_ref, Object::Dict(catalog));
    }
    Ok(Written { pages: count, fields: field_count, warnings })
}

/// The layout a document already carries from an earlier pass, if any.
pub fn existing_layout(doc: &Document) -> Option<crate::Report> {
    let root = doc.root()?;
    let catalog = doc.get(root);
    let acro = doc.resolve(catalog.as_dict()?.get(b"AcroForm")?);
    let mark = doc.resolve(acro.as_dict()?.get(LAYOUT_KEY)?);
    let mark = mark.as_dict()?;
    let warnings = mark
        .get(b"Warnings")
        .map(|w| doc.resolve(w))
        .and_then(|w| w.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|w| w.as_string().map(|s| s.to_text()))
        .take(100)
        .collect();
    Some(crate::Report {
        pages: mark.int(b"Pages").unwrap_or(0).clamp(0, 100_000) as usize,
        fields: mark.int(b"Fields").unwrap_or(0).clamp(0, 1_000_000) as usize,
        template_bytes: 0,
        warnings,
    })
}

/// The rectangle of a widget in PDF user space (for tests).
pub fn pdf_rect(r: Rect, page_height: f64) -> [f64; 4] {
    [r.x, page_height - r.bottom(), r.right(), page_height - r.y]
}
