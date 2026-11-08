//! Make text in ordinary Form XObjects available to the existing paragraph editor.
//!
//! This replaces each supported placement's `Do`, never the shared form object. The same
//! source glyph operators become page content; no cover rectangle or replacement text box is
//! added. Unsupported compositing and malformed streams stay intact.

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, Object};

use super::{ContentStream, Read, content_streams, joined, page_dict, rewritten_contents, splice};
use crate::{EditError, ungroup};

/// Result of preparing one page. Refused groups remain in the page, with an actionable reason.
#[derive(Clone, Debug, Default)]
pub struct Preparation {
    pub expanded: usize,
    pub refused: Vec<String>,
    pub repaired_fonts: usize,
}

/// How many levels of nested groups preparation opens on one page. Text nested deeper stays
/// grouped and is reported in [`Preparation::refused`].
const MAX_LEVELS: usize = 8;

/// What one pass of [`prepare`] inlined, as positions in the next pass's parse of the page.
struct Inlined {
    /// How many operators that parse has; with any other count the positions are wrong.
    ops: usize,
    /// Where each inlined form's operators are: the placements in them are new.
    ranges: Vec<std::ops::Range<usize>>,
    /// Earlier placements refused only because their resources needed renaming
    /// ([`crate::ungroup::Refusal::renaming`]), each with the reason it was given.
    waiting: Vec<(usize, String)>,
}

