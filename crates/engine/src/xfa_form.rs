//! A document's XFA form: laying it out, keeping its datasets and fields in step, and taking it out.

use crate::edit::EditCtx;
use crate::{Document, Edit, EditError, FieldValue, XfaLayout, guard, xfa};

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

/// Keep sibling AcroForm widgets bound to the same XFA data node (such as a repeated
/// page-area field with `match="global"` or an explicit data reference) in step after an edit
/// or script run, before [`xfa_sync_datasets`] writes their values into the datasets packet,
/// and return the current field list so callers avoid an extra form walk.
pub(crate) fn xfa_sync_shared_fields(
    prev_fields: &[pdfcraft_forms::Field],
    next: &mut pdfcraft_cos::Document,
    tpl: Option<&pdfcraft_xfa::model::Template>,
    edit: Option<&Edit>,
) -> Result<Vec<pdfcraft_forms::Field>, String> {
    let next_fields = pdfcraft_forms::fields(next);
    if next_fields.len() < 2 {
        return Ok(next_fields);
    }
    let parsed;
    let tpl = match tpl {
        Some(t) => Some(t),
        None => {
            parsed = pdfcraft_xfa::template_of(next).ok();
            parsed.as_ref()
        }
    };
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut group_idx: std::collections::HashMap<pdfcraft_xfa::DataPath, usize> = std::collections::HashMap::new();
    for (idx, f) in next_fields.iter().enumerate() {
        if matches!(f.kind, pdfcraft_forms::FieldKind::PushButton | pdfcraft_forms::FieldKind::Signature) {
            continue;
        }
        let som = next
            .get(f.obj)
            .as_dict()
            .and_then(|d| d.get(pdfcraft_xfa::SOM_KEY).map(|s| next.resolve(s)))
            .and_then(|s| s.as_string().map(|s| s.to_text()))
            .unwrap_or_else(|| f.name.clone());
        let path = match tpl {
            Some(t) => pdfcraft_xfa::data_path(t, &som),
            None => pdfcraft_xfa::som_to_path(&som),
        };
        if path.is_empty() {
            continue;
        }
        match group_idx.get(&path).copied() {
            Some(g) => groups[g].push(idx),
            None => {
                let g = groups.len();
                group_idx.insert(path, g);
                groups.push(vec![idx]);
            }
        }
    }
    if !groups.iter().any(|m| m.len() >= 2) {
        return Ok(next_fields);
    }
    let prev_values: std::collections::HashMap<&str, &[String]> = prev_fields.iter().map(|f| (f.name.as_str(), f.value.as_slice())).collect();
    let mut targets = Vec::new();
    if let Some(e) = edit {
        collect_edit_targets(e, &mut targets);
    }
    if targets.is_empty() && next_fields.iter().all(|f| prev_values.get(f.name.as_str()) == Some(&f.value.as_slice())) {
        return Ok(next_fields);
    }
    let data = xfa_field_data(next);
    let mut changes: Vec<pdfcraft_forms::FieldChange> = Vec::new();
    for members in groups {
        if members.len() < 2 {
            continue;
        }
        let explicit = targets.iter().rev().find_map(|t| {
            let &m = members.iter().find(|&&m| next_fields[m].name == *t)?;
            let changed = prev_values.get(next_fields[m].name.as_str()) != Some(&next_fields[m].value.as_slice());
            let disagrees = members.iter().any(|&o| next_fields[o].value != next_fields[m].value);
            (changed || disagrees).then_some(m)
        });
        let leader = explicit.or_else(|| {
            members.iter().rev().copied().find(|&m| prev_values.get(next_fields[m].name.as_str()) != Some(&next_fields[m].value.as_slice()))
        });
        let Some(src_idx) = leader else { continue };
        let src = &next_fields[src_idx];
        let text = shared_field_text(next, src, data.get(src_idx));
        for &dst_idx in &members {
            if dst_idx == src_idx {
                continue;
            }
            let dst = &next_fields[dst_idx];
            if let Some(value) = shared_peer_value(next, src, dst, data.get(dst_idx), &text) {
                changes.push(pdfcraft_forms::FieldChange { name: dst.name.clone(), value: Some(value), ..Default::default() });
            }
        }
    }
    if !changes.is_empty() {
        pdfcraft_forms::apply_script_changes(next, &changes, &mut pdfcraft_forms::NoScripts).map_err(|e| e.to_string())?;
        return Ok(pdfcraft_forms::fields(next));
    }
    Ok(next_fields)
}

