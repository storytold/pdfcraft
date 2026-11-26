//! Remove Signature (Acrobat: Clear Signature): the dialog that lists the document's signatures
//! and saves a copy without the one chosen.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use craft::behavior::{DisplayOrder, ItemSource, Selection, SelectionChange};
use craft::host::PickRequest;
use craft::theme::variant::Tone;
use craft::ui::choice::Checkbox;
use craft::ui::contract::{Bind, CollectionEvent, SelectMode, Show, ShowCollection as _};
use craft::ui::display::Text;
use craft::ui::list::{List, ListRow};
use craft::ui::notices::Notice;
use craft::ui::patterns::form_dialogs::{FormButtons, FormDialog};
use pdfcraft_engine::DocId;
use pdfcraft_engine::clear_signature::{Effect as RemoveEffect, Kind as RemoveKind, Removable, Target as RemoveTarget};

use super::{PdfCraftApp, SignatureStatus, sign};
use crate::FilePurpose;

/// Remove Signature (Acrobat: Clear Signature): the document's signatures that can be listed,
/// the one chosen, and whether its field stays. Nothing changes until a new file is chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoveSigDraft {
    pub doc: DocId,
    pub items: Vec<Removable>,
    pub selected: Option<usize>,
    /// Keep an empty signature field to sign again, instead of removing the field.
    pub keep_field: bool,
}

impl Default for RemoveSigDraft {
    fn default() -> Self {
        RemoveSigDraft { doc: DocId(0), items: Vec::new(), selected: None, keep_field: false }
    }
}

impl RemoveSigDraft {
    fn chosen(&self) -> Option<&Removable> {
        self.selected.and_then(|i| self.items.get(i)).filter(|r| r.blocked.is_none())
    }
}

fn kind_label(r: &Removable) -> String {
    match r.kind {
        RemoveKind::Digital => tl!("Digital signature").to_string(),
        RemoveKind::Certification(p) => crate::i18n::fmt(tl!("Certification signature (level {p})"), &[("p", &p.to_string())]),
        RemoveKind::DocumentTimestamp => tl!("Document timestamp").to_string(),
        RemoveKind::Visual { initials: false } => tl!("Visual signature (Fill & Sign, not digital)").to_string(),
        RemoveKind::Visual { initials: true } => tl!("Visual initials (Fill & Sign, not digital)").to_string(),
    }
}

fn said(wording: (&'static str, String)) -> String {
    crate::i18n::fmt(tl!(wording.0), &[("names", &wording.1)])
}

/// A row's first line: what kind of signature, and who signed it.
fn head(r: &Removable) -> String {
    let mut head = kind_label(r);
    match (r.signer.as_deref(), &r.kind) {
        (Some(signer), _) => head.push_str(&format!(" — {signer}")),
        (None, RemoveKind::Visual { .. }) => {}
        (None, _) => head.push_str(&format!(" — {}", tl!("unknown signer"))),
    }
    head
}

/// A row's second line: its field, page, date and validation, then why it can't be removed.
fn details(r: &Removable) -> String {
    let mut sub = Vec::new();
    if let Some(f) = &r.field {
        sub.push(crate::i18n::fmt(tl!("Field: {field}"), &[("field", f)]));
    }
    if let Some(p) = r.page {
        sub.push(crate::i18n::fmt(tl!("Page {p}"), &[("p", &(p + 1).to_string())]));
    }
    if let Some(date) = &r.date {
        sub.push(sign::pdf::display_date(date));
    }
    if let Some(status) = r.status {
        sub.push(
            match status {
                SignatureStatus::Valid => tl!("validated: valid"),
                SignatureStatus::Unknown => tl!("validated: validity unknown"),
                SignatureStatus::Invalid => tl!("validated: invalid"),
            }
            .to_string(),
        );
    }
    let mut text = sub.join(" · ");
    if let Some(b) = &r.blocked {
        text.push('\n');
        text.push_str(&said(b.wording()));
    }
    text
}

/// The listed signatures as the rows of a list keyed by position; one that can't be removed
/// cannot take the lead.
struct Rows<'a>(&'a [Removable]);

