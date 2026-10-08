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
    if data.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2usize;
    let mut guard = 0;
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
        let sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if sof {
            let h = u16::from_be_bytes([*data.get(i + 5)?, *data.get(i + 6)?]) as u32;
            let w = u16::from_be_bytes([*data.get(i + 7)?, *data.get(i + 8)?]) as u32;
            let comps = *data.get(i + 9)?;
            return (w > 0 && h > 0).then_some((w, h, comps));
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

struct Emitter<'a> {
    doc: &'a mut Document,
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
            WidgetKind::Text | WidgetKind::Date(_) => {
                d.set(b"FT".to_vec(), Object::name("Tx"));
                if w.multiline {
                    ff |= 1 << 12;
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
                    let Some((iw, ih, comps)) = jpeg_frame(data) else {
                        self.warnings.push(format!(
                            "a {} image is not a JPEG and is left out",
                            if content_type.is_empty() { "picture" } else { content_type.as_str() }
                        ));
                        continue;
                    };
                    let mut d = Dict::new();
                    d.set(b"Type".to_vec(), Object::name("XObject"));
                    d.set(b"Subtype".to_vec(), Object::name("Image"));
                    d.set(b"Width".to_vec(), Object::Int(i64::from(iw)));
                    d.set(b"Height".to_vec(), Object::Int(i64::from(ih)));
                    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
                    d.set(
                        b"ColorSpace".to_vec(),
                        Object::name(match comps {
                            1 => "DeviceGray",
                            4 => "DeviceCMYK",
                            _ => "DeviceRGB",
                        }),
                    );
                    if comps == 4 {
                        // Adobe APP14 JPEGs store inverted CMYK.
                        d.set(b"Decode".to_vec(), Object::Array([1, 0, 1, 0, 1, 0, 1, 0].iter().map(|v| Object::Int(*v)).collect()));
                    }
                    d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
                    let r = self.doc.add(Object::Stream(Stream::from_raw(d, data.as_ref().clone())));
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
    let mut em =
        Emitter { doc, fields: Vec::new(), radio_groups: HashMap::new(), radio_selected: HashMap::new(), field_count: 0, warnings: Vec::new() };
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
