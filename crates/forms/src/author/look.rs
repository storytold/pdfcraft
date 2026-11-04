use super::{Dict, Document, Field, FieldKind, FormError, ObjRef, Object, Stream, Widget, appearance, fields, invalid, rgb};

/// The Appearance tab: borders, colours and text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub border: Option<[f64; 3]>,
    pub fill: Option<[f64; 3]>,
    /// Line thickness in points (Acrobat: thin 1, medium 2, thick 3).
    pub width: f64,
    pub style: BorderStyle,
    pub text: [f64; 3],
    pub font: FieldFont,
}

/// A partial Appearance-tab edit. Outer `None` keeps a property; `Some(None)` removes a
/// border or fill colour. The patch is applied to each widget's own dictionaries.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LookPatch {
    pub border: Option<Option<[f64; 3]>>,
    pub fill: Option<Option<[f64; 3]>>,
    pub width: Option<f64>,
    pub style: Option<BorderStyle>,
    pub text: Option<[f64; 3]>,
    pub font: Option<FieldFont>,
}

impl LookPatch {
    pub(super) fn validate(&self) -> Result<(), FormError> {
        for c in self.border.flatten().into_iter().chain(self.fill.flatten()).chain(self.text) {
            if !c.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
                return invalid("appearance colours must be finite RGB values between 0 and 1");
            }
        }
        if self.width.is_some_and(|v| !v.is_finite() || !(0.0..=12.0).contains(&v)) {
            return invalid("border width must be between 0 and 12 points");
        }
        Ok(())
    }
}

/// Rewrite only the selected operators in place: the last `Tf` (font and size) and the last
/// fill colour (`g`, `rg` or `k`), appending one when the string has none. Everything else (a
/// custom font resource, colour space or other graphics state) is kept, and repeated edits don't
/// grow the string.
pub(super) fn patch_da(da: &str, size: Option<f64>, patch: &LookPatch) -> String {
    if patch.font.is_none() && size.is_none() && patch.text.is_none() {
        return da.to_string();
    }
    let mut toks: Vec<String> = da.split_whitespace().map(str::to_string).collect();
    let numeric = |s: &str| s.parse::<f64>().is_ok();
    if patch.font.is_some() || size.is_some() {
        let old = appearance::parse_da(da);
        let font = patch.font.map_or_else(|| old.font.clone(), |f| f.resource().to_string());
        // `parse_da` reads sizes up to 300 points; keep the same bound when writing.
        let size = appearance::fmt(size.unwrap_or(old.size).clamp(0.0, 300.0));
        let at = toks.iter().enumerate().rev().find(|(i, t)| t.as_str() == "Tf" && *i >= 2).map(|(i, _)| i);
        match at {
            Some(i) => {
                if let Some(slot) = toks.get_mut(i - 2) {
                    *slot = format!("/{font}");
                }
                if let Some(slot) = toks.get_mut(i - 1) {
                    *slot = size;
                }
            }
            None => toks.extend([format!("/{font}"), size, "Tf".into()]),
        }
    }
    if let Some(c) = patch.text {
        let rgb = [appearance::fmt(c[0]), appearance::fmt(c[1]), appearance::fmt(c[2]), "rg".into()];
        let at = toks.iter().enumerate().rev().find_map(|(i, t)| {
            let n = match t.as_str() {
                "g" => 1,
                "rg" => 3,
                "k" => 4,
                _ => return None,
            };
            let start = i.checked_sub(n)?;
            toks.get(start..i).is_some_and(|ops| ops.iter().all(|o| numeric(o))).then_some((start, i))
        });
        match at {
            Some((start, end)) => {
                toks.splice(start..=end, rgb);
            }
            None => toks.extend(rgb),
        }
    }
    toks.join(" ")
}

