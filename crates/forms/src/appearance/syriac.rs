//! Syriac appearances share one document-backed writer with the other content surfaces.
use super::{
    Dict, Document, Field, FieldKind, Object, Overflow, Stream, TextBox, Widget, comb_dividers, flags, frame, list_rows, mk_rotation, n, parse_da,
    placement, set_form_box,
};
use pdfcraft_fonts::{SyriacFamily, content_text as text};

/// Detect unmasked Syriac text, option labels or captions on eligible field kinds.
/// Button state names are not text; this predicate does not filter hidden widgets.
pub(crate) fn requested(doc: &Document, f: &Field, w: &Widget, values: &[String]) -> bool {
    if f.has(flags::PASSWORD) || !matches!(f.kind, FieldKind::Text | FieldKind::Combo | FieldKind::List | FieldKind::PushButton) {
        return false;
    }
    values.iter().any(|value| text::contains_syriac(value))
        || f.options.iter().any(|(_, display)| text::contains_syriac(display))
        || (f.kind == FieldKind::PushButton && caption(doc, w).is_some_and(|caption| text::contains_syriac(&caption)))
}

/// Read the widget caption through a possibly indirect appearance-characteristics dictionary.
fn caption(doc: &Document, w: &Widget) -> Option<String> {
    let widget = doc.get(w.obj);
    let mk = widget.as_dict()?.get(b"MK").and_then(|mk| doc.dict(mk))?;
    mk.get(b"CA").and_then(|caption| doc.resolve(caption).as_string().map(|s| s.to_text()))
}

/// Combine baseline-relative ink bounds while retaining minimum Latin ascent and descent.
fn extents(lines: &[text::Line]) -> Result<(f64, f64), String> {
    let (mut top, mut bottom) = (0.718f64, -0.207f64);
    for line in lines {
        let (a, d) = line.vertical_extents()?;
        top = top.max(a);
        bottom = bottom.min(d);
    }
    Ok((top, bottom))
}

/// Measure list-label ink bounds with the selected family, refusing more than 1024 options.
pub(super) fn option_extents(f: &Field, family: SyriacFamily) -> Result<(f64, f64), String> {
    if f.options.len() > 1024 {
        return Err("the Syriac choice list has too many options".into());
    }
    let (mut top, mut bottom) = (0.718f64, -0.207f64);
    for (_, label) in &f.options {
        let row = text::line_for_family(label, unicode_bidi_rtl(label, family), family)?;
        let (a, d) = row.vertical_extents()?;
        top = top.max(a);
        bottom = bottom.min(d);
    }
    Ok((top, bottom))
}

/// Lay out a field value with the selected face, wrapping multiline text and flattening other breaks.
fn layout(value: &str, size: f64, width: f64, multiline: bool, family: SyriacFamily) -> Result<Vec<text::Line>, String> {
    if multiline {
        text::wrap_for_family(value, size, width, family)
    } else {
        let shown = value.replace(['\n', '\r'], " ");
        Ok(vec![text::line_for_family(&shown, unicode_bidi_rtl(&shown, family), family)?])
    }
}

/// Ask the shared paragraph layout for base direction, defaulting to LTR if layout fails.
fn unicode_bidi_rtl(value: &str, family: SyriacFamily) -> bool {
    // The layout helper owns UAX #9; an unwrapped paragraph gets the same base direction.
    text::wrap_for_family(value, 1.0, f64::MAX, family).ok().and_then(|lines| lines.first().map(|line| line.rtl)).unwrap_or(false)
}