impl DisplayOrder<usize> for Rows<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn key(&self, at: usize) -> usize {
        at
    }

    fn enabled(&self, at: usize) -> bool {
        self.0.get(at).is_some_and(|r| r.blocked.is_none())
    }

    fn text(&self, at: usize) -> Option<Cow<'_, str>> {
        self.0.get(at).map(|r| Cow::Owned(head(r)))
    }
}

impl ItemSource<usize> for Rows<'_> {}

impl PdfCraftApp {
    /// Open Remove Signature for the active document, with `field` chosen when given.
    pub fn start_remove_signature(&mut self, field: Option<&str>) {
        let Some((_, id)) = self.active_ids() else { return };
        match self.session.removable_signatures(id) {
            Ok(items) => {
                let selected = field
                    .and_then(|f| items.iter().position(|r| r.field.as_deref() == Some(f)))
                    .or_else(|| items.iter().position(|r| r.blocked.is_none()));
                self.remove_sig = Some(RemoveSigDraft { doc: id, items, selected, keep_field: false });
            }
            Err(e) => self.notify_error(e),
        }
    }

    /// The dialog's Remove and Save As: ask where the new version goes (never over the
    /// original), then remove and save there.
    fn confirm_remove_signature(&mut self) {
        let Some(draft) = self.remove_sig.take() else { return };
        let Some(chosen) = draft.chosen().cloned() else { return };
        let Some(doc) = self.session.get(draft.doc) else { return };
        let stem = Path::new(&doc.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
        let folder = doc.path.clone();
        let (id, keep_field) = (draft.doc, draft.keep_field);
        let name = crate::i18n::fmt(tl!("{name}_unsigned.pdf"), &[("name", &stem)]);
        let mut request = PickRequest::save(FilePurpose::SignedCopy.key()).file_name(name).filter("PDF", &["pdf"]);
        if let Some(parent) = folder.as_deref().and_then(|p| Path::new(p).parent()) {
            request = request.default_directory(parent);
        }
        let target = self.pick_target(Some(id));
        self.pick_first(request, target, move |app: &mut Self, ctx: &egui::Context, dest: PathBuf| {
            app.remove_signature_to(ctx, id, &chosen, keep_field, &dest);
        });
    }

    /// Remove `chosen` from document `id` (undoable) and check that it is gone; if it isn't,
    /// undo it again. Returns the edit position before the removal, for [`Self::undo_back_to`].
    fn apply_removal(&mut self, ctx: &egui::Context, id: DocId, chosen: &Removable, keep_field: bool) -> Result<u64, String> {
        let visuals =
            |app: &Self| app.session.removable_signatures(id).map(|l| l.iter().filter(|r| matches!(r.kind, RemoveKind::Visual { .. })).count());
        let before = visuals(self).map_err(|e| e.to_string())?;
        let edit = match &chosen.target {
            RemoveTarget::Field(f) => pdfcraft_engine::Edit::ClearSignature { field: f.clone(), remove_field: !keep_field },
            RemoveTarget::Annotation { page, index } => pdfcraft_engine::Edit::DeleteAnnotation { page: *page, index: *index },
            RemoveTarget::None => return Err(tl!("this signature can't be removed on its own").to_string()),
        };
        let mark = self.session.get(id).and_then(|doc| doc.edit_position()).unwrap_or_default();
        self.session.apply(id, edit).map_err(|e| e.to_string())?;
        let gone = match &chosen.target {
            RemoveTarget::Field(f) => {
                self.session.get(id).is_some_and(|doc| !doc.signatures.iter().any(|s| &s.field == f && (s.signed || !keep_field)))
            }
            _ => visuals(self).is_ok_and(|n| n < before),
        };
        self.refresh_view_of(ctx, id);
        if !gone {
            let _ = self.session.undo(id);
            self.refresh_view_of(ctx, id);
            return Err(tl!("it was still in the document afterwards, so the change was undone").to_string());
        }
        Ok(mark)
    }

    /// Undo edits back to edit position `mark`, noted before the removal: the removal, and
    /// whatever saving added after it (flattening Fill & Sign marks, when that preference is
    /// on), and never an edit the user made before it.
    fn undo_back_to(&mut self, ctx: &egui::Context, id: DocId, mark: u64) {
        while self.session.get(id).and_then(|doc| doc.edit_position()).is_some_and(|p| p > mark) {
            if self.session.undo(id).is_err() {
                break;
            }
        }
        self.refresh_view_of(ctx, id);
    }

    /// Remove `chosen` and save the document to `dest`, a file other than its own. When the
    /// save fails the removal is undone, so the document is as it was and nothing is claimed.
    pub(crate) fn remove_signature_to(&mut self, ctx: &egui::Context, id: DocId, chosen: &Removable, keep_field: bool, dest: &Path) {
        let Some(doc) = self.session.get(id) else { return };
        let dest_text = dest.to_string_lossy().into_owned();
        if doc.path.as_deref().is_some_and(|p| crate::same_file(p, &dest_text)) {
            self.notify_tr("Choose another file: the original is kept as it is, and the version without the signature is saved next to it.");
            return;
        }
        let mark = match self.apply_removal(ctx, id, chosen, keep_field) {
            Ok(mark) => mark,
            Err(e) => {
                self.notify_fmt("The signature was not removed: {e}", &[("e", &e)]);
                return;
            }
        };
        if !self.save_doc_to(ctx, id, dest, crate::SaveTarget::As) {
            // The save said why; the document goes back to how it was.
            self.undo_back_to(ctx, id, mark);
            return;
        }
        let Some(doc) = self.session.get(id) else { return };
        let file = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(dest_text);
        let what = match (&chosen.target, keep_field) {
            (RemoveTarget::Annotation { page, .. }, _) => {
                crate::i18n::fmt(tl!("Removed the visual signature on page {p}"), &[("p", &(page + 1).to_string())])
            }
            (_, true) => {
                crate::i18n::fmt(tl!("Removed the signature {field} and kept its empty field"), &[("field", chosen.field.as_deref().unwrap_or(""))])
            }
            (_, false) => crate::i18n::fmt(tl!("Removed the signature {field} with its field"), &[("field", chosen.field.as_deref().unwrap_or(""))]),
        };
        let history = if doc.is_signed() {
            tl!("Other signatures remain, so the removed one is still in the file's earlier revision; check the remaining signatures again.")
        } else {
            tl!("The new file has no signature left and no earlier revision holding it.")
        };
        self.notify(crate::i18n::fmt(
            tl!("{what}. Saved {file}, checked to reopen with every page. {history}"),
            &[("what", &what), ("file", &file), ("history", history)],
        ));
        self.choose_right_panel(Some(crate::RightPanel::Signatures));
    }
}

/// A line of explanatory text.
fn caption(text: impl Into<String>) -> impl Show {
    Text::new(text.into())
}

/// Shows Remove Signature while its draft is open, and carries out Remove and Save As.
pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let Some(draft) = app.remove_sig.as_ref() else { return };
    let dirty = app.session.get(draft.doc).is_some_and(|d| d.dirty);
    // With nothing that can be removed there is nothing to accept: the dialog only explains, with Close.
    let any = draft.items.iter().any(|r| r.blocked.is_none());
    let mut open = true;
    let out = {
        let Some(d) = app.remove_sig.as_mut() else { return };
        let can_go = d.chosen().is_some();
        let dialog = FormDialog::new(tl!("Remove Signature"), Bind::bound(d)).opened(Bind::bound(&mut open)).accept_stays_open(true);
        let dialog = if any { dialog.ok_label(tl!("Remove and Save As…")).ok_enabled(can_go) } else { dialog.buttons(FormButtons::Close) };
        dialog.show(ui, |ui, _, d| {
            body(ui, d, dirty);
            d.chosen().is_some()
        })
    };
    if out.closed.is_some() || !open {
        app.remove_sig = None;
        return;
    }
    if any && out.picked == Some(0) {
        app.confirm_remove_signature();
    }
}