pub(super) fn patch_widget_look(doc: &mut Document, w: &Widget, patch: &LookPatch) -> Result<(), FormError> {
    let touches_mk = patch.border.is_some() || patch.fill.is_some();
    let touches_bs = patch.width.is_some() || patch.style.is_some();
    // A size- or font-only edit changes /DA, not the widget: leave it out of the save.
    if !touches_mk && !touches_bs {
        return Ok(());
    }
    let wd = doc.get(w.obj).as_dict().cloned().ok_or_else(|| FormError::Invalid("the widget is not a dictionary".into()))?;
    if touches_mk {
        let indirect = wd.get(b"MK").and_then(Object::as_ref);
        let mut mk = wd.get(b"MK").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned()).unwrap_or_default();
        for (key, change) in [(b"BC".as_slice(), patch.border), (b"BG".as_slice(), patch.fill)] {
            if let Some(change) = change {
                match change {
                    Some(c) => mk.set(key.to_vec(), rgb(c)),
                    None => {
                        mk.remove(key);
                    }
                }
            }
        }
        // A shared indirect /MK is updated where it lives, not copied into each widget.
        match indirect {
            Some(r) => doc.set(r, Object::Dict(mk)),
            None => doc.update_dict(w.obj, |d| d.set(b"MK".to_vec(), Object::Dict(mk)))?,
        }
    }
    if touches_bs {
        let indirect = wd.get(b"BS").and_then(Object::as_ref);
        let mut bs = wd.get(b"BS").map(|b| doc.resolve(b)).and_then(|b| b.as_dict().cloned()).unwrap_or_default();
        if let Some(width) = patch.width {
            bs.set(b"W".to_vec(), Object::Real(width));
        }
        if let Some(style) = patch.style {
            bs.set(b"S".to_vec(), Object::name(style.code()));
            if style == BorderStyle::Dashed && !bs.contains(b"D") {
                bs.set(b"D".to_vec(), Object::Array(vec![Object::Int(3)]));
            }
        }
        match indirect {
            Some(r) => doc.set(r, Object::Dict(bs)),
            None => doc.update_dict(w.obj, |d| d.set(b"BS".to_vec(), Object::Dict(bs)))?,
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderStyle {
    Solid,
    Dashed,
    Beveled,
    Inset,
    Underline,
}

impl BorderStyle {
    pub const ALL: [BorderStyle; 5] = [BorderStyle::Solid, BorderStyle::Dashed, BorderStyle::Beveled, BorderStyle::Inset, BorderStyle::Underline];

    pub(super) fn code(self) -> &'static str {
        match self {
            BorderStyle::Solid => "S",
            BorderStyle::Dashed => "D",
            BorderStyle::Beveled => "B",
            BorderStyle::Inset => "I",
            BorderStyle::Underline => "U",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BorderStyle::Solid => "Solid",
            BorderStyle::Dashed => "Dashed",
            BorderStyle::Beveled => "Beveled",
            BorderStyle::Inset => "Inset",
            BorderStyle::Underline => "Underline",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldFont {
    Helvetica,
    Times,
    Courier,
}

impl FieldFont {
    pub const ALL: [FieldFont; 3] = [FieldFont::Helvetica, FieldFont::Times, FieldFont::Courier];

    fn resource(self) -> &'static str {
        match self {
            FieldFont::Helvetica => "Helv",
            FieldFont::Times => "TiRo",
            FieldFont::Courier => "Cour",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FieldFont::Helvetica => "Helvetica",
            FieldFont::Times => "Times Roman",
            FieldFont::Courier => "Courier",
        }
    }
}

/// The mark a check box or radio button shows when on (Field Properties ▸ Options ▸ style).
/// Stored as `/MK /CA`, the ZapfDingbats character Acrobat uses for it; PdfCraft draws the
/// mark as paths, so no symbol font is needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckStyle {
    Check,
    Circle,
    Cross,
    Diamond,
    Square,
    Star,
}

impl CheckStyle {
    pub const ALL: [CheckStyle; 6] =
        [CheckStyle::Check, CheckStyle::Circle, CheckStyle::Cross, CheckStyle::Diamond, CheckStyle::Square, CheckStyle::Star];

    pub(super) fn code(self) -> &'static str {
        match self {
            CheckStyle::Check => "4",
            CheckStyle::Circle => "l",
            CheckStyle::Cross => "8",
            CheckStyle::Diamond => "u",
            CheckStyle::Square => "n",
            CheckStyle::Star => "H",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CheckStyle::Check => "Check",
            CheckStyle::Circle => "Circle",
            CheckStyle::Cross => "Cross",
            CheckStyle::Diamond => "Diamond",
            CheckStyle::Square => "Square",
            CheckStyle::Star => "Star",
        }
    }

    /// A widget's style: its `/MK /CA`, else a check for check boxes and a circle for radios.
    pub(crate) fn of_widget(doc: &Document, wd: &Dict, kind: FieldKind) -> Self {
        let mk = wd.get(b"MK").and_then(|m| doc.resolve(m).as_dict().cloned()).unwrap_or_default();
        let ca = mk.get(b"CA").and_then(|c| doc.resolve(c).as_string().map(|s| s.to_text()));
        CheckStyle::ALL.into_iter().find(|s| ca.as_deref() == Some(s.code())).unwrap_or(if kind == FieldKind::Radio {
            CheckStyle::Circle
        } else {
            CheckStyle::Check
        })
    }
}

/// A check box's or radio button's style (from its first widget).
pub fn check_style(doc: &Document, f: &Field) -> CheckStyle {
    let wd = f.widgets.first().and_then(|w| doc.get(w.obj).as_dict().cloned()).unwrap_or_default();
    CheckStyle::of_widget(doc, &wd, f.kind)
}

fn rgb_of(doc: &Document, o: Option<&Object>) -> Option<[f64; 3]> {
    let v: Vec<f64> = doc.resolve(o?).as_array()?.iter().filter_map(Object::as_f64).collect();
    match v.len() {
        1 => Some([v[0]; 3]),
        3 => Some([v[0], v[1], v[2]]),
        4 => Some([(1.0 - v[0]) * (1.0 - v[3]), (1.0 - v[1]) * (1.0 - v[3]), (1.0 - v[2]) * (1.0 - v[3])]),
        _ => None,
    }
}

/// A field's current look (from its first widget and its default appearance).
pub fn look(doc: &Document, f: &Field) -> Look {
    let wd = f.widgets.first().and_then(|w| doc.get(w.obj).as_dict().cloned()).unwrap_or_default();
    let mk = wd.get(b"MK").and_then(|m| doc.resolve(m).as_dict().cloned()).unwrap_or_default();
    let bs = wd.get(b"BS").and_then(|b| doc.resolve(b).as_dict().cloned()).unwrap_or_default();
    let da = wd.get(b"DA").and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text())).unwrap_or_else(|| f.da.clone());
    let parsed = appearance::parse_da(&da);
    let nums: Vec<f64> = parsed.color.split_whitespace().filter_map(|t| t.parse().ok()).collect();
    let text = match nums.len() {
        1 => [nums[0]; 3],
        3 => [nums[0], nums[1], nums[2]],
        4 => [(1.0 - nums[0]) * (1.0 - nums[3]), (1.0 - nums[1]) * (1.0 - nums[3]), (1.0 - nums[2]) * (1.0 - nums[3])],
        _ => [0.0; 3],
    };
    Look {
        border: rgb_of(doc, mk.get(b"BC")),
        fill: rgb_of(doc, mk.get(b"BG")),
        width: bs.get(b"W").and_then(|w| doc.resolve(w).as_f64()).unwrap_or(1.0),
        style: match bs.name(b"S") {
            Some(b"D") => BorderStyle::Dashed,
            Some(b"B") => BorderStyle::Beveled,
            Some(b"I") => BorderStyle::Inset,
            Some(b"U") => BorderStyle::Underline,
            _ => BorderStyle::Solid,
        },
        text,
        font: match parsed.font.as_str() {
            "TiRo" | "Times-Roman" => FieldFont::Times,
            "Cour" | "Courier" => FieldFont::Courier,
            _ => FieldFont::Helvetica,
        },
    }
}