/// Use an explicit size or compute an automatic size from shaped bounds, with a 4-point minimum.
fn size_for(value: &str, wanted: Option<f64>, width: f64, height: f64, multiline: bool, family: SyriacFamily) -> Result<f64, String> {
    if let Some(size) = wanted {
        return Ok(size);
    }
    if multiline {
        let mut size = 12.0;
        while size > 4.0 {
            let rows = layout(value, size, width, true, family)?;
            let (top, bottom) = extents(&rows)?;
            if rows.len() as f64 * (top - bottom + 0.1).max(1.15) * size <= height {
                break;
            }
            size -= 0.5;
        }
        Ok(size)
    } else {
        let rows = layout(value, 1.0, f64::MAX, false, family)?;
        let (top, bottom) = extents(&rows)?;
        let advance = rows.first().map_or(0.0, |line| line.width);
        let by_height = height / (top - bottom).max(0.1);
        let by_width = if advance > 0.0 { width / advance } else { 500.0 };
        Ok(by_height.min(by_width).clamp(4.0, 500.0))
    }
}

/// Fit the widest visible cell and ink height against the full bounded comb cell count.
fn comb_auto_size(width: f64, height: f64, cells: usize, widest: f64, ink_height: f64) -> Result<f64, String> {
    if !(1..=4096).contains(&cells)
        || !width.is_finite()
        || width <= 0.0
        || !height.is_finite()
        || height <= 0.0
        || !widest.is_finite()
        || widest < 0.0
        || !ink_height.is_finite()
        || ink_height <= 0.0
    {
        return Err("Syriac comb auto-size needs bounded cells and positive finite geometry".into());
    }
    let by_width = if widest > 0.0 { width / cells as f64 / widest } else { 500.0 };
    let size = by_width.min(height / ink_height).min(500.0);
    if !size.is_finite() || size <= 0.0 {
        return Err("the Syriac comb cells leave no room for text".into());
    }
    Ok(size)
}

/// Report measured fit separately from shaping errors so callers can apply fallback metrics.
pub(super) fn fits(box_: &TextBox, value: &str) -> Result<bool, String> {
    let size = size_for(value, box_.size, box_.inner_w, box_.inner_h, box_.multiline, box_.syriac_family)?;
    let rows = layout(value, size, box_.inner_w, box_.multiline, box_.syriac_family)?;
    let (top, bottom) = extents(&rows)?;
    let pitch = (top - bottom + 0.1).max(1.15) * size;
    let height = (top - bottom) * size + rows.len().saturating_sub(1) as f64 * pitch;
    Ok(rows.iter().all(|line| line.width * size <= box_.inner_w + 0.01) && height <= box_.inner_h + 0.01)
}

/// Measure the same visible isolated cells, fixed/automatic size and full cell width as drawing.
/// Sparse values retain the field's full MaxLen count; no joined-word width substitutes for a cell.
fn comb_overflow(box_: &TextBox, f: &Field, shown: &str) -> Result<Option<Overflow>, String> {
    let cells = f.max_len.unwrap_or(1).max(1);
    if cells > 4096 {
        return Err("the Syriac comb field has too many cells".into());
    }
    let rows = text::comb_cells_for_family(shown, box_.syriac_family)?;
    let visible = rows.get(..rows.len().min(cells)).unwrap_or_default();
    let (top, bottom) = extents(visible)?;
    let widest = visible.iter().map(|line| line.width).fold(0.0, f64::max);
    let size = match box_.size {
        Some(size) => size,
        None => comb_auto_size(box_.outer_w, box_.inner_h, cells, widest, (top - bottom).max(0.1))?,
    };
    let height = (top - bottom) * size;
    if height > box_.inner_h + 0.01 {
        return Ok(Some(Overflow::High { height, room: box_.inner_h, size }));
    }
    let room = box_.outer_w / cells as f64;
    let width = widest * size;
    Ok((width > room + 0.01).then_some(Overflow::Wide { width, room, size }))
}

