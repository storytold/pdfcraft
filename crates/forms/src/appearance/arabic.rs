//! Bounded Auto Arabic forms, adapted from PR #3465 (285d4697) shaped_field.
//! Keep its appearance_text writer, with measured overflow and the existing display policy.
//! This fallback leaves encodable custom fonts unchanged, even when their existing
//! Arabic letters are unjoined; source-face shaping is outside this first slice.
use super::{Da, Dict, Document, Field, FieldKind, Object, Overflow, Placement, Stream, flags, frame, n, set_form_box};
use pdfcraft_fonts::{Std14, appearance_text as text};

/// Auto text and combo appearances exclude explicit direction and passwords.
pub(super) fn eligible(f: &Field) -> bool {
    matches!(f.kind, FieldKind::Text | FieldKind::Combo) && f.text_direction == pdfcraft_fonts::TextDirection::Auto && !f.has(flags::PASSWORD)
}

/// Syriac keeps its existing selected-family route; Arabic requests use visible text only.
pub(super) fn requested(f: &Field, shown: &str) -> bool {
    eligible(f)
        && shown.len() <= pdfcraft_fonts::text_direction::MAX_DIRECTION_BYTES
        && !pdfcraft_fonts::content_text::contains_syriac(shown)
        && shown.chars().any(pdfcraft_fonts::source_shaping::is_arabic)
}

/// Recognized standard faces retain mixed Latin metrics; a family-name hint is insufficient.
pub(super) fn latin_face(doc: &Document, name: &str) -> Std14 {
    super::dr_font(doc, name)
        .and_then(|(font, _)| {
            let font = doc.resolve(&font);
            let base = std::str::from_utf8(font.as_dict()?.name(b"BaseFont")?).ok()?;
            Std14::recognise(base)
        })
        .filter(|(font, matched)| *matched == pdfcraft_fonts::Std14Match::Alias && !matches!(font, Std14::Symbol | Std14::ZapfDingbats))
        .map_or(Std14::Helvetica, |(font, _)| font)
}

/// Measure the same flattened, unwrapped line used for drawing, retaining finite font bounds.
fn single_line(shown: &str, wanted: Option<f64>, width: f64, height: f64, latin: Std14) -> Option<(text::Line, f64, Option<Overflow>)> {
    if shown.len() > pdfcraft_fonts::text_direction::MAX_DIRECTION_BYTES || !text::can_shape(shown) {
        return None;
    }
    let shown = shown.replace(['\r', '\n'], " ");
    let measure = |size| text::wrap_with_latin(&shown, size, 1.0e100, latin).ok()?.into_iter().next();
    let measured = measure(1.0)?;
    let ink_height = measured.ascent + measured.descent;
    let size = wanted.unwrap_or_else(|| {
        let by_height = height / ink_height.max(0.1);
        let by_width = if measured.width > 0.0 { width / measured.width } else { 500.0 };
        by_height.min(by_width).clamp(4.0, 500.0)
    });
    if !size.is_finite() || size <= 0.0 {
        return None;
    }
    let line = measure(size)?;
    let ink_height = (line.ascent + line.descent) * size;
    let overflow = if ink_height > height + 0.01 {
        Some(Overflow::High { height: ink_height, room: height, size })
    } else if line.width > width + 0.01 {
        Some(Overflow::Wide { width: line.width, room: width, size })
    } else {
        None
    };
    Some((line, size, overflow))
}

/// Rows measured before clipping, shared by fitting, overflow and drawing.
struct Layout {
    lines: Vec<text::Line>,
    size: f64,
    overflow: Option<Overflow>,
}

