use super::draw::{draw, params};
use super::layout::{first_undrawable, is_thai, nums, shaped_arabic, thai_error};
use super::types::{Added, AddedImage, AddedText, CHAR_SPACING_LIMIT, Family, Ink, MarkerScan, TAG};
use super::{Content, Dict, Document, EditError, GlyphError, MarkKind, ObjRef, Object, Stream, align_from, check, contents, page_list, place_tagged};

/// A text item's gray or CMYK ink. Items written before inks, and an ink out of range or not
/// made only of numbers (a `/Gray` name, a `/CMYK` entry that isn't a number), have none: the
/// text paints in its RGB `/Color`.
fn ink(doc: &Document, d: &Dict) -> Option<Ink> {
    let ink = if let Some(g) = d.get(b"Gray") {
        Ink::Gray(read_value(doc, g, Object::as_f64)?)
    } else {
        // Four entries or none: a longer array is never copied or read through.
        read_value(doc, d.get(b"CMYK")?, |cmyk| {
            let [c, m, y, k] = cmyk.as_array()?.as_slice() else { return None };
            Some(Ink::Cmyk([c.as_f64()?, m.as_f64()?, y.as_f64()?, k.as_f64()?]))
        })?
    };
    Some(ink).filter(|i| i.valid())
}

/// `f` of `o`, a reference followed. A direct value is read where it is, never copied, so a
/// large one a file puts there costs nothing to look at.
fn read_value<T>(doc: &Document, o: &Object, f: impl FnOnce(&Object) -> T) -> T {
    match o {
        Object::Ref(_) => f(&doc.resolve(o)),
        direct => f(direct),
    }
}

fn parse(doc: &Document, d: &Dict) -> Option<Content> {
    let r = nums(doc, d.get(b"Rect"));
    let rect: [f64; 4] = r.try_into().ok()?;
    match d.name(b"Kind")? {
        b"Text" => {
            let (family, bold, italic) = Family::from_base(d.name(b"Font").unwrap_or(b"Helvetica"));
            let c = nums(doc, d.get(b"Color"));
            let text = d.get(b"Text").and_then(|t| doc.resolve(t).as_string().map(|s| s.to_text())).unwrap_or_default();
            let align = align_from(d.int(b"Align"));
            // At most one entry per paragraph, whatever the file says.
            let aligns = d
                .get(b"Aligns")
                .and_then(|a| {
                    doc.resolve(a)
                        .as_array()
                        .map(|a| a.iter().take(crate::paragraph_align::count(&text)).map(|v| v.as_int().map(|c| align_from(Some(c)))).collect())
                })
                .unwrap_or_default();
            let mut t = AddedText {
                rect,
                text,
                family,
                bold,
                italic,
                size: d.get(b"Size").and_then(Object::as_f64).unwrap_or(12.0),
                color: if c.len() == 3 { [c[0], c[1], c[2]] } else { [0.0; 3] },
                ink: ink(doc, d),
                align,
                aligns: crate::paragraph_align::tidy(aligns, align),
                // Items written before character spacing have none.
                char_spacing: d.get(b"CharSpacing").and_then(Object::as_f64).filter(|v| v.is_finite()).unwrap_or(0.0),
            };
            // An ink whose look isn't the stored colour (a file edited by hand) is none.
            t.drop_stale_ink();
            // One that stands is the ink as painted, finer digits dropped, and its look.
            if let Some(i) = t.ink {
                t.set_ink(Some(i));
            }
            Some(Content::Text(t))
        }
        b"Image" => {
            let crop = nums(doc, d.get(b"Crop"));
            Some(Content::Image(AddedImage {
                rect,
                image: d.get(b"Image")?.as_ref()?,
                rotation: (d.int(b"Rotate").unwrap_or(0).rem_euclid(360) / 90) as u8,
                flip_h: matches!(d.get(b"FlipH"), Some(Object::Bool(true))),
                flip_v: matches!(d.get(b"FlipV"), Some(Object::Bool(true))),
                crop: crop.try_into().unwrap_or([0.0; 4]),
            }))
        }
        _ => None,
    }
}

fn with_resolved<T>(doc: &Document, object: &Object, f: impl FnOnce(&Object) -> T) -> T {
    match object {
        Object::Ref(reference) => {
            let resolved = doc.get(*reference);
            f(&resolved)
        }
        direct => f(direct),
    }
}

fn for_each_content_entry(doc: &Document, page: &pdfcraft_model::Page, mut visit: impl FnMut(&Object)) {
    let Some(contents) = page.dict.get(b"Contents") else { return };
    with_resolved(doc, contents, |resolved| match resolved {
        Object::Array(entries) => {
            for entry in entries {
                visit(entry);
            }
        }
        _ => visit(contents),
    });
}

