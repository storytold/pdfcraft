//! The bundled Inter face fills only the gaps in the requested standard font. Keep its real
//! program and Unicode mapping in the PDF, so another machine needs no installed fallback font.

use pdfcraft_fonts::latin_text::{self, EmbeddedLatinText};
use pdfcraft_fonts::pdf::Metrics;

use super::types::AddedText;
use super::{Dict, Document, EditError, literal, n, win_ansi};

pub(crate) enum LatinFallback {
    Scalar { name: String, metrics: Box<Metrics> },
    Shaped(EmbeddedLatinText),
}

impl LatinFallback {
    pub(crate) fn prepare(doc: &mut Document, t: &AddedText, taken: &Dict, fonts: &mut Dict) -> Result<Option<Self>, EditError> {
        let needed = t.text.chars().any(|c| {
            !matches!(c, '\n' | '\r')
                && !pdfcraft_fonts::win_ansi_encodable(c)
                && !super::layout::is_arabic(c)
                && !super::layout::is_hebrew(c)
                && !super::layout::is_thai(c)
        });
        if !needed {
            return Ok(None);
        }
        let faces = pdfcraft_fonts::document_latin_fonts(t.bold);
        let face = faces.first().ok_or_else(|| EditError::Invalid("the bundled Latin document font is unavailable".into()))?;
        let (_, postscript) = pdfcraft_fonts::embedding::names(face.bytes).map_err(EditError::Invalid)?;
        // Include every existing name when embedding: a page may already have PCFull0, or an
        // earlier item may share the exact full face. Never overwrite a document font.
        let mut available = taken.clone();
        for (name, font) in fonts.iter() {
            available.set(name.clone(), font.clone());
        }
        if latin_text::required(&t.text) {
            if t.text.len() > latin_text::MAX_ROW_BYTES {
                return Err(EditError::Invalid("the NFD text item exceeds its text budget; use shorter text boxes".into()));
            }
            // Preflight every Latin piece before allocating any PDF objects. A later line's
            // missing glyph or excessive mark chain must not leave an earlier font behind.
            let mut planned = std::collections::HashSet::new();
            let plans = runs(&t.text)
                .into_iter()
                .filter(|(extended, run)| *extended && planned.insert(*run))
                .map(|(_, run)| latin_text::plan(run, t.bold).map_err(EditError::Invalid))
                .collect::<Result<Vec<_>, _>>()?;
            let shaped = latin_text::embed(doc, &mut available, &plans, t.bold).map_err(EditError::Invalid)?;
            // All resources added by the shared writer retain the page's existing names.
            for (name, object) in available.iter() {
                if !taken.contains(name) && !fonts.contains(name) {
                    fonts.set(name.clone(), object.clone());
                }
            }
            return Ok(Some(Self::Shaped(shaped)));
        }
        let name = pdfcraft_fonts::embedding::embed(doc, &mut available, face.bytes, &postscript).map_err(EditError::Invalid)?;
        let object = available.get(name.as_bytes()).ok_or_else(|| EditError::Invalid("the embedded Latin font is unavailable".into()))?;
        let dict = doc.dict(object).ok_or_else(|| EditError::Invalid("the embedded Latin font has no dictionary".into()))?;
        let metrics = Metrics::from_dict(doc, &dict);
        fonts.set(name.as_bytes().to_vec(), object.clone());
        Ok(Some(Self::Scalar { name, metrics: Box::new(metrics) }))
    }
}