/// A `/DA` string with the given font, size and colour.
pub(super) fn da_string(font: FieldFont, size: f64, c: [f64; 3]) -> String {
    let f = crate::appearance::fmt;
    format!("/{} {} Tf {} {} {} rg", font.resource(), f(size.clamp(0.0, 100.0)), f(c[0]), f(c[1]), f(c[2]))
}

/// Regenerate every widget appearance of a field from its current value and settings. A value or
/// button caption its font can't hold is drawn with `?` for those characters, the widget is marked
/// (`pdfcraft_fonts::UNDRAWN_MARK`) and other viewers are asked to redraw the form (see
/// `ask_viewers_to_redraw`), as when a value is set.
pub fn redraw_field(doc: &mut Document, name: &str) -> Result<(), FormError> {
    redraw_widgets(doc, name, None)
}

/// `redraw_field` for the widget at index `only` alone (so a caption-only edit leaves the field's
/// other widgets' appearances as they are), or for every widget when `only` is `None`. Fails with
/// `FormError::NoSuchField` when no field is named `name`.
pub(super) fn redraw_widgets(doc: &mut Document, name: &str, only: Option<usize>) -> Result<(), FormError> {
    let f = fields(doc).into_iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?;
    let mut undrawn = false;
    for (index, w) in f.widgets.iter().enumerate() {
        if only.is_some_and(|only| only != index) {
            continue;
        }
        let mut missing = None;
        let ap = match f.kind {
            FieldKind::Text | FieldKind::Combo | FieldKind::List => {
                let (s, m) = appearance::field_appearance_drawn(doc, &f, w, &f.value, true);
                missing = m;
                let r = doc.add(Object::Stream(s));
                let mut d = Dict::new();
                d.set(b"N".to_vec(), Object::Ref(r));
                d
            }
            FieldKind::CheckBox | FieldKind::Radio => {
                let on = w.on_state.clone().unwrap_or_else(|| "Yes".into());
                appearance::check_box_states(doc, w, f.kind, &on, &f.da)
            }
            FieldKind::PushButton => {
                let (s, m) = button_appearance(doc, w, &f.da);
                missing = m;
                let r = doc.add(Object::Stream(s));
                let mut d = Dict::new();
                d.set(b"N".to_vec(), Object::Ref(r));
                d
            }
            FieldKind::Signature => {
                let s = empty_box(doc, w);
                let r = doc.add(Object::Stream(s));
                let mut d = Dict::new();
                d.set(b"N".to_vec(), Object::Ref(r));
                d
            }
        };
        undrawn |= missing.is_some();
        let existing = crate::merged_appearance(doc, w.obj, &ap);
        doc.update_dict(w.obj, |d| {
            d.set(b"AP".to_vec(), Object::Dict(existing));
            pdfcraft_fonts::set_undrawn_mark(d, missing);
        })?;
    }
    if undrawn {
        crate::ask_viewers_to_redraw(doc)?;
    }
    Ok(())
}