/// Finds page marks and editable added content in one pass over the page content entries.
pub fn scan_markers(doc: &Document) -> MarkerScan {
    let mut scan = MarkerScan::default();
    for (pi, page) in page_list(doc).iter().enumerate() {
        for_each_content_entry(doc, page, |entry| {
            with_resolved(doc, entry, |resolved| {
                let Some(dict) = resolved.as_dict() else { return };
                let tag = dict.name(b"PCMark");
                let kind = if tag == Some(&b"HeaderFooter"[..]) {
                    Some(MarkKind::HeaderFooter)
                } else if tag == Some(&b"Watermark"[..]) {
                    Some(MarkKind::Watermark)
                } else if tag == Some(&b"Background"[..]) {
                    Some(MarkKind::Background)
                } else {
                    None
                };
                if let Some(kind) = kind
                    && !scan.marks.contains(&kind)
                {
                    scan.marks.push(kind);
                }

                let Some(obj) = Object::as_ref(entry) else { return };
                if tag != Some(TAG.as_bytes()) {
                    return;
                }
                let Some(params_object) = dict.get(b"PCAdded") else { return };
                with_resolved(doc, params_object, |resolved_params| {
                    let Some(params) = resolved_params.as_dict() else { return };
                    if let Some(content) = parse(doc, params) {
                        // Display points depend on the page's /UserUnit. An item records the one it was
                        // written for; none means 1, as for items written before display space followed
                        // /UserUnit.
                        let written = params
                            .get(b"UserUnit")
                            .and_then(Object::as_f64)
                            .filter(|u| u.is_finite() && *u >= 1.0)
                            .map_or(1.0, |u| u.min(pdfcraft_model::MAX_USER_UNIT));
                        scan.added.push(Added { page: pi, obj, content: content.scaled(page.user_unit(doc) / written) });
                    }
                });
            });
        });
    }
    scan
}

fn validate(c: &Content) -> Result<(), EditError> {
    let r = c.rect();
    if !r.iter().all(|v| v.is_finite()) {
        return Err(EditError::Invalid("invalid position".into()));
    }
    match c {
        Content::Text(t) => {
            if t.text.trim().is_empty() {
                return Err(EditError::Invalid("type some text first".into()));
            }
            if !(t.size.is_finite() && (1.0..=500.0).contains(&t.size)) {
                return Err(EditError::Invalid("the font size must be between 1 and 500 points".into()));
            }
            if !(t.char_spacing.is_finite() && (-CHAR_SPACING_LIMIT..=CHAR_SPACING_LIMIT).contains(&t.char_spacing)) {
                return Err(EditError::Invalid(format!(
                    "the character spacing must be between -{CHAR_SPACING_LIMIT} and {CHAR_SPACING_LIMIT} points"
                )));
            }
            if t.ink.is_some_and(|i| !i.valid()) {
                return Err(EditError::Invalid("a gray level and each CMYK value must be from 0 to 1".into()));
            }
            if (r[2] - r[0]).abs() < 1.0 {
                return Err(EditError::Invalid("the text box is too narrow".into()));
            }
            if t.text.chars().any(is_thai) && pdfcraft_fonts::document_thai_font().is_none() {
                return Err(thai_error(GlyphError::NoFont, &t.text));
            }
            // Without the Arabic face, `draw` decides for the Arabic letters themselves (#403): a new
            // item is refused with what's missing, and one that already holds Arabic stays movable.
            // Everything else in it is drawn in the standard font, so it still has to fit.
            let undrawable = if t.text.chars().any(shaped_arabic) && pdfcraft_fonts::document_arabic_font().is_none() {
                t.text.split('\n').find_map(|line| {
                    line.chars()
                        .find(|c| !(shaped_arabic(*c) || is_thai(*c) && pdfcraft_fonts::thai_has(*c) || pdfcraft_fonts::win_ansi_encodable(*c)))
                })
            } else {
                first_undrawable(t)
            };
            if let Some(c) = undrawable {
                return Err(crate::undrawable(c));
            }
        }
        Content::Image(_) => {
            if (r[2] - r[0]).abs() < 1.0 || (r[3] - r[1]).abs() < 1.0 {
                return Err(EditError::Invalid("the image is too small".into()));
            }
        }
    }
    Ok(())
}