fn collect_edit_targets(edit: &Edit, out: &mut Vec<String>) {
    match edit {
        Edit::SetFieldValue { name, .. } => out.push(name.clone()),
        Edit::ResetForm { names: Some(names) } => out.extend(names.iter().cloned()),
        Edit::ApplyScriptChanges { changes } => {
            for c in changes {
                if c.value.is_some() {
                    out.push(c.name.clone());
                }
            }
        }
        Edit::Batch { edits, .. } => edits.iter().for_each(|e| collect_edit_targets(e, out)),
        _ => {}
    }
}

fn check_items_of(doc: &pdfcraft_cos::Document, obj: pdfcraft_cos::ObjRef) -> (String, String) {
    let items: Vec<String> = doc
        .get(obj)
        .as_dict()
        .and_then(|d| d.get(b"PCItems").map(|o| doc.resolve(o)))
        .and_then(|a| a.as_array().map(|a| a.iter().filter_map(|x| x.as_string().map(|s| s.to_text())).collect()))
        .unwrap_or_default();
    let on = items.first().cloned().unwrap_or_else(|| "1".into());
    let off = items.get(1).cloned().unwrap_or_default();
    (on, off)
}

fn shared_field_text(doc: &pdfcraft_cos::Document, f: &pdfcraft_forms::Field, datum: Option<&pdfcraft_xfa::FieldDatum>) -> String {
    use pdfcraft_forms::FieldKind as K;
    match f.kind {
        K::Text | K::Combo | K::List => f.value.join("\n"),
        K::CheckBox => {
            let (on, off) = check_items_of(doc, f.obj);
            if !f.value.is_empty() { on } else { off }
        }
        K::Radio => datum
            .map_or_else(|| f.value.first().cloned().unwrap_or_default(), |d| pdfcraft_xfa::radio_dataset_value(doc, d, &f.value.first().cloned())),
        K::PushButton | K::Signature => String::new(),
    }
}

fn shared_peer_value(
    doc: &pdfcraft_cos::Document,
    src: &pdfcraft_forms::Field,
    dst: &pdfcraft_forms::Field,
    dst_datum: Option<&pdfcraft_xfa::FieldDatum>,
    text: &str,
) -> Option<Vec<String>> {
    use pdfcraft_forms::FieldKind as K;
    match dst.kind {
        K::Text => (dst.value.join("\n") != text).then(|| vec![text.to_owned()]),
        K::CheckBox => {
            let checked = if src.kind == K::CheckBox && check_items_of(doc, src.obj) == check_items_of(doc, dst.obj) {
                !src.value.is_empty()
            } else {
                let (on, _) = check_items_of(doc, dst.obj);
                pdfcraft_xfa::data::is_on(text, &on)
            };
            if dst.value.is_empty() != checked {
                return None;
            }
            let on_state = dst.widgets.first().and_then(|w| w.on_state.clone()).unwrap_or_else(|| "1".into());
            Some(if checked { vec![on_state] } else { Vec::new() })
        }
        K::Combo | K::List => {
            let saved = |v: &str| dst.options.iter().find(|(e, s)| e == v || s == v).map_or_else(|| v.to_owned(), |(e, _)| e.clone());
            let picked: Vec<String> = if dst.kind == K::List && dst.has(pdfcraft_forms::flags::MULTI_SELECT) {
                text.lines().map(str::trim).filter(|l| !l.is_empty()).map(saved).collect()
            } else if text.trim().is_empty() {
                Vec::new()
            } else {
                vec![saved(text.trim())]
            };
            (dst.value != picked).then_some(picked)
        }
        K::Radio => {
            let sel = (!text.trim().is_empty()).then(|| radio_button(doc, dst, dst_datum, text.trim()));
            (dst.value.first() != sel.as_ref()).then(|| sel.into_iter().collect())
        }
        K::PushButton | K::Signature => None,
    }
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
    sync_usage_rights_warning(doc);
}

