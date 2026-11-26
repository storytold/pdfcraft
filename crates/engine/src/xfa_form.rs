//! A document's XFA form: laying it out, keeping its datasets and fields in step, and taking it out.

use crate::edit::EditCtx;
use crate::{Document, EditError, FieldValue, XfaLayout, guard, xfa};

/// Lay a dynamic XFA form out (pages and fields) and give its widgets appearances.
pub(crate) fn xfa_layout(doc: &mut pdfcraft_cos::Document) -> Result<XfaLayout, String> {
    let mut report = pdfcraft_xfa::render_into(doc).map_err(|e| e.to_string())?;
    xfa_redraw(doc)?;
    // The form's own values: one its field's font can't draw is drawn with `?` and reported.
    report.warnings.extend(undrawn_field_warnings(doc));
    Ok(report)
}

/// Give the fields of a laid-out XFA form their appearances. A signed signature box keeps the
/// picture its signer's application drew.
pub(crate) fn xfa_redraw(doc: &mut pdfcraft_cos::Document) -> Result<(), String> {
    let signed = |doc: &pdfcraft_cos::Document, f: &pdfcraft_forms::Field| {
        f.kind == pdfcraft_forms::FieldKind::Signature && doc.get(f.obj).as_dict().is_some_and(|d| d.contains(b"V"))
    };
    pdfcraft_forms::redraw_fields(doc, |doc, f| !signed(doc, f)).map_err(|e| e.to_string())
}

