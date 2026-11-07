//! Make text in ordinary Form XObjects available to the existing paragraph editor.
//!
//! This replaces each supported placement's `Do`, never the shared form object. The same
//! source glyph operators become page content; no cover rectangle or replacement text box is
//! added. Unsupported compositing and malformed streams stay intact.

mod expand;

use pdfcraft_content::{Matrix, Pieces};
use pdfcraft_cos::{Dict, Document, Object};

use super::{page_dict, rewritten_contents, splice};
use crate::EditError;

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

/// Prepare grouped source text atomically, within eight levels and 64 placements per page.
/// Shared forms and other pages are untouched. Preparation is a distinct undoable engine edit.
pub fn prepare(doc: &mut Document, page: usize) -> Result<Preparation, EditError> {
    let mut next = doc.clone();
    let mut result = Preparation::default();
    let mut bytes_left = 16usize << 20;
    let mut visits = 0usize;
    let mut decode = pdfcraft_cos::DecodeBudget::new(16 << 20, 0);
    // Forms found to be artwork, or refused for a reason in the form itself (`Refusal::intrinsic`):
    // a later pass leaves their placements as they are without reading or counting them again.
    // Only placements that are read count against the limit, so a page of logos doesn't use it
    // up pass after pass. A form refused only where it is drawn is read again where it appears.
    let mut settled = std::collections::HashSet::new();
    // One pass opens one level of every placement; the last pass only reports what is deeper.
    for pass in 0..=MAX_LEVELS {
        let p = page_dict(&next, page)?;
        let streams = strict_streams(&next, &p.dict)?;
        let pieces = Pieces::join(&streams.iter().map(|(_, b)| b.as_slice()).collect::<Vec<_>>());
        let parsed = pieces.parse();
        if parsed.skipped > 0 {
            return Err(EditError::Invalid("the page has unreadable content; grouped text was left unchanged".into()));
        }
        let ops = parsed.ops;
        let resources = p.dict.get(b"Resources").map(|r| next.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
        let space = expand::page_blend_space(&next, &p.dict);
        let images: std::collections::BTreeMap<_, _> =
            crate::page_images(&next, page)?.into_iter().filter(|i| i.is_form).map(|i| (i.op, i)).collect();
        let mut replacements = std::collections::HashMap::new();
        let mut merged = resources;
        let mut pending_path = false;
        let mut in_text = false;
        let (mut refused_here, mut expanded_here) = (std::collections::HashSet::new(), std::collections::HashSet::new());
        for (at, op) in ops.iter().enumerate() {
            match op.op.as_slice() {
                b"m" | b"l" | b"c" | b"v" | b"y" | b"re" | b"h" | b"W" | b"W*" => pending_path = true,
                b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => pending_path = false,
                b"BT" => in_text = true,
                b"ET" => in_text = false,
                _ => {}
            }
            let Some(image) = images.get(&at) else { continue };
            if image.object.is_some_and(|o| settled.contains(&o)) {
                continue;
            }
            if pending_path || in_text {
                result.refused.push(format!("{}: the group is drawn inside an unfinished path or text object", image.name));
                continue;
            }
            // The renderer draws a `Do`'s last operand; `image` was found by its first. With more
            // than one they can be different forms, so this placement is left as it is.
            if op.operands.len() != 1 {
                result.refused.push(format!("{}: the group is drawn with an operand too many", image.name));
                continue;
            }
            visits = visits.saturating_add(1);
            if visits > 64 {
                return Err(EditError::Invalid("too many grouped text placements on this page (limit 64); the page was left unchanged".into()));
            }
            let translucent = expand::translucent_at(&next, &merged, &ops, at);
            // The resource name exactly as the `Do` writes it (its only operand): a name need not
            // be UTF-8, and `image.name` is only its display form.
            let name = op.last_name().map_or_else(|| image.name.as_bytes().to_vec(), <[u8]>::to_vec);
            match expand::ungroup_ops(&next, &merged, &name, translucent, space.as_ref(), Matrix(image.matrix), &mut decode) {
                Ok(expand::Ungrouped { ops: inline, resources: mut res, patterns, shows_text }) => {
                    // Vector and image artwork needs no preparation. Nested text groups are
                    // visited on the next pass, with their inherited placement and clip in force.
                    if !shows_text {
                        settled.extend(image.object);
                        continue;
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
                    result.refused.push(format!("{}: {}", image.name, refusal.error));
                    if refusal.intrinsic {
                        refused_here.extend(image.object);
                    }
                }
            }
        }
        settled.extend(refused_here.difference(&expanded_here).copied());
        if replacements.is_empty() {
            break;
        }
        let changed = splice(&pieces, &ops, |at| match replacements.remove(&at) {
            Some(ops) => (ops, false),
            None => (Vec::new(), true),
        });
        let contents = rewritten_contents(&mut next, &streams, changed);
        next.update_dict(p.obj, |d| {
            d.set(b"Contents".to_vec(), contents);
            d.set(b"Resources".to_vec(), Object::Dict(merged));
        })?;
    }
    result.refused.sort();
    result.refused.dedup();
    if result.expanded > 0 {
        for reason in &result.refused {
            next.note_repair(format!("Grouped text left intact: {reason}"));
        }
        result.repaired_fonts = repair_thai_fonts(&mut next, page)?;
        next.note_repair(format!(
            "Prepared {} grouped text placement(s) on page {} for direct source-text editing",
            result.expanded,
            page.saturating_add(1)
        ));
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
        let old = pdfcraft_cos::DecodeBudget::new(1 << 20, 0).decode_strict(&tu)?;
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

/// Editing cannot infer the state after a damaged page stream. Read all pieces strictly before
/// expanding a placement, rather than treating an undecodable piece as empty.
fn strict_streams(doc: &Document, page: &Dict) -> Result<Vec<(Object, Vec<u8>)>, EditError> {
    let mut budget = pdfcraft_cos::DecodeBudget::new(16 << 20, 0);
    let list = match page.get(b"Contents") {
        None => Vec::new(),
        Some(value) => match doc.resolve(value).as_ref() {
            Object::Null => Vec::new(),
            Object::Array(a) if a.len() <= 4096 => a.clone(),
            Object::Array(_) => return Err(EditError::Invalid("too many page content streams to prepare safely".into())),
            _ => vec![value.clone()],
        },
    };
    list.into_iter()
        .map(|value| {
            let resolved = doc.resolve(&value);
            let Object::Stream(stream) = resolved.as_ref() else {
                return Err(EditError::Invalid("the page has a missing or unreadable content stream".into()));
            };
            let bytes = budget.decode_strict(stream)?;
            Ok((value, bytes))
        })
        .collect()
}