/// Keep [`pdfcraft_xfa::USAGE_RIGHTS_REMOVED`] in step with whether saving still preserves the
/// original usage-rights shell (#4309): withheld while `ur_shell` is kept (`ur_data_only`), shown
/// as soon as an edit leaves data-only mode (while the document is still a laid-out XFA form),
/// and removed again if that edit is undone or reverted.
pub(crate) fn sync_usage_rights_warning(doc: &mut Document) {
    let msg = pdfcraft_xfa::USAGE_RIGHTS_REMOVED;
    let Some(ed) = doc.editor.as_ref() else { return };
    match ed.ur_shell.as_ref().map(|_| ed.ur_data_only) {
        Some(true) => {
            if let Some(xfa) = doc.xfa.as_mut() {
                xfa.warnings.retain(|w| w != msg);
            }
            doc.xfa_warnings.retain(|w| w != msg);
            doc.info.warnings.retain(|w| w != msg);
        }
        Some(false) if doc.xfa.is_some() => {
            if let Some(xfa) = doc.xfa.as_mut()
                && !xfa.warnings.iter().any(|w| w == msg)
            {
                xfa.warnings.insert(0, msg.to_owned());
            }
            let single = [msg.to_owned()];
            note_warnings(&mut doc.xfa_warnings, &single);
            note_warnings(&mut doc.info.warnings, &single);
        }
        Some(false) => {
            doc.xfa_warnings.retain(|w| w != msg);
            doc.info.warnings.retain(|w| w != msg);
        }
        None => {
            if doc.xfa_warnings.iter().any(|w| w == msg)
                && let Some(xfa) = doc.xfa.as_mut()
                && !xfa.warnings.iter().any(|w| w == msg)
            {
                xfa.warnings.insert(0, msg.to_owned());
            }
        }
    }
}

/// Whether `edit` (taking `prev` to `next` with `sync_warnings` from [`xfa_sync_datasets`] and
/// `xfa_errors` from `run_edit`/scripts) changes only XFA form data or `<form>`-packet script
/// state, so saving into the original shell preserves everything the user changed (#4309).
pub(crate) fn keeps_xfa_usage_rights(
    edit: &Edit,
    prev: &pdfcraft_cos::Document,
    prev_fields: &[pdfcraft_forms::Field],
    next: &pdfcraft_cos::Document,
    sync_warnings: &[String],
    xfa_errors: &[String],
) -> bool {
    pdfcraft_xfa::existing_layout(next).is_some()
        && sync_warnings.is_empty()
        && xfa_errors.is_empty()
        && pdfcraft_xfa::overrides_roundtrip(next)
        && edit_is_xfa_data_only(edit, prev, prev_fields, next, xfa_errors)
}

fn field_som_key(doc: &pdfcraft_cos::Document, mut obj: pdfcraft_cos::ObjRef) -> Option<String> {
    let mut seen = Vec::new();
    for _ in 0..16 {
        if seen.contains(&obj) {
            return None;
        }
        seen.push(obj);
        match &*doc.get(obj) {
            pdfcraft_cos::Object::Ref(next) => obj = *next,
            pdfcraft_cos::Object::Dict(d) => {
                let raw = d.get(pdfcraft_xfa::SOM_KEY)?;
                return doc.resolve(raw).as_string().map(|s| s.to_text());
            }
            _ => return None,
        }
    }
    None
}