/// Apply comb settings from a static XFA template to existing text fields. The live form tree
/// includes hidden and unplaced fields, which have no page item to look up.
pub(crate) fn xfa_apply_static_comb_fields(doc: &mut pdfcraft_cos::Document) -> Result<Vec<String>, String> {
    let Some(packets) = pdfcraft_xfa::read_packets(doc).map_err(|e| e.to_string())? else { return Ok(Vec::new()) };
    if packets.needs_rendering || !packets.has_fields || !packets.xdp.contains("comb") {
        return Ok(Vec::new());
    }
    // Converting existing widgets would modify the field flags and appearances covered by a signature.
    if pdfcraft_sign::has_signatures(doc) {
        return Ok(Vec::new());
    }
    let tpl = pdfcraft_xfa::template_of(doc).map_err(|e| e.to_string())?;
    let data = pdfcraft_xfa::data_of(doc);
    let uses = pdfcraft_xfa::page_area_uses(doc, &tpl);
    let live = pdfcraft_xfa::form_tree_paged(&tpl, data.as_ref(), &pdfcraft_xfa::overrides(doc), &uses);
    if live.truncated {
        return Err("the static XFA form tree exceeded its limit while reading comb fields".into());
    }
    let mut combs = std::collections::HashMap::new();
    collect_static_comb_fields(&live.root, &mut combs);

    let mut changed = Vec::new();
    let mut changed_objects = std::collections::HashSet::new();
    for field in pdfcraft_forms::fields(doc) {
        if field.kind != pdfcraft_forms::FieldKind::Text || field.has(pdfcraft_forms::flags::PASSWORD) {
            continue;
        }
        let som = pdfcraft_xfa::data::field_som(doc, field.obj, &field.name);
        let Some((cells, border)) = combs.get(&som).copied() else { continue };
        let max_len = cells.min(100_000);
        if max_len == 0 {
            continue;
        }
        let flags = (field.flags | pdfcraft_forms::flags::COMB)
            & !(pdfcraft_forms::flags::MULTILINE | pdfcraft_forms::flags::DO_NOT_SCROLL | pdfcraft_forms::flags::RICH_TEXT);
        let mut field_changed = flags != field.flags || field.max_len != Some(max_len);
        if field_changed {
            doc.update_dict(field.obj, |d| {
                d.set(b"Ff".to_vec(), pdfcraft_cos::Object::Int(i64::from(flags)));
                d.set(b"MaxLen".to_vec(), pdfcraft_cos::Object::Int(max_len as i64));
            })
            .map_err(|e| e.to_string())?;
        }
        for widget in &field.widgets {
            if comb_border_matches(doc, widget.obj, border) {
                continue;
            }
            doc.update_dict(widget.obj, |d| match border {
                Some(edge) => {
                    let mut comb = pdfcraft_cos::Dict::new();
                    comb.set(b"C".to_vec(), pdfcraft_cos::Object::Array(edge.color.0.iter().map(|v| pdfcraft_cos::Object::Real(*v)).collect()));
                    comb.set(b"W".to_vec(), pdfcraft_cos::Object::Real(edge.thickness));
                    d.set(pdfcraft_xfa::COMB_BORDER_KEY.to_vec(), pdfcraft_cos::Object::Dict(comb));
                }
                None => {
                    d.remove(pdfcraft_xfa::COMB_BORDER_KEY);
                }
            })
            .map_err(|e| e.to_string())?;
            field_changed = true;
        }
        if field_changed {
            changed_objects.insert(field.obj);
            changed.push(field.name);
        }
    }
    if !changed_objects.is_empty() {
        pdfcraft_forms::redraw_fields(doc, |_, field| changed_objects.contains(&field.obj)).map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

fn collect_static_comb_fields(
    node: &pdfcraft_xfa::FormNode,
    combs: &mut std::collections::HashMap<String, (usize, Option<pdfcraft_xfa::model::CombBorder>)>,
) {
    if node.kind == Some(pdfcraft_xfa::NodeKind::Field)
        && let Some(cells) = node.comb_cells
    {
        combs.insert(node.som.clone(), (cells, node.comb_border));
    }
    for child in &node.children {
        collect_static_comb_fields(child, combs);
    }
}

fn comb_border_matches(doc: &pdfcraft_cos::Document, widget: pdfcraft_cos::ObjRef, expected: Option<pdfcraft_xfa::model::CombBorder>) -> bool {
    let object = doc.get(widget);
    let Some(dict) = object.as_dict() else { return false };
    let Some(expected) = expected else { return !dict.contains(pdfcraft_xfa::COMB_BORDER_KEY) };
    let Some(comb) = dict.get(pdfcraft_xfa::COMB_BORDER_KEY).map(|v| doc.resolve(v)) else { return false };
    let Some(comb) = comb.as_dict() else { return false };
    let Some(color) = comb.get(b"C").map(|v| doc.resolve(v)) else { return false };
    let Some(color) = color.as_array() else { return false };
    if color.len() != 3 {
        return false;
    }
    let correct_color = color
        .iter()
        .zip(expected.color.0)
        .all(|(actual, wanted)| doc.resolve(actual).as_f64().is_some_and(|actual| (actual - wanted).abs() < 0.000_001));
    let correct_width =
        comb.get(b"W").map(|v| doc.resolve(v)).and_then(|v| v.as_f64()).is_some_and(|actual| (actual - expected.thickness).abs() < 0.000_001);
    correct_color && correct_width
}

/// A warning for each field whose appearance PdfCraft drew with `?` for a character its font
/// can't draw (laying out an XFA form's own values).
pub(crate) fn undrawn_field_warnings(doc: &pdfcraft_cos::Document) -> Vec<String> {
    pdfcraft_forms::undrawn_fields(doc)
        .into_iter()
        .map(|(name, c)| {
            format!("form field {name:?} holds \"{}\" (U+{:04X}), which its font can't draw: it shows ? in its place", c.escape_debug(), u32::from(c))
        })
        .collect()
}

/// The form's fields as the XFA data layer wants them.
fn xfa_field_data(doc: &pdfcraft_cos::Document) -> Vec<pdfcraft_xfa::FieldDatum> {
    use pdfcraft_forms::FieldKind as K;
    pdfcraft_forms::fields(doc)
        .into_iter()
        .map(|f| {
            let data = match f.kind {
                K::Text | K::Combo | K::List => pdfcraft_xfa::FieldData::Text(f.value.join("\n")),
                K::CheckBox => pdfcraft_xfa::FieldData::Check(!f.value.is_empty()),
                K::Radio => pdfcraft_xfa::FieldData::Radio(f.value.first().cloned()),
                K::PushButton | K::Signature => pdfcraft_xfa::FieldData::None,
            };
            pdfcraft_xfa::FieldDatum { obj: f.obj, name: f.name, data }
        })
        .collect()
}

/// Keep the XFA datasets packet in step with the fields after an edit. Returns what could not
/// be written.
/// `datasets`: the stream this edit already wrote, replaced in place rather than added again
/// (and set to the one written).
/// `tpl`: the form's parsed template when the document has one cached (else it is parsed).
pub(crate) fn xfa_sync_datasets(
    doc: &mut pdfcraft_cos::Document,
    tpl: Option<&pdfcraft_xfa::model::Template>,
    datasets: &mut Option<pdfcraft_cos::ObjRef>,
) -> Result<Vec<String>, String> {
    let data = xfa_field_data(doc);
    let r = pdfcraft_xfa::write_datasets_with(doc, tpl, &data, *datasets).map_err(|e| e.to_string())?;
    if r.stream.is_some() {
        *datasets = r.stream;
    }
    Ok(r.warnings)
}

/// What a document whose form script ran away is told.
pub(crate) const XFA_SCRIPTS_OFF: &str =
    "A script of this form ran too long and was abandoned; the form's scripts are off for this document (reopen it to run them again)";

/// Most XFA warnings kept per document.
const MAX_XFA_WARNINGS: usize = 50;

/// Add `new` to `list` (no repeats, at most [`MAX_XFA_WARNINGS`]).
pub(crate) fn note_warnings(list: &mut Vec<String>, new: &[String]) {
    for w in new {
        if !list.contains(w) && list.len() < MAX_XFA_WARNINGS {
            list.push(w.clone());
        }
    }
}

/// A data file's text: UTF-8 or UTF-16 (by a byte order mark, or how `<` is written), or
/// Latin-1 when its XML declaration says so. `None`: not text at all (FDF); `Err`: an
/// encoding this can't read.
pub(crate) fn decode_data_file(bytes: &[u8]) -> Option<Result<String, String>> {
    if bytes.starts_with(b"%FDF") {
        return None;
    }
    let declared = |text: &str| -> Option<String> {
        let head = text.get(..text.len().min(256))?;
        let decl = head.strip_prefix('\u{feff}').unwrap_or(head).trim_start().strip_prefix("<?xml")?;
        let decl = decl.split_once("?>")?.0;
        let enc = decl.split_once("encoding=")?.1;
        let quote = enc.chars().next()?;
        Some(enc.get(1..)?.split(quote).next()?.trim().to_ascii_lowercase())
    };
    if let Some((text, _)) = pdfcraft_xfa::packets::decode(bytes) {
        match declared(&text).as_deref() {
            None | Some("utf-8" | "utf8" | "utf-16" | "utf-16le" | "utf-16be" | "us-ascii" | "ascii") => return Some(Ok(text)),
            Some("iso-8859-1" | "latin1" | "latin-1" | "windows-1252" | "cp1252") if !bytes.starts_with(&[0xEF, 0xBB, 0xBF]) => {
                // Each byte is the character it names (the 0x80–0x9F block of Windows-1252
                // differs, and is rare in form data).
                return Some(Ok(bytes.iter().map(|b| char::from(*b)).collect()));
            }
            Some(other) => return Some(Err(format!("the data file's encoding {other:?} is not supported (UTF-8, UTF-16 and ISO-8859-1 are)"))),
        }
    }
    // Not valid UTF-8/UTF-16: Latin-1 by declaration, else refused.
    let latin1: String = bytes.iter().map(|b| char::from(*b)).collect();
    match declared(&latin1).as_deref() {
        Some("iso-8859-1" | "latin1" | "latin-1" | "windows-1252" | "cp1252") => Some(Ok(latin1)),
        _ => Some(Err("the data file is not UTF-8, UTF-16 or ISO-8859-1 text".into())),
    }
}

/// The on state a radio group's XFA data `item` selects: the button saving writes `item` for
/// (`/PCOn`, else its `/Opt` entry, else its state; `pdfcraft_xfa::radio_state_for_item`), so a
/// saved selection reopens on the same button; else `item` read as an export value, then as an
/// on-state name ([`pdfcraft_forms::Field::state_for_export`]), as data from elsewhere is; else
/// `item` itself.
fn radio_button(doc: &pdfcraft_cos::Document, f: &pdfcraft_forms::Field, datum: Option<&pdfcraft_xfa::FieldDatum>, item: &str) -> String {
    datum
        .and_then(|d| pdfcraft_xfa::radio_state_for_item(doc, d, item))
        .or_else(|| f.state_for_export(item).map(str::to_owned))
        .unwrap_or_else(|| item.to_owned())
}

/// Take the XFA out of `doc` and note what went with it: the packets, data no field showed,
/// buttons that did nothing but run scripts, and fields whose value differs from the form's
/// data (data PdfCraft could not read into them, now lost).
pub(crate) fn strip_xfa(doc: &mut pdfcraft_cos::Document, cx: &mut EditCtx) -> Result<(), EditError> {
    let fields = pdfcraft_forms::fields(doc);
    let data = xfa_field_data(doc);
    let differ = pdfcraft_xfa::read_values(doc, &data)
        .into_iter()
        .filter(|(name, value)| {
            let Some(f) = fields.iter().find(|f| f.name == *name) else { return false };
            match value {
                pdfcraft_xfa::FieldData::Text(t) => f.value.join("\n") != *t && !t.trim().is_empty(),
                pdfcraft_xfa::FieldData::Check(on) => f.value.is_empty() == *on,
                // The field holds an on-state name; the data names a button by its item. They agree
                // when the data holds what saving writes (`/PCOn`, the `/Opt` entry or the state
                // name), or names the same button as opening reads it ([`radio_button`]).
                pdfcraft_xfa::FieldData::Radio(sel) => {
                    let state = f.value.first().map(String::as_str);
                    let datum = data.iter().find(|d| d.name == *name);
                    let same_button = sel.as_deref().map(|s| radio_button(doc, f, datum, s)).as_deref() == state;
                    let written = datum.map(|d| pdfcraft_xfa::radio_dataset_value(doc, d, &f.value.first().cloned()));
                    !same_button && written.as_deref() != Some(sel.as_deref().unwrap_or_default())
                }
                pdfcraft_xfa::FieldData::None => false,
            }
        })
        .count();
    let r = pdfcraft_xfa::strip(doc).map_err(|e| EditError::Write(e.to_string()))?;
    cx.xfa_removed.extend(r.notes());
    if differ > 0 {
        cx.xfa_removed.push(format!(
            "{differ} field(s) held a value other than the form's XFA data (data PdfCraft could not read into them); the data's version is gone"
        ));
    }
    Ok(())
}

/// Keep the document's XFA state in step with its file after an edit: a script may have laid
/// the form out again (rows added, subforms shown), an edit may have taken the XFA out
/// (flattened, or made static), or an undo put it back.
pub(crate) fn sync_xfa_state(doc: &mut Document) {
    let Some(cos) = doc.editor.as_ref().map(|e| &e.cos) else { return };
    match pdfcraft_xfa::existing_layout(cos) {
        Some(report) => {
            if doc.xfa.is_none() {
                // Back after an undo: the XFA's own warnings apply again, the strip's don't.
                doc.xfa_warnings.retain(|w| !w.starts_with(XFA_REMOVED_PREFIX));
                if doc.xfa_template.is_none() && !doc.xfa_scripts_off {
                    doc.xfa_template = xfa::template(cos);
                }
            }
            doc.xfa = Some(report);
        }
        None => {
            doc.xfa = None;
            doc.xfa_template = None;
            // Only what the strip said stays (the XFA's own warnings no longer apply).
            doc.xfa_warnings.retain(|w| w.starts_with(XFA_REMOVED_PREFIX));
        }
    }
}

/// How the warnings of a strip start, so they can be told from the XFA's own.
pub(crate) const XFA_REMOVED_PREFIX: &str = "XFA removed: ";

/// Is this an XFA data file (an XDP, a datasets document, a data element or a data document),
/// rather than XFDF, Acrobat's `<fields>` XML or a table? By its root element.
pub(crate) fn is_xfa_data_file(text: &str) -> bool {
    let text = text.get(..text.len().min(65_536)).unwrap_or(text);
    let mut rest = text.trim_start_matches('\u{feff}').trim_start();
    // Past the XML declaration, comments and processing instructions.
    let mut guard = 0;
    while guard < 100 {
        guard += 1;
        if let Some(r) = rest.strip_prefix("<?") {
            rest = r.split_once("?>").map_or("", |(_, r)| r).trim_start();
        } else if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.split_once("-->").map_or("", |(_, r)| r).trim_start();
        } else if let Some(r) = rest.strip_prefix("<!") {
            rest = r.split_once('>').map_or("", |(_, r)| r).trim_start();
        } else {
            break;
        }
    }
    let Some(tag) = rest.strip_prefix('<') else { return false };
    let name = tag.split(|c: char| c.is_whitespace() || c == '>' || c == '/').next().unwrap_or("");
    let local = name.rsplit(':').next().unwrap_or(name);
    !local.is_empty() && !matches!(local, "xfdf" | "fields" | "html" | "body")
}

/// The content type a picture is saved under, from its bytes: the kinds a laid-out form shows.
pub(crate) fn image_content_type(bytes: &[u8]) -> Option<&'static str> {
    match pdfcraft_xfa::image::kind(bytes)? {
        pdfcraft_xfa::image::Kind::Jpeg => Some("image/jpeg"),
        pdfcraft_xfa::image::Kind::Png => Some("image/png"),
        pdfcraft_xfa::image::Kind::Gif => Some("image/gif"),
        pdfcraft_xfa::image::Kind::Bmp => Some("image/bmp"),
        pdfcraft_xfa::image::Kind::Tiff => Some("image/tiff"),
    }
}