/// Prepare grouped source text atomically, within eight levels and 64 placements per page, and
/// repair the page fonts' damaged legacy Thai maps (`repair_thai_fonts`), grouped or not.
/// Shared forms and other pages are untouched. Preparation is a distinct undoable engine edit.
pub fn prepare(doc: &mut Document, page: usize) -> Result<Preparation, EditError> {
    let mut next = doc.clone();
    let mut result = Preparation::default();
    let mut bytes_left = 16usize << 20;
    let mut visits = 0usize;
    let mut decode = pdfcraft_cos::DecodeBudget::new(16 << 20, 0);
    // Forms found to be artwork, or refused for a reason in the form itself (`Refusal::intrinsic`):
    // a placement of one that an expanded group adds is left as it is without being read or
    // counted, so logos drawn inside many text groups don't use up the limit. A form refused only
    // where it is drawn is read again where an expanded group draws it.
    let mut settled = std::collections::HashSet::new();
    // What the previous pass inlined. Only the placements inside it are new. Every other placement
    // was read and counted on an earlier pass, so a group refused where it is drawn counts once.
    // Reading one again gives the same answer (an inlined form restores the graphics state it
    // changes, and the decode budget only shrinks), except for a group refused because its
    // resources needed renaming: names merged in for other groups since can make that
    // unnecessary. Such a group is read again, uncounted, once its resource names say so.
    let mut inlined: Option<Inlined> = None;
    // One pass opens one level of every placement; the last pass only reports what is deeper.
    for pass in 0..=MAX_LEVELS {
        let p = page_dict(&next, page)?;
        let streams = strict_streams(&next, &p.dict)?;
        let pieces = joined(&streams);
        let parsed = pieces.parse();
        if parsed.skipped > 0 {
            return Err(EditError::Invalid("the page has unreadable content; grouped text was left unchanged".into()));
        }
        let ops = parsed.ops;
        // `None` reads every placement: on the first pass, and if the page didn't parse back to
        // the operators written, where the positions can't be trusted. Reading one again can only
        // count it twice; skipping one could leave its text grouped without a reason.
        let (new, mut waiting) = match inlined.take() {
            Some(i) if i.ops == ops.len() => (Some(i.ranges), i.waiting),
            _ => (None, Vec::new()),
        };
        let mut still_waiting = Vec::new();
        let resources = p.dict.get(b"Resources").map(|r| next.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
        let space = ungroup::page_blend_space(&next, &p.dict);
        let images: std::collections::BTreeMap<_, _> =
            crate::page_images(&next, page)?.into_iter().filter(|i| i.is_form).map(|i| (i.op, i)).collect();
        let mut replacements = std::collections::BTreeMap::new();
        let mut merged = resources;
        let (mut refused_here, mut expanded_here) = (std::collections::HashSet::new(), std::collections::HashSet::new());
        for (at, op) in ops.iter().enumerate() {
            let Some(image) = images.get(&at) else { continue };
            if image.object.is_some_and(|o| settled.contains(&o)) {
                continue;
            }
            // An earlier placement is read again only when it was refused for a rename that the
            // names merged since have made unnecessary. The last pass opens nothing, so there it
            // keeps its reason.
            let earlier = new.as_ref().is_some_and(|ranges| !ranges.iter().any(|r| r.contains(&at)));
            if earlier {
                let Some(w) = waiting.iter().position(|(i, _)| *i == at) else { continue };
                let (_, reason) = waiting.swap_remove(w);
                // The name exactly as the `Do` writes it: a name need not be UTF-8, and
                // `image.name` is only its display form.
                if pass == MAX_LEVELS || !op.last_name().is_some_and(|name| ungroup::merges_without_renaming(&next, &merged, name)) {
                    still_waiting.push((at, reason));
                    continue;
                }
                // What reading it again finds replaces the reason it was given.
                if let Some(i) = result.refused.iter().position(|r| *r == reason) {
                    result.refused.remove(i);
                }
            }
            if !earlier {
                visits = visits.saturating_add(1);
                if visits > 64 {
                    return Err(EditError::Invalid("too many grouped text placements on this page (limit 64); the page was left unchanged".into()));
                }
            }
            match ungroup::ungroup_ops(&next, &merged, &ops, at, space.as_ref(), Matrix(image.matrix), &mut decode) {
                Ok(ungroup::Ungrouped { ops: inline, resources: mut res, patterns, shows_text }) => {
                    // Vector and image artwork needs no preparation. Nested text groups are
                    // visited on the next pass, with their inherited placement and clip in force.
                    match shows_text {
                        Some(true) => {}
                        Some(false) => {
                            settled.extend(image.object);
                            continue;
                        }
                        // Whether it has text can't be known: it is left as it is, and said.
                        None => {
                            result.refused.push(format!("{}: it draws recursive forms, or nested content that can't be checked", image.name));
                            continue;
                        }
                    }
                    if pass == MAX_LEVELS {
                        result.refused.push(format!("{}: its text is grouped more than {MAX_LEVELS} levels deep", image.name));
                        continue;
                    }
                    let data_len = pdfcraft_content::serialize_ops(&inline).len();
                    bytes_left = bytes_left.checked_sub(data_len).ok_or_else(|| {
                        EditError::Invalid("grouped page content exceeds the 16 MiB editing budget; the page was left unchanged".into())
                    })?;
                    if !patterns.is_empty() {
                        let mut table = res.get(b"Pattern").map(|r| next.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
                        for (name, value) in patterns {
                            table.set(name, Object::Ref(next.add(value)));
                        }
                        res.set(b"Pattern".to_vec(), Object::Dict(table));
                    }
                    merged = res;
                    replacements.insert(at, inline);
                    result.expanded += 1;
                    expanded_here.extend(image.object);
                }
                Err(refusal) => {
                    let reason = format!("{}: {}", image.name, refusal.error);
                    if refusal.renaming {
                        still_waiting.push((at, reason.clone()));
                    }
                    if refusal.intrinsic {
                        refused_here.extend(image.object);
                    }
                    result.refused.push(reason);
                }
            }
        }
        settled.extend(refused_here.difference(&expanded_here).copied());
        if replacements.is_empty() {
            break;
        }
        // Where an operator lands in the next pass: each replaced `Do` before it becomes its
        // inlined operators.
        let moved = |i: usize| replacements.range(..i).fold(i, |n, (_, inline)| n.saturating_add(inline.len()).saturating_sub(1));
        inlined = Some(Inlined {
            ops: moved(ops.len()),
            ranges: replacements.iter().map(|(at, inline)| moved(*at)..moved(*at).saturating_add(inline.len())).collect(),
            waiting: still_waiting.into_iter().map(|(at, reason)| (moved(at), reason)).collect(),
        });
        let changed = splice(&pieces, &ops, |at| match replacements.remove(&at) {
            Some(ops) => (ops, false),
            None => (Vec::new(), true),
        });
        let contents = rewritten_contents(&mut next, &streams, changed)?;
        next.update_dict(p.obj, |d| {
            d.set(b"Contents".to_vec(), contents);
            d.set(b"Resources".to_vec(), Object::Dict(merged));
        })?;
    }
    result.refused.sort();
    result.refused.dedup();
    // The page's own damaged maps are repaired too, those of text that was never grouped
    // included: its text reads wrong all the same.
    result.repaired_fonts = repair_thai_fonts(&mut next, page)?;
    if result.expanded > 0 || result.repaired_fonts > 0 {
        for reason in &result.refused {
            next.note_repair(format!("Grouped text left intact: {reason}"));
        }
        if result.expanded > 0 {
            next.note_repair(format!(
                "Prepared {} grouped text placement(s) on page {} for direct source-text editing",
                result.expanded,
                page.saturating_add(1)
            ));
        }
        *doc = next;
    }
    Ok(result)
}

/// A font is copied into this page: repairing the map must never alter a shared form/font.
fn repair_thai_fonts(doc: &mut Document, page: usize) -> Result<usize, EditError> {
    let p = page_dict(doc, page)?;
    let mut res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut fonts = res.get(b"Font").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut count = 0;
    let mut candidates = 0usize;
    for (name, value) in fonts.clone().iter() {
        let Some(mut font) = doc.resolve(value).as_dict().cloned() else { continue };
        if font.name(b"BaseFont").is_some_and(|n| String::from_utf8_lossy(n).contains("THSarabunPSK")) {
            candidates += 1;
            if candidates > 64 {
                return Err(EditError::Invalid("too many legacy Thai font maps to repair safely; the page was left unchanged".into()));
            }
        } else {
            continue;
        }
        let Some(map) = pdfcraft_fonts::pdf::recovered_thai_map(doc, &font) else { continue };
        // Append corrective bfchar entries after the original entries so valid Latin mappings
        // remain. The original map's accidental oversized range is still overridden for Thai.
        let Some(Object::Stream(tu)) = font.get(b"ToUnicode").map(|r| doc.resolve(r)).as_deref().cloned() else { continue };
        let old = pdfcraft_cos::DecodeBudget::new(1 << 20, 0).decode_strict(doc, &tu)?;
        // After the last `endcmap`: a stream with more than one must not have an earlier block
        // (or its oversized range) after the corrective entries.
        let marker = b"endcmap";
        let Some(at) = old.windows(marker.len()).rposition(|w| w == marker) else { continue };
        let mut added = format!("\n{} beginbfchar\n", map.len());
        for (code, ch) in map {
            added.push_str(&format!("<{code:04x}> <{:04x}>\n", u32::from(ch)));
        }
        added.push_str("endbfchar\n");
        let mut bytes = old.get(..at).unwrap_or_default().to_vec();
        bytes.extend_from_slice(added.as_bytes());
        bytes.extend_from_slice(old.get(at..).unwrap_or_default());
        let tu = doc.add(Object::Stream(pdfcraft_cos::Stream::flate(Dict::new(), &bytes)));
        font.set(b"ToUnicode".to_vec(), Object::Ref(tu));
        let object = doc.add(Object::Dict(font));
        fonts.set(name.clone(), Object::Ref(object));
        count += 1;
    }
    if count > 0 {
        res.set(b"Font".to_vec(), Object::Dict(fonts));
        doc.update_dict(p.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
        doc.note_repair(format!(
            "Recovered the damaged legacy THSarabunPSK Thai character map in {count} page font(s); original glyph programs were preserved"
        ));
    }
    Ok(count)
}

/// Editing cannot infer the state after a damaged page stream. Every piece must decode whole
/// before a placement is expanded, rather than treating an undecodable piece as empty.
fn strict_streams(doc: &Document, page: &Dict) -> Result<Vec<ContentStream>, EditError> {
    let streams = content_streams(doc, page)?;
    if streams.iter().any(|s| s.read != Read::Whole) {
        return Err(EditError::Invalid("the page has a missing or unreadable content stream".into()));
    }
    Ok(streams)
}
