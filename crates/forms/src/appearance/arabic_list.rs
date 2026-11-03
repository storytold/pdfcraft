//! Bounded Arabic/Latin list rows using the PR #3465 appearance_text writer.
//! Mixed Syriac lists retain their existing selected-family route.
use super::{Da, Dict, Document, Field, FieldKind, Object, Placement, Stream, flags, frame, n, set_form_box};
use pdfcraft_fonts::{TextDirection, appearance_text as text};

/// A list fallback inspects at most 1024 labels and 262144 aggregate UTF-8 bytes.
fn bounded(labels: &[String]) -> bool {
    labels.len() <= 1024 && labels.iter().fold(0usize, |n, label| n.saturating_add(label.len())) <= 262_144
}

/// Keep a wholly encodable selected face; never mix the Arabic fallback with a Syriac list.
fn requested(doc: &Document, da: &Da, labels: &[String]) -> bool {
    bounded(labels)
        && labels.iter().any(|s| s.chars().any(pdfcraft_fonts::source_shaping::is_arabic))
        && !labels.iter().any(|s| pdfcraft_fonts::content_text::contains_syriac(s))
        && super::dr_font(doc, &da.font)
            .is_none_or(|(_, font)| labels.iter().any(|s| font.first_unencodable(&s.replace(['\r', '\n'], " ")).is_some()))
}

/// Shape each option as a single row and derive one common ink-safe row height.
fn layout(doc: &Document, da: &Da, labels: &[String], size: f64) -> Option<(Vec<text::Line>, f64)> {
    if !requested(doc, da, labels) || labels.iter().any(|s| !text::can_shape(s)) {
        return None;
    }
    let latin = super::arabic::latin_face(doc, &da.font);
    let mut lines = Vec::with_capacity(labels.len());
    let mut row = size * 1.15;
    for label in labels {
        let flat = label.replace(['\r', '\n'], " ");
        let line = text::wrap_with_latin_bounded(&flat, size, 1.0e100, latin, 1).ok()?.into_iter().next()?;
        row = row.max((line.ascent + line.descent + 0.1) * size);
        lines.push(line);
    }
    Some((lines, row))
}

/// Read valid option labels without admitting an unbounded option vector into shaping.
fn labels(doc: &Document, field: Option<&Dict>) -> Option<Vec<String>> {
    let options = doc.resolve(field?.get(b"Opt")?);
    let options = options.as_array()?;
    if options.len() > 1024 {
        return None;
    }
    let mut out = Vec::new();
    let mut bytes = 0usize;
    for option in options {
        let option = doc.resolve(option);
        let value = if let Some(pair) = option.as_array() {
            let (Some(export), Some(label)) = (pair.first(), pair.get(1)) else { continue };
            let export = doc.resolve(export);
            if crate::text_of(&export).is_none() {
                continue;
            }
            doc.resolve(label)
        } else {
            option.clone()
        };
        if let Some(label) = crate::text_of(&value) {
            bytes = bytes.saturating_add(label.len());
            if bytes > 262_144 {
                return None;
            }
            out.push(label);
        }
    }
    Some(out)
}

/// Reader, hit testing, selection scrolling and drawing share these row metrics.
pub(super) fn row_height(doc: &Document, field: Option<&Dict>, da: &Da, direction: TextDirection, size: f64) -> Option<f64> {
    if direction != TextDirection::Auto {
        return None;
    }
    layout(doc, da, &labels(doc, field)?, size).map(|(_, row)| row)
}

/// Only Auto lists without password or comb semantics enter the Arabic list renderer.
pub(super) fn eligible(f: &Field) -> bool {
    f.kind == FieldKind::List && f.text_direction == TextDirection::Auto && !f.has(flags::PASSWORD) && !f.is_comb()
}

/// Whether regeneration must allocate fresh font resources rather than reuse a cached stream.
pub(super) fn needs(doc: &Document, f: &Field, da: &Da) -> bool {
    eligible(f) && field_labels(f).is_some_and(|labels| requested(doc, da, &labels))
}

/// Apply aggregate limits before cloning a field's parsed option labels.
fn field_labels(f: &Field) -> Option<Vec<String>> {
    if f.options.len() > 1024 || f.options.iter().fold(0usize, |n, (_, label)| n.saturating_add(label.len())) > 262_144 {
        return None;
    }
    Some(f.options.iter().map(|(_, label)| label.clone()).collect())
}

/// Paint visible rows and highlights while retaining logical exports and selection indices.
pub(super) fn field(doc: &Document, f: &Field, values: &[String], wd: &Dict, da: &Da, place: Placement) -> Option<(Stream, Option<char>)> {
    if !eligible(f) || f.options.len() > 1024 {
        return None;
    }
    let labels = field_labels(f)?;
    let size = if da.size > 0.0 { da.size } else { 12.0 };
    let (lines, row) = layout(doc, da, &labels, size)?;
    let (width, height) = (place.layout_w, place.layout_h);
    let (frame, border) = frame(doc, wd, width, height);
    let field = doc.get(f.obj);
    // Layout already measured all labels; do not repeat shaping just to read /TI.
    let rows = super::ListRows { top: super::top_index(doc, wd, field.as_dict()), inset: 2.0 + border, row, size };
    let count = rows.visible(height).min(lines.len().saturating_sub(rows.top));
    let visible = lines.get(rows.top..rows.top.saturating_add(count))?;
    let mut backgrounds = String::new();
    let mut positions = Vec::with_capacity(count);
    let alignment = wd.get(b"Q").and_then(|o| doc.resolve(o).as_int()).unwrap_or(f.quadding);
    let pad = 2.0 + border;
    let mut top = height - rows.inset;
    for (offset, line) in visible.iter().enumerate() {
        let (export, _) = f.options.get(rows.top.saturating_add(offset))?;
        if values.contains(export) {
            backgrounds.push_str(&format!(
                "0.6 0.75 0.86 rg {} {} {} {} re f\n",
                n(border),
                n(top - row),
                n((width - 2.0 * border).max(0.0)),
                n(row)
            ));
        }
        let x = match alignment {
            1 => pad + ((width - 2.0 * pad).max(1.0) - line.width) / 2.0,
            2 => width - pad - line.width,
            _ => pad,
        };
        let y = top - (row - (line.ascent + line.descent) * size) / 2.0 - line.ascent * size;
        positions.push([x, y]);
        top -= row;
    }
    let (drawn, fonts) = text::draw_positioned(visible, size, &positions, None).ok()?;
    let mut content = format!(
        "{frame}/Tx BMC\nq\n{} {} {} {} re W n\n{backgrounds}{}\n",
        n(border),
        n(border),
        n((width - 2.0 * border).max(0.0)),
        n((height - 2.0 * border).max(0.0)),
        da.color
    )
    .into_bytes();
    content.extend(drawn);
    content.extend_from_slice(b"Q\nEMC\n");
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    let mut dict = Dict::new();
    dict.set(b"Type".to_vec(), Object::name("XObject"));
    dict.set(b"Subtype".to_vec(), Object::name("Form"));
    set_form_box(&mut dict, place);
    dict.set(b"Resources".to_vec(), Object::Dict(resources));
    Some((Stream::flate(dict, &content), labels.iter().find_map(|s| text::first_undrawable(s))))
}