/// Each grapheme is shaped independently; bidi assigns its cell without joining neighbors.
fn comb_layout(shown: &str, wanted: Option<f64>, width: f64, height: f64, cells: usize, latin: Std14) -> Option<(Layout, Vec<[f64; 2]>)> {
    if !(1..=4096).contains(&cells) || !width.is_finite() || width <= 0.0 || !height.is_finite() || height <= 0.0 || !text::can_shape(shown) {
        return None;
    }
    let (measured, _) = text::comb_with_latin(shown, 1.0, cells, latin).ok()?;
    let extents: Vec<_> = measured.iter().map(|line| line.ink_bounds(1.0)).collect::<Result<_, _>>().ok()?;
    let widest = extents.iter().map(|b| b[2] - b[0]).fold(0.0f64, f64::max);
    let tallest = extents.iter().map(|b| b[3] - b[1]).fold(0.1f64, f64::max);
    let cell = width / cells as f64;
    let size = wanted.unwrap_or_else(|| (height / tallest).min(if widest > 0.0 { cell / widest } else { 500.0 }).min(500.0));
    if !size.is_finite() || size <= 0.0 {
        return None;
    }
    let (lines, slots) = text::comb_with_latin(shown, size, cells, latin).ok()?;
    let overflow = if tallest * size > height + 0.01 {
        Some(Overflow::High { height: tallest * size, room: height, size })
    } else if widest * size > cell + 0.01 {
        Some(Overflow::Wide { width: widest * size, room: cell, size })
    } else {
        None
    };
    let positions = slots
        .iter()
        .zip(extents)
        .map(|(slot, b)| {
            [cell * *slot as f64 + (cell - (b[2] - b[0]) * size) / 2.0 - b[0] * size, (height - (b[3] - b[1]) * size) / 2.0 - b[1] * size]
        })
        .collect();
    Some((Layout { lines, size, overflow }, positions))
}

/// At most 1024 rows and 17 automatic-size candidates (12 down to 4 points).
/// An automatic candidate that exhausts the row cap retries smaller sizes; fixed sizes refuse.
fn layout(shown: &str, wanted: Option<f64>, width: f64, height: f64, latin: Std14, multiline: bool) -> Option<Layout> {
    if !multiline {
        return single_line(shown, wanted, width, height, latin).map(|(line, size, overflow)| Layout { lines: vec![line], size, overflow });
    }
    if shown.len() > pdfcraft_fonts::text_direction::MAX_DIRECTION_BYTES || !text::can_shape(shown) {
        return None;
    }
    let mut size = wanted.unwrap_or(12.0);
    for attempt in 0..17 {
        let lines = match text::wrap_with_latin_bounded(shown, size, width, latin, 1024) {
            Ok(lines) => lines,
            Err(pdfcraft_fonts::GlyphError::TooComplex) if wanted.is_none() && attempt < 16 => {
                size -= 0.5;
                continue;
            }
            Err(_) => return None,
        };
        let full_height = text::height(&lines, size);
        let high = lines.iter().map(|line| (line.ascent + line.descent) * size).find(|height_| *height_ > height + 0.01);
        let wide = lines.iter().map(|line| line.width).find(|width_| *width_ > width + 0.01);
        let overflow = if let Some(ink_height) = high {
            Some(Overflow::High { height: ink_height, room: height, size })
        } else if let Some(ink_width) = wide {
            Some(Overflow::Wide { width: ink_width, room: width, size })
        } else if full_height > height + 0.01 {
            let mut used = 0.0;
            let room = lines
                .iter()
                .take_while(|line| {
                    used += (line.ascent + line.descent) * size;
                    used <= height + 0.01
                })
                .count();
            Some(Overflow::Tall { lines: lines.len(), room, size })
        } else {
            None
        };
        if wanted.is_some() || overflow.is_none() || attempt == 16 {
            return Some(Layout { lines, size, overflow });
        }
        size -= 0.5;
    }
    None
}

/// Edit-fit checks share the actual shaped advances and positioned mark extents.
pub(super) fn fits(box_: &super::TextBox, shown: &str) -> Option<bool> {
    layout(shown, box_.size, box_.inner_w, box_.inner_h + if box_.multiline { 2.0 } else { 0.0 }, box_.arabic_latin, box_.multiline)
        .map(|layout| layout.overflow.is_none())
}