/// The dialog's body: the signatures to choose from, and what removing the chosen one does.
fn body(ui: &mut egui::Ui, d: &mut RemoveSigDraft, dirty: bool) {
    if d.items.is_empty() {
        caption(tl!("This document has no signature that can be removed.")).show(ui);
        caption(tl!(
            "A signature that is part of a page's content, like a scanned page, is not a field or an annotation, so it can't be removed on its own, and PdfCraft doesn't erase parts of a page to hide it. To take it out, redact that area (Redact), or delete the picture with Edit PDF when the signature is an image of its own."
        ))
        .show(ui);
        return;
    }
    caption(tl!("Choose the signature to remove:")).show(ui);
    let items = d.items.clone();
    let source = Rows(&items);
    let mut selection = d.selected.map_or_else(Selection::default, Selection::single);
    let out = List::<usize>::new(tl!("Signatures"))
        .select(SelectMode::Single)
        .natural_height(true)
        .selection(Bind::bound(&mut selection))
        .show_items(ui, &source, &mut |_, icx| match items.get(icx.at) {
            Some(r) => ListRow::new(head(r)).below(caption(details(r)), |_| {}),
            None => ListRow::new(String::new()),
        });
    for event in out.events {
        match event {
            CollectionEvent::Selection(SelectionChange::Set(keys)) => {
                d.selected = keys.last().copied().filter(|&i| items.get(i).is_some_and(|r| r.blocked.is_none()));
            }
            CollectionEvent::Selection(SelectionChange::Toggle(key)) => d.selected = (d.selected != Some(key)).then_some(key),
            CollectionEvent::Selection(SelectionChange::Clear) => d.selected = None,
            _ => {}
        }
    }
    let Some(chosen) = d.chosen().cloned() else {
        // Nothing chosen yet: a blocked one was asked for (from its row in the Signatures panel),
        // or none can be removed at all.
        let any = d.items.iter().any(|r| r.blocked.is_none());
        caption(if any { tl!("Choose a signature that can be removed.") } else { tl!("None of these signatures can be removed.") }).show(ui);
        return;
    };
    let digital = matches!(chosen.target, RemoveTarget::Field(_));
    if digital {
        Checkbox::new(Bind::bound(&mut d.keep_field)).label(tl!("Keep an empty signature field to sign again")).show(ui);
    }
    let others = chosen.effects.iter().any(|e| matches!(e, RemoveEffect::OtherSignatures(_)));
    let warning = match (digital, others) {
        (true, true) => tl!(
            "Removing this signature changes the document and invalidates this signature in the version that results. It can affect the other signatures and certifications. The original file is kept as it is."
        ),
        (true, false) => tl!(
            "Removing this signature changes the document and invalidates this signature in the version that results. The original file is kept as it is."
        ),
        (false, _) => {
            tl!("This is a picture of a signature, not a digital signature: only the mark on the page goes. The original file is kept as it is.")
        }
    };
    Notice::new(Tone::Warn, warning.to_owned()).show(ui);
    for e in &chosen.effects {
        caption(format!("• {}", said(e.wording()))).show(ui);
    }
    if dirty {
        caption(format!("• {}", tl!("Unsaved changes to this document are saved in the new file too."))).show(ui);
    }
    caption(tl!("You choose where the new version is saved. Continue?")).show(ui);
}