/// After the XFA data changed outside the fields (an import, a picture): a laid-out form is
/// laid out again from the data (then, when `calculate`, its calculations run), and the
/// fields take the data's values.
pub(crate) fn xfa_after_data_change(doc: &mut pdfcraft_cos::Document, cx: &mut EditCtx, calculate: bool) -> Result<(), EditError> {
    if let Some(tpl) = cx.xfa.clone() {
        let highlight = cx.highlight();
        let mut run = xfa::XfaRun {
            page_count: cx.pages,
            out: &mut cx.xfa_out,
            datasets: cx.xfa_datasets,
            ran_away: false,
            highlight,
            time_scale: cx.xfa_time_scale,
        };
        let r = guard(|| xfa::on_data_replaced(doc, &tpl, calculate, &mut run))
            .unwrap_or_else(|m| Err(format!("laying the form out again failed unexpectedly ({m})")));
        cx.xfa_datasets = run.datasets;
        cx.xfa_ran_away |= run.ran_away;
        // The next edit of a batch reads the highlight this one's scripts set.
        let on = run.highlight;
        cx.set_highlight(on);
        if let Err(e) = r {
            cx.xfa_out.errors.push(e);
        }
    }
    let taken = xfa_values_from_datasets(doc).map_err(EditError::Write)?;
    if !taken.is_empty() {
        cx.xfa_out.console.push(format!("{} field value(s) taken from the imported data", taken.len()));
    }
    // Not laid out again (a static form, or scripts off): fields the data no longer holds are
    // emptied, so the data replaced is the data shown.
    if cx.xfa.is_none() && calculate {
        let data = xfa_field_data(doc);
        let present: std::collections::HashSet<String> = pdfcraft_xfa::read_values(doc, &data).into_iter().map(|(n, _)| n).collect();
        for f in pdfcraft_forms::fields(doc) {
            if present.contains(&f.name) || f.value.is_empty() {
                continue;
            }
            use pdfcraft_forms::FieldKind as K;
            let empty = match f.kind {
                K::Text => FieldValue::Text(String::new()),
                K::CheckBox => FieldValue::Check(false),
                K::Radio => FieldValue::Radio(None),
                K::Combo | K::List => FieldValue::Choice(Vec::new()),
                K::PushButton | K::Signature => continue,
            };
            if let Err(e) = pdfcraft_forms::set_value(doc, &f.name, &empty) {
                cx.xfa_out.errors.push(format!("{}: not emptied by the import: {e}", f.name));
            }
        }
    }
    Ok(())
}