/// Classify the already formatted/display-label-resolved text with its selected widget family.
/// Keep shaping failures distinct so the caller can retain the established fallback metrics.
pub(super) fn overflow(box_: &TextBox, f: &Field, shown: &str) -> Result<Option<Overflow>, String> {
    if shown.len() > text::MAX_TEXT_BYTES {
        return Err("the Syriac field value exceeds its text budget".into());
    }
    if box_.comb && f.kind == FieldKind::Text {
        return comb_overflow(box_, f, shown);
    }
    // As `field` draws it: only a text field wraps; a combo shows one line whatever its flags.
    let multiline = box_.multiline && f.kind == FieldKind::Text;
    let size = size_for(shown, box_.size, box_.inner_w, box_.inner_h, multiline, box_.syriac_family)?;
    let rows = layout(shown, size, box_.inner_w, multiline, box_.syriac_family)?;
    let (top, bottom) = extents(&rows)?;
    let height = (top - bottom) * size;
    if height > box_.inner_h + 0.01 {
        return Ok(Some(Overflow::High { height, room: box_.inner_h, size }));
    }
    if multiline {
        let room = (((box_.inner_h - height).max(0.0) / ((top - bottom + 0.1).max(1.15) * size)).floor() as usize).saturating_add(1);
        if room < rows.len() {
            return Ok(Some(Overflow::Tall { lines: rows.len(), room, size }));
        }
    }
    if let Some(width) = rows.iter().map(|line| line.width * size).filter(|width| *width > box_.inner_w + 0.01).reduce(f64::max) {
        return Ok(Some(Overflow::Wide { width, room: box_.inner_w, size }));
    }
    Ok(None)
}

/// Measure only labels drawn after `/TI`, using the writer's row size, family and rotated box.
/// Shaping failures remain distinct from measured overflow; export values never supply metrics.
pub(super) fn list_overflow(doc: &Document, f: &Field, w: &Widget) -> Result<Option<Overflow>, String> {
    let box_ = super::text_box(doc, f, w);
    // The writer validates all option extents before drawing any visible label.
    option_extents(f, box_.syriac_family)?;
    let widget = doc.get(w.obj);
    let wd = widget.as_dict().cloned().unwrap_or_default();
    let place = placement(mk_rotation(doc, &wd), (w.rect[2] - w.rect[0]).max(1.0), (w.rect[3] - w.rect[1]).max(1.0));
    let rows = list_rows(doc, f, w);
    let mut y = place.layout_h - rows.inset;
    let mut width = 0.0f64;
    for (_, label) in f.options.iter().skip(rows.top) {
        if y - rows.row < 0.0 {
            break;
        }
        if label.len() > text::MAX_TEXT_BYTES {
            return Err("the Syriac choice list exceeds its drawing budget".into());
        }
        let line = text::line_for_family(label, unicode_bidi_rtl(label, box_.syriac_family), box_.syriac_family)?;
        width = width.max(line.width * rows.size);
        y -= rows.row;
    }
    Ok((width > box_.inner_w + 0.01).then_some(Overflow::Wide { width, room: box_.inner_w, size: rows.size }))
}