fn edit_is_xfa_data_only(
    edit: &Edit,
    prev: &pdfcraft_cos::Document,
    prev_fields: &[pdfcraft_forms::Field],
    next: &pdfcraft_cos::Document,
    xfa_errors: &[String],
) -> bool {
    match edit {
        Edit::SetFieldValue { .. } | Edit::ResetForm { .. } => acroform_field_properties_unchanged(prev, prev_fields, next),
        Edit::XfaEvent { .. } => true,
        Edit::SetFieldImage { name, .. } | Edit::SetFieldImageTarget { name, .. } => {
            if !xfa_errors.is_empty() {
                return false;
            }
            let Ok(tpl) = pdfcraft_xfa::template_of(prev) else {
                return false;
            };
            let target = match edit {
                Edit::SetFieldImageTarget { object, .. } => Some(*object),
                _ => None,
            };
            let mut matched_any = false;
            for f in prev_fields.iter().filter(|f| f.name == *name && target.is_none_or(|o| f.obj == o)) {
                matched_any = true;
                if f.button != Some(pdfcraft_forms::af::ButtonAction::ImportIcon)
                    || !field_som_key(prev, f.obj).is_some_and(|som| pdfcraft_xfa::is_template_image_field(&tpl, &som))
                {
                    return false;
                }
            }
            matched_any
        }
        Edit::ImportData { bytes, .. } => {
            !bytes[..bytes.len().min(1024)].windows(5).any(|w| w == b"%PDF-")
                && acroform_field_properties_unchanged(prev, prev_fields, next)
                && (matches!(decode_data_file(bytes), Some(Ok(t)) if is_xfa_data_file(&t) && xfa_errors.is_empty())
                    || pdfcraft_xfdf::export_xfdf(prev, true, false, "") == pdfcraft_xfdf::export_xfdf(next, true, false, ""))
        }
        Edit::ApplyScriptChanges { changes } => {
            changes.iter().all(field_change_is_value_only) && acroform_field_properties_unchanged(prev, prev_fields, next)
        }
        Edit::Batch { edits, .. } => edits.iter().all(|e| edit_is_xfa_data_only(e, prev, prev_fields, next, xfa_errors)),
        _ => false,
    }
}

/// Whether `c` changes only a field's value (no flags, appearance, geometry, options or actions
/// that [`pdfcraft_xfa::sync_form_into_shell`] cannot serialize into the XFA packets).
pub(crate) fn field_change_is_value_only(c: &pdfcraft_forms::FieldChange) -> bool {
    let pdfcraft_forms::FieldChange {
        name: _,
        object: _,
        value: _,
        formatted: _,
        read_only,
        required,
        display,
        caption,
        text_color,
        fill_color,
        rect,
        stroke_color,
        text_size,
        line_width,
        border_style,
        user_name,
        icon,
        options,
        actions,
    } = c;
    read_only.is_none()
        && required.is_none()
        && display.is_none()
        && text_color.is_none()
        && fill_color.is_none()
        && caption.is_none()
        && rect.is_none()
        && text_size.is_none()
        && stroke_color.is_none()
        && line_width.is_none()
        && border_style.is_none()
        && user_name.is_none()
        && icon.is_none()
        && options.is_none()
        && actions.is_empty()
}

/// Whether no pre-existing AcroForm field in `next` had non-value properties (flags, appearance,
/// geometry, options, tooltip or actions) modified by an AcroForm callback during `prev -> next`,
/// and no non-XFA shell field changed value without a matching laid-out XFA field holding that
/// value.
fn acroform_field_properties_unchanged(prev: &pdfcraft_cos::Document, prev_fields: &[pdfcraft_forms::Field], next: &pdfcraft_cos::Document) -> bool {
    let next_fields = pdfcraft_forms::fields(next);
    let prev_by_obj: std::collections::HashMap<pdfcraft_cos::ObjRef, &pdfcraft_forms::Field> = prev_fields.iter().map(|f| (f.obj, f)).collect();
    for after in &next_fields {
        let is_laid_out_xfa = next.get(after.obj).as_dict().is_some_and(|d| d.contains(pdfcraft_xfa::SOM_KEY));
        let Some(before) = prev_by_obj.get(&after.obj) else {
            if !is_laid_out_xfa {
                return false;
            }
            continue;
        };
        if !field_properties_unchanged(prev, before, next, after) {
            return false;
        }
        if !is_laid_out_xfa && before.value != after.value {
            let shared_with_xfa = next_fields.iter().any(|other| {
                other.obj != after.obj
                    && other.value == after.value
                    && next
                        .get(other.obj)
                        .as_dict()
                        .and_then(|d| d.get(pdfcraft_xfa::SOM_KEY).map(|s| next.resolve(s)))
                        .and_then(|s| s.as_string().map(|s| s.to_text()))
                        .is_some_and(|som| som == after.name || other.name == after.name)
            });
            if !shared_with_xfa {
                return false;
            }
        }
    }
    true
}