/// Adapt PR #3465's framed writer to bounded rows or comb cells with truthful overflow.
pub(super) fn field(
    doc: &Document,
    f: &Field,
    shown: &str,
    wd: &Dict,
    da: &Da,
    place: Placement,
) -> Option<(Stream, Option<char>, Option<Overflow>)> {
    let (width, height) = (place.layout_w, place.layout_h);
    let (mut frame, border) = frame(doc, wd, width, height);
    let pad = 2.0 + border;
    let inner_w = (width - 2.0 * pad).max(1.0);
    let multiline = f.kind == FieldKind::Text && f.has(flags::MULTILINE);
    let comb = if f.is_comb() {
        let cells = f.max_len?;
        frame.push_str(&super::comb_dividers(doc, wd, width, height, cells));
        Some(comb_layout(shown, (da.size > 0.0).then_some(da.size), width, height - 2.0 * pad, cells, latin_face(doc, &da.font))?)
    } else {
        None
    };
    let (laid, comb_positions) = if let Some((laid, mut positions)) = comb {
        for point in &mut positions {
            point[1] += pad;
        }
        (laid, Some(positions))
    } else {
        (
            layout(
                shown,
                (da.size > 0.0).then_some(da.size),
                inner_w,
                height - 2.0 * pad + if multiline { 2.0 } else { 0.0 },
                latin_face(doc, &da.font),
                multiline,
            )?,
            None,
        )
    };
    let size = laid.size;
    let q = wd.get(b"Q").and_then(|o| doc.resolve(o).as_int()).unwrap_or(f.quadding);
    let mut top = height - pad;
    let positions: Vec<[f64; 2]> = comb_positions.unwrap_or_else(|| {
        laid.lines
            .iter()
            .map(|line| {
                let x = match q {
                    1 => pad + (inner_w - line.width) / 2.0,
                    2 => width - pad - line.width,
                    _ => pad,
                };
                let y = if multiline {
                    let baseline = top - line.ascent * size;
                    top = baseline - line.descent * size;
                    baseline
                } else {
                    (height + (line.descent - line.ascent) * size) / 2.0
                };
                [x, y]
            })
            .collect()
    });
    let (drawn, fonts) = text::draw_positioned(&laid.lines, size, &positions, None).ok()?;
    let mut content = frame.into_bytes();
    content.extend(
        format!(
            "/Tx BMC\nq\n{} {} {} {} re W n\n{}\n",
            n(border),
            n(border),
            n((width - 2.0 * border).max(0.0)),
            n((height - 2.0 * border).max(0.0)),
            da.color
        )
        .bytes(),
    );
    content.extend(drawn);
    content.extend_from_slice(b"Q\nEMC\n");
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    let mut dict = Dict::new();
    dict.set(b"Type".to_vec(), Object::name("XObject"));
    dict.set(b"Subtype".to_vec(), Object::name("Form"));
    set_form_box(&mut dict, place);
    dict.set(b"Resources".to_vec(), Object::Dict(resources));
    Some((Stream::flate(dict, &content), text::first_undrawable(shown), laid.overflow))
}

#[cfg(test)]
mod comb_tests {
    /// A row cap at the first auto-size candidate must not suppress a smaller supported layout.
    #[test]
    fn review5914_arabic_autosize_retries_after_row_budget() {
        if pdfcraft_fonts::document_arabic_font().is_none() {
            assert!(std::env::var_os("CRAFT_FONTS_DIR").is_none(), "supplied input must include Arabic");
            return;
        }
        let value = "ب ".repeat(1100);
        let latin = pdfcraft_fonts::Std14::Helvetica;
        assert!(pdfcraft_fonts::appearance_text::wrap_with_latin_bounded(&value, 12.0, 12.0, latin, 1024).is_err());
        assert!(pdfcraft_fonts::appearance_text::wrap_with_latin_bounded(&value, 4.0, 12.0, latin, 1024).is_ok());
        let laid = super::layout(&value, None, 12.0, 20_000.0, latin, true).expect("a smaller bounded candidate must be tried");
        assert!(laid.size < 12.0 && laid.size >= 4.0);
        assert!(laid.lines.len() <= 1024);
        assert!(laid.overflow.is_none());
        assert!(super::layout(&value, Some(12.0), 12.0, 20_000.0, latin, true).is_none(), "fixed sizes keep the row cap");
    }

    /// Comb placement centres complete glyph/mark ink within each visual cell, rather
    /// than centring advances that can leave bearings or marks outside the cell.
    #[test]
    fn arabic_comb_centers_complete_ink_bounds_in_visual_cells() {
        if pdfcraft_fonts::document_arabic_font().is_none() {
            assert!(std::env::var_os("CRAFT_FONTS_REQUIRED").is_none(), "Arabic font required");
            return;
        }
        let (layout, positions) = super::comb_layout("لَا12", None, 120.0, 18.0, 6, pdfcraft_fonts::Std14::Helvetica).unwrap();
        assert!(layout.overflow.is_none());
        for ((line, point), slot) in layout.lines.iter().zip(positions).zip([5, 4, 2, 3]) {
            let bounds = line.ink_bounds(layout.size).unwrap();
            let (left, right) = (point[0] + bounds[0], point[0] + bounds[2]);
            assert!(((left + right) / 2.0 - (slot as f64 + 0.5) * 20.0).abs() < 0.000001);
            assert!(left >= slot as f64 * 20.0 - 0.000001 && right <= (slot + 1) as f64 * 20.0 + 0.000001);
            assert!(point[1] + bounds[1] >= -0.000001 && point[1] + bounds[3] <= 18.000001);
        }
    }
}