/// Build a bounded Syriac field appearance with the selected family and embedded cluster fonts.
/// Handle choice highlights, comb cells, alignment and multiline baselines in widget layout space.
pub(super) fn field(doc: &mut Document, f: &Field, w: &Widget, values: &[String], format: bool) -> Result<Stream, String> {
    let family = super::syriac_family(doc, f, w);
    let widget = doc.get(w.obj);
    let wd = widget.as_dict().cloned().unwrap_or_default();
    let place = placement(mk_rotation(doc, &wd), (w.rect[2] - w.rect[0]).max(1.0), (w.rect[3] - w.rect[1]).max(1.0));
    let (width, height) = (place.layout_w, place.layout_h);
    let da = parse_da(wd.get(b"DA").and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text())).as_deref().unwrap_or(&f.da));
    let (background, border) = frame(doc, &wd, width, height);
    let pad = border + 2.0;
    let inner_w = (width - 2.0 * pad).max(1.0);
    let inner_h = (height - 2.0 * pad).max(1.0);
    let alignment = wd.get(b"Q").and_then(|o| doc.resolve(o).as_int()).unwrap_or(f.quadding);
    let mut value = String::new();
    for (index, value_part) in values.iter().take(if f.kind == FieldKind::Combo { values.len() } else { 1 }).enumerate() {
        let part = if f.kind == FieldKind::Combo {
            f.options.iter().find(|(export, _)| export == value_part).map_or(value_part.as_str(), |(_, label)| label.as_str())
        } else {
            value_part.as_str()
        };
        if value.len().saturating_add(part.len()).saturating_add(2) > text::MAX_TEXT_BYTES {
            return Err("the Syriac field value exceeds its text budget".into());
        }
        if index > 0 {
            value.push_str(", ");
        }
        value.push_str(part);
    }
    if f.kind == FieldKind::Combo {
        value = super::displayed_text(f, values, format);
    } else if format && matches!(f.kind, FieldKind::Text | FieldKind::Combo) && f.actions.format != crate::af::Format::None {
        value = crate::af::format_value(&f.actions.format, &value);
    }
    if value.len() > text::MAX_TEXT_BYTES {
        return Err("the Syriac field value exceeds its text budget".into());
    }
    let multiline = f.kind == FieldKind::Text && f.has(flags::MULTILINE);
    let comb = if f.kind == FieldKind::Text && f.is_comb() {
        let cells = f.max_len.unwrap_or(1).max(1);
        if cells > 4096 {
            return Err("the Syriac comb field has too many cells".into());
        }
        Some((cells, text::comb_cells_for_family(&value, family)?))
    } else {
        None
    };
    let size = if let Some((cells, laid)) = &comb {
        if da.size > 0.0 {
            da.size
        } else {
            // Cells shape independently: neither the joined word's width nor the number
            // of filled cells determines the room left by the field's actual MaxLen.
            let visible = laid.get(..laid.len().min(*cells)).unwrap_or_default();
            let (top, bottom) = extents(visible)?;
            let widest = visible.iter().map(|line| line.width).fold(0.0, f64::max);
            comb_auto_size(width, inner_h, *cells, widest, (top - bottom).max(0.1))?
        }
    } else {
        size_for(&value, (da.size > 0.0).then_some(da.size), inner_w, inner_h, multiline, family)?
    };
    let mut content = format!(
        "/Tx BMC\nq\n{background}{border} {border} {} {} re W n\n{}\n",
        n((width - 2.0 * border).max(0.0)),
        n((height - 2.0 * border).max(0.0)),
        da.color
    )
    .into_bytes();
    let mut fonts = Dict::new();
    let x_for = |line: &text::Line, size: f64| match alignment {
        1 => pad + (inner_w - line.width * size) / 2.0,
        2 => width - pad - line.width * size,
        _ => pad,
    };
    if f.kind == FieldKind::List {
        let rows = list_rows(doc, f, w);
        let (top, bottom) = option_extents(f, family)?;
        let mut y = height - rows.inset;
        for (index, (export, label)) in f.options.iter().skip(rows.top).enumerate() {
            if y - rows.row < 0.0 {
                break;
            }
            if index >= 1024 || label.len() > text::MAX_TEXT_BYTES {
                return Err("the Syriac choice list exceeds its drawing budget".into());
            }
            if values.contains(export) {
                content
                    .extend(format!("0.6 0.75 0.86 rg {} {} {} {} re f\n", n(border), n(y - rows.row), n(width - 2.0 * border), n(rows.row)).bytes());
            }
            content.extend(format!("{}\n", da.color).bytes());
            let line = text::line_for_family(label, unicode_bidi_rtl(label, family), family)?;
            let baseline = y - (rows.row - (top - bottom) * rows.size) / 2.0 - top * rows.size;
            content.extend(line.write(doc, &mut fonts, [pad, baseline], rows.size)?);
            y -= rows.row;
        }
    } else if let Some((cells, laid)) = comb {
        let cell = width / cells as f64;
        content.extend(comb_dividers(doc, &wd, width, height, cells).bytes());
        let start = if unicode_bidi_rtl(&value, family) { cells.saturating_sub(laid.len().min(cells)) } else { 0 };
        for (index, line) in laid.into_iter().take(cells).enumerate() {
            let (top, bottom) = line.vertical_extents()?;
            let y = (height - (top - bottom) * size) / 2.0 - bottom * size;
            content.extend(line.write(doc, &mut fonts, [cell * index.saturating_add(start) as f64 + (cell - line.width * size) / 2.0, y], size)?);
        }
    } else {
        let rows = layout(&value, size, inner_w, multiline, family)?;
        let (top, bottom) = extents(&rows)?;
        let mut y = if multiline { height - pad - top * size } else { (height - (top - bottom) * size) / 2.0 - bottom * size };
        for line in rows {
            content.extend(line.write(doc, &mut fonts, [x_for(&line, size), y], size)?);
            y -= (top - bottom + 0.1).max(1.15) * size;
        }
    }
    content.extend_from_slice(b"Q\nEMC\n");
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    let mut dict = Dict::new();
    dict.set(b"Type".to_vec(), Object::name("XObject"));
    dict.set(b"Subtype".to_vec(), Object::name("Form"));
    set_form_box(&mut dict, place);
    dict.set(b"Resources".to_vec(), Object::Dict(resources));
    Ok(Stream::flate(dict, &content))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sparse comb values still use MaxLen cell widths, actual mark height and bounded geometry.
    #[test]
    fn sparse_syriac_comb_auto_size_uses_the_maximum_cell_count() {
        // One 2-em isolated letter in a 24-cell field still has only 10 pt of room.
        assert_eq!(comb_auto_size(240.0, 30.0, 24, 2.0, 1.5).unwrap(), 5.0);
        // Font ink height, including vowel marks, can be the tighter constraint.
        assert_eq!(comb_auto_size(240.0, 6.0, 24, 2.0, 1.5).unwrap(), 4.0);
        assert!(comb_auto_size(240.0, 30.0, 0, 2.0, 1.5).is_err());
        assert!(comb_auto_size(240.0, 30.0, 4097, 2.0, 1.5).is_err());
        assert!(comb_auto_size(f64::NAN, 30.0, 24, 2.0, 1.5).is_err());
    }

    /// Syriac checkbox/radio state names must not replace their non-text appearance path.
    #[test]
    fn syriac_export_values_do_not_request_text_appearances_for_buttons() {
        let mut doc = Document::new_empty();
        let mut widget = Dict::new();
        widget.set(b"FT".to_vec(), Object::name("Btn"));
        widget.set(b"Subtype".to_vec(), Object::name("Widget"));
        widget.set(b"T".to_vec(), pdfcraft_cos::PdfString::text("choice"));
        widget.set(b"Rect".to_vec(), Object::Array([0, 0, 20, 20].into_iter().map(Object::Int).collect()));
        let object = doc.add(Object::Dict(widget));
        let mut form = Dict::new();
        form.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(object)]));
        doc.update_dict(doc.root().unwrap(), |catalog| catalog.set(b"AcroForm".to_vec(), Object::Dict(form))).unwrap();
        let mut field = crate::fields(&doc).remove(0);
        let widget = field.widgets.first().unwrap().clone();
        let selected = vec!["ܫܠܡܐ".to_owned()];
        // These values identify selected states; they are never visible text captions.
        for kind in [FieldKind::CheckBox, FieldKind::Radio, FieldKind::Signature] {
            field.kind = kind;
            assert!(!requested(&doc, &field, &widget, &selected), "{kind:?} must retain its existing appearance path");
        }
        field.kind = FieldKind::Text;
        assert!(requested(&doc, &field, &widget, &selected), "the same Unicode remains supported as actual text");
    }
}