fn field_properties_unchanged(
    prev: &pdfcraft_cos::Document,
    before: &pdfcraft_forms::Field,
    next: &pdfcraft_cos::Document,
    after: &pdfcraft_forms::Field,
) -> bool {
    if before.flags != after.flags
        || before.options != after.options
        || before.da != after.da
        || before.tooltip != after.tooltip
        || before.actions != after.actions
        || before.widgets.len() != after.widgets.len()
        || !same_dict_entry(prev, before.obj, next, after.obj, b"AA")
    {
        return false;
    }
    before.widgets.iter().zip(&after.widgets).all(|(wb, wa)| {
        wb.obj == wa.obj
            && wb.rect == wa.rect
            && wb.hidden == wa.hidden
            && wb.read_only == wa.read_only
            && wb.caption == wa.caption
            && wb.mouse_up_script == wa.mouse_up_script
            && [b"F".as_slice(), b"MK".as_slice(), b"BS".as_slice(), b"AA".as_slice()]
                .into_iter()
                .all(|key| same_dict_entry(prev, wb.obj, next, wa.obj, key))
    })
}

fn same_dict_entry(
    prev: &pdfcraft_cos::Document,
    obj_a: pdfcraft_cos::ObjRef,
    next: &pdfcraft_cos::Document,
    obj_b: pdfcraft_cos::ObjRef,
    key: &[u8],
) -> bool {
    let a = prev.get(obj_a);
    let b = next.get(obj_b);
    let va = a.as_dict().and_then(|d| d.get(key));
    let vb = b.as_dict().and_then(|d| d.get(key));
    same_resolved_object(prev, va, next, vb, 0)
}

fn same_resolved_object(
    prev: &pdfcraft_cos::Document,
    a: Option<&pdfcraft_cos::Object>,
    next: &pdfcraft_cos::Document,
    b: Option<&pdfcraft_cos::Object>,
    depth: usize,
) -> bool {
    if depth > 8 {
        return false;
    }
    match (a.map(|o| prev.resolve(o)), b.map(|o| next.resolve(o))) {
        (None, None) => true,
        (Some(a), Some(b)) => match (&*a, &*b) {
            (pdfcraft_cos::Object::Dict(da), pdfcraft_cos::Object::Dict(db)) => {
                da.len() == db.len() && da.iter().all(|(k, va)| same_resolved_object(prev, Some(va), next, db.get(k), depth + 1))
            }
            (pdfcraft_cos::Object::Array(aa), pdfcraft_cos::Object::Array(ab)) => {
                aa.len() == ab.len() && aa.iter().zip(ab.iter()).all(|(va, vb)| same_resolved_object(prev, Some(va), next, Some(vb), depth + 1))
            }
            (oa, ob) => oa == ob,
        },
        _ => false,
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
        let new = match value {
            pdfcraft_xfa::FieldData::Text(t) if matches!(f.kind, pdfcraft_forms::FieldKind::Combo | pdfcraft_forms::FieldKind::List) => {
                // Saved values, one per line for a multi-select list; shown text stands for
                // its saved value (an older viewer may have written it).
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
            // for ([`radio_button`]), which with `/Opt` may be a position (`1` is the state `/0`).
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
    // All at once: one by one, each value would read the whole form again, which took minutes
    // on a form with thousands of fields.
    let Ok(results) = pdfcraft_forms::set_values(doc, &updates) else { return Ok(Vec::new()) };
    Ok(updates.into_iter().zip(results).filter(|(_, r)| r.is_ok()).map(|((name, _), _)| name).collect())
}