/// Give the fields the values the XFA datasets hold (a form filled in another viewer). Returns
/// the names of the fields that changed.
pub(crate) fn xfa_values_from_datasets(doc: &mut pdfcraft_cos::Document) -> Result<Vec<String>, String> {
    let data = xfa_field_data(doc);
    let fields = pdfcraft_forms::fields(doc);
    // The first field of each name, as a lookup by name would find it.
    let mut by_name = std::collections::HashMap::new();
    for f in &fields {
        by_name.entry(f.name.as_str()).or_insert(f);
    }
    let mut updates = Vec::new();
    for (name, value) in pdfcraft_xfa::read_values(doc, &data) {
        let Some(f) = by_name.get(name.as_str()) else { continue };
        // A read-only AcroForm widget is intentionally left at its existing value.
        if f.has(pdfcraft_forms::flags::READ_ONLY) {
            continue;
        }
        let new = match value {
            pdfcraft_xfa::FieldData::Text(t) if matches!(f.kind, pdfcraft_forms::FieldKind::Combo | pdfcraft_forms::FieldKind::List) => {
                // Saved values, one per line for a multi-select list; shown text stands for its
                // saved value (an older viewer may have written it).
                let saved = |v: &str| f.options.iter().find(|(e, s)| e == v || s == v).map_or(v.to_string(), |(e, _)| e.clone());
                let picked: Vec<String> = if f.has(pdfcraft_forms::flags::MULTI_SELECT) {
                    t.lines().map(str::trim).filter(|l| !l.is_empty()).map(saved).collect()
                } else {
                    vec![saved(t.trim())]
                };
                (f.value != picked).then_some(FieldValue::Choice(picked))
            }
            pdfcraft_xfa::FieldData::Text(t) => (f.value.join("\n") != t).then_some(FieldValue::Text(t)),
            pdfcraft_xfa::FieldData::Check(on) => (f.value.is_empty() == on).then_some(FieldValue::Check(on)),
            // The data holds the item; the field takes the on state of the button saving writes it
            // for radio_button, which with /Opt may be a position rather than its state name.
            pdfcraft_xfa::FieldData::Radio(sel) => {
                let datum = data.iter().find(|d| d.name == name);
                let state = sel.map(|v| radio_button(doc, f, datum, &v));
                (f.value.first() != state.as_ref()).then_some(FieldValue::Radio(state))
            }
            pdfcraft_xfa::FieldData::None => None,
        };
        if let Some(v) = new {
            updates.push((name, v));
        }
    }
    if updates.is_empty() {
        return Ok(Vec::new());
    }
    // A field or recalculation failure can follow successful writes. Adopt the copy only when the
    // whole dataset batch succeeds, so no caller keeps a partial mutation.
    let mut working = doc.clone();
    let results = pdfcraft_forms::set_values(&mut working, &updates).map_err(|e| e.to_string())?;
    if let Some(((name, _), Err(error))) = updates.iter().zip(&results).find(|(_, result)| result.is_err()) {
        return Err(format!("XFA data for {name:?} could not be applied: {error}"));
    }
    *doc = working;
    Ok(updates.into_iter().map(|(name, _)| name).collect())
}