/// Paint one Latin piece at an explicit baseline. Missing standard-font characters use Inter;
/// every other character keeps the selected face. Explicit positioning retains word spacing
/// across the font switches. A base and its marks always use one face and one spacing step.
pub(crate) fn draw_latin(
    out: &mut Vec<u8>,
    t: &AddedText,
    standard_name: &str,
    fallback: Option<&LatinFallback>,
    text: &str,
    at: [f64; 2],
    word_spacing: f64,
) -> Result<(), EditError> {
    let [mut pen, y] = at;
    for (extended, run) in runs(text) {
        if extended && let Some(LatinFallback::Shaped(shaped)) = fallback {
            let plan = latin_text::plan(run, t.bold).map_err(EditError::Invalid)?;
            let shear = if t.italic { 0.2125565616700221 } else { 0.0 };
            pen += shaped.write(out, &plan, t.size, [pen, y], shear, t.char_spacing, word_spacing).map_err(EditError::Invalid)?;
            continue;
        }
        let (name, bytes, width, shear) = if extended {
            let Some(LatinFallback::Scalar { name, metrics }) = fallback else {
                return Err(EditError::Invalid("the Latin text has no fallback font".into()));
            };
            let bytes = metrics.encode(run).ok_or_else(|| EditError::Invalid("the Latin fallback cannot encode the text".into()))?;
            let width = metrics.codes(&bytes).iter().map(|(code, _)| metrics.width(*code) * t.size).sum::<f64>();
            // Inter has a real SemiBold face, but no bundled italic face. Match the standard
            // oblique text's 12-degree slant without copying a proprietary system font.
            let shear = if t.italic { 0.2125565616700221 } else { 0.0 };
            (name.as_str(), bytes, width, shear)
        } else {
            (standard_name, win_ansi(run), t.family.width(run, t.size, t.bold, t.italic), 0.0)
        };
        if latin_text::required(&t.text) {
            // A positioned NFD cluster resets spacing/rise. Restate them before an
            // ordinary run while leaving legacy non-NFD drawing bytes unchanged.
            out.extend(
                format!("/{name} {} Tf {} Tc {} Tw 0 Ts 1 0 {} 1 {} {} Tm ", n(t.size), n(t.char_spacing), n(word_spacing), n(shear), n(pen), n(y))
                    .bytes(),
            );
        } else {
            out.extend(format!("/{name} {} Tf {} Tw 1 0 {} 1 {} {} Tm ", n(t.size), n(word_spacing), n(shear), n(pen), n(y)).bytes());
        }
        out.extend(literal(&bytes));
        out.extend_from_slice(b" Tj\n");
        pen += width + t.char_spacing * run.chars().count() as f64 + word_spacing * run.matches(' ').count() as f64;
    }
    Ok(())
}

/// Contiguous standard/fallback runs, splitting only between complete base/mark clusters.
/// Script pieces are drawn elsewhere; filtering them here also keeps their font out of a
/// mixed-script item's Latin preflight.
fn runs(text: &str) -> Vec<(bool, &str)> {
    let mut runs = Vec::new();
    let (mut start, mut end, mut kind) = (0usize, 0usize, None);
    for cluster in latin_text::clusters(text) {
        let extended = cluster.chars().any(|c| !matches!(c, '\n' | '\r') && !pdfcraft_fonts::win_ansi_encodable(c))
            && !cluster.chars().any(|c| super::layout::is_arabic(c) || super::layout::is_hebrew(c) || super::layout::is_thai(c));
        if kind.is_some_and(|kind| kind != extended) {
            if let Some(run) = text.get(start..end) {
                runs.push((kind.unwrap_or(false), run));
            }
            start = end;
        }
        kind = Some(extended);
        end = end.saturating_add(cluster.len());
    }
    if let (Some(kind), Some(run)) = (kind, text.get(start..end)) {
        runs.push((kind, run));
    }
    runs
}

/// Measure the exact mixed runs the writer uses. Legacy NFC/ASCII text keeps its old metrics;
/// only a marked cluster uses the shaped Inter advance and counts as one spacing step.
pub(super) fn width(t: &AddedText, text: &str) -> f64 {
    if !latin_text::required(text) {
        return pdfcraft_fonts::standard_with_latin_fallback_width(text, t.size, t.family.std14(t.bold, t.italic), t.bold)
            + t.char_spacing * text.chars().count() as f64;
    }
    runs(text)
        .into_iter()
        .map(|(extended, run)| {
            if extended {
                latin_text::plan(run, t.bold).map_or_else(
                    |_| pdfcraft_fonts::standard_with_latin_fallback_width(run, t.size, t.family.std14(t.bold, t.italic), t.bold),
                    |plan| plan.advance * t.size,
                ) + t.char_spacing * latin_text::clusters(run).count() as f64
            } else {
                t.family.width(run, t.size, t.bold, t.italic) + t.char_spacing * run.chars().count() as f64
            }
        })
        .sum()
}

/// Added-page rows split only at LF. A lone CR would otherwise be drawn as `?`, and must
/// retain the old refusal even though the shared form planner accepts it as a line separator.
pub(crate) fn first_unsupported(text: &str, bold: bool) -> Option<char> {
    if !latin_text::required(text) {
        return pdfcraft_fonts::first_non_win_ansi_or_latin(text, bold);
    }
    let (row, carriage_return) = text.split_once('\r').map_or((text, false), |(row, _)| (row, true));
    latin_text::first_unsupported(row, bold).or_else(|| carriage_return.then_some('\r'))
}