/// Write an item's stream (new, or replacing `obj`) and make its resources available to the page.
fn write(doc: &mut Document, page: usize, c: &Content, obj: Option<ObjRef>) -> Result<ObjRef, EditError> {
    validate(c)?;
    let all = page_list(doc);
    check(&[page], all.len())?;
    let p = &all[page];
    let view = p.view_matrix(doc);
    let user_unit = p.user_unit(doc);
    // The page gets its own copy of its resources with the item's fonts and images added, and
    // without the Arabic fonts of the version it replaces.
    let mut pres = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    if let Some(r) = obj {
        drop_fonts(doc, &mut pres, &own_fonts(doc, r));
    }
    let taken = pres.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    let (content, res) = draw(doc, c, view, &taken, obj.is_some())?;
    if obj.is_none() {
        crate::check_on_top(doc, p)?;
    }
    for (k, v) in res.iter() {
        let mut sub = pres.get(k).map(|s| doc.resolve(s)).and_then(|s| s.as_dict().cloned()).unwrap_or_default();
        for (n2, o) in v.as_dict().into_iter().flat_map(|d| d.iter()) {
            sub.set(n2.clone(), o.clone());
        }
        pres.set(k.clone(), Object::Dict(sub));
    }
    doc.update_dict(p.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(pres)))?;
    let mut sd = Dict::new();
    sd.set(b"PCMark".to_vec(), Object::name(TAG));
    let mut added = params(c);
    if user_unit != 1.0 {
        added.set(b"UserUnit".to_vec(), Object::Real(user_unit));
    }
    sd.set(b"PCAdded".to_vec(), Object::Dict(added));
    let stream = Stream::flate(sd, &content);
    match obj {
        Some(r) => {
            doc.set(r, Object::Stream(stream));
            Ok(r)
        }
        None => {
            // place_tagged appends a new stream; recover its reference.
            let p = page_list(doc).swap_remove(page);
            place_tagged(doc, &p, TAG, content.clone(), false)?;
            let p = page_list(doc).swap_remove(page);
            let r = contents(doc, &p)?.last().and_then(Object::as_ref).ok_or_else(|| EditError::Invalid("could not add the content".into()))?;
            doc.set(r, Object::Stream(stream));
            Ok(r)
        }
    }
}

/// Every added item, page by page, in drawing order.
pub fn list_added(doc: &Document) -> Vec<Added> {
    scan_markers(doc).added
}

/// Add an item to a page (0-based). Returns its index among the page's added items.
pub fn add_content(doc: &mut Document, page: usize, c: &Content) -> Result<usize, EditError> {
    write(doc, page, c, None)?;
    Ok(list_added(doc).iter().filter(|a| a.page == page).count() - 1)
}

fn find(doc: &Document, page: usize, index: usize) -> Result<Added, EditError> {
    list_added(doc)
        .into_iter()
        .filter(|a| a.page == page)
        .nth(index)
        .ok_or_else(|| EditError::Invalid(format!("page {} has no added item {}", page.saturating_add(1), index.saturating_add(1))))
}

/// Replace item `index` of `page` (moved, resized, retyped or reformatted).
pub fn update_content(doc: &mut Document, page: usize, index: usize, c: &Content) -> Result<(), EditError> {
    let a = find(doc, page, index)?;
    if std::mem::discriminant(&a.content) != std::mem::discriminant(c) {
        return Err(EditError::Invalid("an item can't change between text and image".into()));
    }
    write(doc, page, c, Some(a.obj))?;
    Ok(())
}

/// Remove item `index` of `page`.
pub fn delete_content(doc: &mut Document, page: usize, index: usize) -> Result<(), EditError> {
    let a = find(doc, page, index)?;
    let p = page_list(doc).swap_remove(page);
    let list: Vec<Object> = contents(doc, &p)?.into_iter().filter(|o| o.as_ref() != Some(a.obj)).collect();
    let fonts = own_fonts(doc, a.obj);
    // Only the page's own resources carry an item's fonts (`write` gives the page a copy).
    let res = match p.dict.get(b"Resources") {
        Some(Object::Dict(d)) if !fonts.is_empty() => Some(d.clone()),
        _ => None,
    };
    let res = res.map(|mut r| {
        drop_fonts(doc, &mut r, &fonts);
        r
    });
    doc.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), Object::Array(list));
        if let Some(r) = res {
            d.set(b"Resources".to_vec(), Object::Dict(r));
        }
    })?;
    Ok(())
}

/// Leave only item `index` on `page`: its contents become the item's own stream and its
/// annotations go, so a render of the page (on a transparent backdrop) is the item's pixels
/// alone. For previews on a copy of the document; the page's resources stay as they are.
pub fn isolate_content(doc: &mut Document, page: usize, index: usize) -> Result<(), EditError> {
    let a = find(doc, page, index)?;
    let p = page_list(doc).swap_remove(page);
    // An item's stream starts from the page's initial graphics state: `place_tagged` closes
    // everything drawn before it, so it draws the same on its own.
    doc.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), Object::Array(vec![Object::Ref(a.obj)]));
        d.remove(b"Annots");
    })?;
    Ok(())
}

/// The Type 3 fonts an Arabic item's stream shows text with. Each item has its own (`draw_shaped`
/// names them so), so they go when the item is replaced or deleted.
fn own_fonts(doc: &Document, obj: ObjRef) -> Vec<Vec<u8>> {
    let Object::Stream(s) = &*doc.get(obj) else { return Vec::new() };
    let Ok(data) = s.decoded() else { return Vec::new() };
    data.split(u8::is_ascii_whitespace).filter_map(|w| w.strip_prefix(b"/")).filter(|w| w.starts_with(b"PCAr")).map(<[u8]>::to_vec).collect()
}

fn drop_fonts(doc: &Document, res: &mut Dict, names: &[Vec<u8>]) {
    if names.is_empty() {
        return;
    }
    let Some(mut fonts) = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()) else { return };
    for name in names {
        fonts.remove(name);
    }
    res.set(b"Font".to_vec(), Object::Dict(fonts));
}
