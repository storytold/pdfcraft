//! Prepare bounded redaction appearances before the document is changed.

use std::borrow::Cow;

use super::{Mark, RedactError};

/// Shared by every selected page, including fills, text and PDF operators.
pub(super) const MAX_BYTES: usize = 8 * 1024 * 1024;

struct Output<'a> {
    bytes: Vec<u8>,
    remaining: &'a mut usize,
}

impl Output<'_> {
    fn push(&mut self, bytes: impl AsRef<[u8]>) -> Result<(), RedactError> {
        let bytes = bytes.as_ref();
        let left = self.remaining.checked_sub(bytes.len()).ok_or(RedactError::OverlayLimit)?;
        self.bytes.try_reserve(bytes.len()).map_err(|_| RedactError::OverlayLimit)?;
        self.bytes.extend_from_slice(bytes);
        *self.remaining = left;
        Ok(())
    }
}

fn repeat_line(text: &str, across: f64, down: f64, remaining: usize) -> Result<(String, usize), RedactError> {
    // Check floating-point counts before casting, and the product before allocating.
    if !across.is_finite() || !down.is_finite() || across > remaining as f64 || down > remaining as f64 {
        return Err(RedactError::OverlayLimit);
    }
    let (across, down) = (across as usize, down as usize);
    let bytes = text.len().checked_add(1).and_then(|n| n.checked_mul(across)).and_then(|n| n.checked_sub(1));
    let bytes = bytes.filter(|n| *n <= remaining).ok_or(RedactError::OverlayLimit)?;
    let mut line = String::new();
    line.try_reserve_exact(bytes).map_err(|_| RedactError::OverlayLimit)?;
    for i in 0..across {
        if i > 0 {
            line.push(' ');
        }
        line.push_str(text);
    }
    Ok((line, down))
}

pub(super) fn content(marks: &[&Mark], remaining: &mut usize) -> Result<Vec<u8>, RedactError> {
    let n = |v: f64| {
        let s = format!("{:.3}", v);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    let mut c = Output { bytes: Vec::new(), remaining };
    for m in marks {
        let Some(fill) = m.fill else { continue };
        if !fill.iter().all(|v| v.is_finite()) || m.rects.iter().flatten().any(|v| !v.is_finite()) {
            return Err(RedactError::OverlayLimit);
        }
        c.push(format!("q {} {} {} rg\n", n(fill[0]), n(fill[1]), n(fill[2])))?;
        for r in &m.rects {
            let (w, h) = (r[2] - r[0], r[3] - r[1]);
            if !w.is_finite() || !h.is_finite() {
                return Err(RedactError::OverlayLimit);
            }
            c.push(format!("{} {} {} {} re f\n", n(r[0]), n(r[1]), n(w), n(h)))?;
        }
        c.push(b"Q\n")?;
        if m.overlay.is_empty() {
            continue;
        }
        if m.overlay.len() > *c.remaining {
            return Err(RedactError::OverlayLimit);
        }
        let look = &m.look;
        if !look.size.is_finite() || !look.color.iter().all(|v| v.is_finite()) {
            return Err(RedactError::OverlayLimit);
        }
        // Keep upstream's standard-14 metrics when preparing the bounded appearance.
        let (res, face) = match look.font {
            pdfcraft_annot::OverlayFont::Helvetica => ("PCHelv", pdfcraft_fonts::Std14::Helvetica),
            pdfcraft_annot::OverlayFont::Times => ("PCTimes", pdfcraft_fonts::Std14::TimesRoman),
            pdfcraft_annot::OverlayFont::Courier => ("PCCour", pdfcraft_fonts::Std14::Courier),
        };
        let width = move |s: &str, size: f64| face.text_width(s, size);
        let [cr, cg, cb] = look.color.map(|v| v.clamp(0.0, 1.0));
        for r in &m.rects {
            let (w, h) = (r[2] - r[0], r[3] - r[1]);
            let mut size = if look.size > 0.0 { look.size } else { (h * 0.7).min(12.0) };
            let tw = width(&m.overlay, size);
            if look.size <= 0.0 && tw > w - 2.0 && tw > 0.0 {
                size *= (w - 2.0).max(0.0) / tw;
            }
            if size < 2.0 {
                continue;
            }
            let (line, count) = if look.repeat {
                let unit = width(&format!("{} ", m.overlay), size).max(0.01);
                let across = ((w - 2.0) / unit).floor().max(1.0);
                let down = (h / (size * 1.2)).floor().max(1.0);
                let (line, count) = repeat_line(&m.overlay, across, down, *c.remaining)?;
                (Cow::Owned(line), count)
            } else {
                (Cow::Borrowed(m.overlay.as_str()), 1)
            };
            // Every row is identical: measure and encode it once, without a Vec of copies.
            let encoded = pdfcraft_fonts::literal(&pdfcraft_fonts::win_ansi(&line));
            if encoded.len().checked_add(4).and_then(|n| n.checked_mul(count)).is_none_or(|n| n > *c.remaining) {
                return Err(RedactError::OverlayLimit);
            }
            let lw = width(&line, size);
            let x = match look.align {
                0 => r[0] + 1.0,
                2 => r[2] - 1.0 - lw,
                _ => r[0] + (w - lw) / 2.0,
            };
            let block = count as f64 * size * 1.2;
            let mut y = r[1] + (h + block) / 2.0 - size * 0.95;
            if !lw.is_finite() || !x.is_finite() || !y.is_finite() {
                return Err(RedactError::OverlayLimit);
            }
            c.push(format!("q {} {} {} {} re W n BT {} {} {} rg /{res} {} Tf ", n(r[0]), n(r[1]), n(w), n(h), n(cr), n(cg), n(cb), n(size)))?;
            for _ in 0..count {
                c.push(format!("1 0 0 1 {} {} Tm ", n(x), n(y)))?;
                c.push(&encoded)?;
                c.push(b" Tj ")?;
                y -= size * 1.2;
            }
            c.push(b"ET Q\n")?;
        }
    }
    Ok(c.bytes)
}