fn frame_only(doc: &Document, w: &Widget) -> (String, crate::appearance::Placement) {
    let (page_w, page_h) = ((w.rect[2] - w.rect[0]).max(1.0), (w.rect[3] - w.rect[1]).max(1.0));
    let wobj = doc.get(w.obj);
    let wd = wobj.as_dict().cloned().unwrap_or_default();
    let place = crate::appearance::placement(crate::appearance::mk_rotation(doc, &wd), page_w, page_h);
    let (width, height) = (place.layout_w, place.layout_h);
    let mk = wd.get(b"MK").and_then(|m| m.as_dict()).cloned().unwrap_or_default();
    let col = |k: &[u8]| -> Option<String> {
        let v: Vec<f64> = mk.get(k)?.as_array()?.iter().filter_map(|x| x.as_f64()).collect();
        (v.len() == 3).then(|| format!("{} {} {}", crate::appearance::fmt(v[0]), crate::appearance::fmt(v[1]), crate::appearance::fmt(v[2])))
    };
    let mut c = String::new();
    if let Some(bg) = col(b"BG") {
        c.push_str(&format!("{bg} rg 0 0 {width:.3} {height:.3} re f\n"));
    }
    if let Some(bc) = col(b"BC") {
        c.push_str(&format!("{bc} RG 1 w 0.5 0.5 {:.3} {:.3} re S\n", width - 1.0, height - 1.0));
    }
    (c, place)
}

fn form_stream(place: crate::appearance::Placement, content: Vec<u8>, resources: Dict) -> Stream {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    crate::appearance::set_form_box(&mut d, place);
    d.set(b"Resources".to_vec(), Object::Dict(resources));
    Stream::flate(d, &content)
}

