//! Copy and paste the selected fields while preparing a form (#1571).

use pdfcraft_engine::{Edit, FormField};

use super::PrepareView;
use crate::canvas::DocView;

/// How far a pasted field lands from the one it copies, in points (right and down).
const PASTE_STEP: f64 = 18.0;

/// Copy: remember the selected widgets and return the text to put on the clipboard (their
/// field names, one per line). Paste brings the fields back only while the clipboard still
/// holds that text, not after something else has been copied since. `None` with nothing selected.
pub(crate) fn copy_selected(prepare: &mut PrepareView) -> Option<String> {
    prepare.selected.as_ref()?;
    prepare.copied = prepare.selected.iter().chain(prepare.also.iter()).cloned().collect();
    prepare.pastes = 0;
    prepare.clip = prepare.names().join("\n");
    Some(prepare.clip.clone())
}

/// Whether pasting `text` brings back the fields Copy took.
pub(crate) fn pastes_fields(prepare: &PrepareView, text: Option<&str>) -> bool {
    !prepare.copied.is_empty() && text.is_some_and(|t| t.replace("\r\n", "\n").trim_end() == prepare.clip.trim_end())
}

/// Paste: a copy of every copied field on the current page, as one undoable step. Each copy is a
/// field of its own with the next free name. On the page they came from the copies land one step
/// right and down, and each further paste one step more; on another page the first lands in
/// place. The copies are selected once the edit is applied.
pub(crate) fn paste_copied(view: &mut DocView, form: &[FormField]) -> Option<Edit> {
    let current = view.current();
    let prepare = &mut view.session.prepare;
    let mut sources: Vec<(String, Vec<usize>)> = Vec::new();
    let mut from_page = None;
    for (name, wi) in &prepare.copied {
        let Some(w) = form.iter().find(|f| &f.name == name).and_then(|f| f.widgets.get(*wi)) else { continue };
        from_page = from_page.or(w.page);
        match sources.iter_mut().find(|(n, _)| n == name) {
            Some((_, widgets)) => widgets.push(*wi),
            None => sources.push((name.clone(), vec![*wi])),
        }
    }
    if sources.is_empty() {
        return None;
    }
    let steps = prepare.pastes.saturating_add(u32::from(from_page == Some(current)));
    prepare.pastes = prepare.pastes.saturating_add(1);
    let d = PASTE_STEP * f64::from(steps);
    let offset = [d, -d];
    let mut edits: Vec<Edit> = sources.into_iter().map(|(name, widgets)| Edit::CopyField { name, widgets, page: current, offset }).collect();
    prepare.selected = None;
    prepare.also.clear();
    prepare.select_pasted = Some(form.len());
    if edits.len() == 1 {
        return edits.pop();
    }
    Some(Edit::Batch { label: "Paste fields".into(), edits })
}