/// A push button: background, border, its icon (`/MK /I`, scaled to fit and centred) and its
/// centred caption (unless the layout is icon only, `/TP 1`). The caption uses the font, size and
/// color its `/DA` asks for (the widget's, else `field_da`, the field's own or inherited one).
/// Also returns the first caption character the font can't draw (drawn as `?`), for the caller to
/// mark and report.
pub(crate) fn button_appearance(doc: &Document, w: &Widget, field_da: &str) -> (Stream, Option<char>) {
    let (mut c, place) = frame_only(doc, w);
    let (width, height) = (place.layout_w, place.layout_h);
    let wobj = doc.get(w.obj);
    let mk = wobj.as_dict().and_then(|d| d.get(b"MK")).map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned()).unwrap_or_default();
    let caption = mk.get(b"CA").map(|c| doc.resolve(c)).and_then(|c| c.as_string().map(|s| s.to_text())).unwrap_or_default();
    let icon_only = mk.get(b"TP").and_then(|t| doc.resolve(t).as_int()) == Some(1);
    let mut content = std::mem::take(&mut c).into_bytes();
    let mut xobjects = Dict::new();
    if let Some(icon) = mk.get(b"I").and_then(Object::as_ref)
        && let Object::Stream(s) = &*doc.get(icon)
    {
        let bb: Vec<f64> = s.dict.get(b"BBox").and_then(|b| b.as_array().map(|a| a.iter().filter_map(Object::as_f64).collect())).unwrap_or_default();
        if let [x0, y0, x1, y1] = bb[..]
            && x1 > x0
            && y1 > y0
        {
            // Proportional, as large as fits (Acrobat's default icon placement), centred.
            let k = ((width - 2.0) / (x1 - x0)).min((height - 2.0) / (y1 - y0)).max(0.0);
            let (dx, dy) = ((width - (x1 - x0) * k) / 2.0 - x0 * k, (height - (y1 - y0) * k) / 2.0 - y0 * k);
            content.extend(format!("q {k:.6} 0 0 {k:.6} {dx:.3} {dy:.3} cm /Icon Do Q\n").bytes());
            xobjects.set(b"Icon".to_vec(), Object::Ref(icon));
        }
    }
    let mut missing = None;
    let mut fonts = Dict::new();
    if !icon_only && !caption.is_empty() {
        let da = wobj.as_dict().and_then(|d| d.get(b"DA")).and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text()));
        let auto = ((height - 4.0) * 0.6).clamp(4.0, 14.0);
        let (ops, name, font, m) = appearance::button_caption(doc, da.as_deref().unwrap_or(field_da), &caption, width, height, auto);
        content.extend(ops);
        fonts.set(name.into_bytes(), font);
        missing = m;
    }
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    if !xobjects.is_empty() {
        res.set(b"XObject".to_vec(), Object::Dict(xobjects));
    }
    (form_stream(place, content, res), missing)
}

/// Give a push button (an image field) the picture `image` (an image XObject of `px` pixels):
/// it becomes the button's icon (`/MK /I`) and its appearance.
pub fn set_button_icon(doc: &mut Document, name: &str, image: ObjRef, px: (u32, u32)) -> Result<(), FormError> {
    let f = fields(doc).into_iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?;
    if f.kind != FieldKind::PushButton {
        return invalid(format!("{name} is not a button or image field"));
    }
    // The icon form draws the image in a box of its pixel size (keeping its aspect ratio).
    let (w, h) = (px.0.max(1) as f64, px.1.max(1) as f64);
    let mut xo = Dict::new();
    xo.set(b"Im0".to_vec(), Object::Ref(image));
    let mut res = Dict::new();
    res.set(b"XObject".to_vec(), Object::Dict(xo));
    let icon =
        doc.add(Object::Stream(form_stream(crate::appearance::placement(0, w, h), format!("q {w} 0 0 {h} 0 0 cm /Im0 Do Q").into_bytes(), res)));
    for wd in &f.widgets {
        let mut mk =
            doc.get(wd.obj).as_dict().and_then(|d| d.get(b"MK").map(|m| doc.resolve(m))).and_then(|m| m.as_dict().cloned()).unwrap_or_default();
        mk.set(b"I".to_vec(), Object::Ref(icon));
        if !mk.contains(b"TP") {
            mk.set(b"TP".to_vec(), Object::Int(1));
        }
        doc.update_dict(wd.obj, |d| d.set(b"MK".to_vec(), Object::Dict(mk)))?;
    }
    redraw_field(doc, name)
}

fn empty_box(doc: &Document, w: &Widget) -> Stream {
    let (c, place) = frame_only(doc, w);
    form_stream(place, c.into_bytes(), Dict::new())
}
