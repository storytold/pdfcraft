//! Remove Signature (Acrobat: Clear Signature): the dialog that lists the document's signatures
//! and saves a copy without the one chosen.

use pdfcraft_engine::DocId;
use pdfcraft_engine::clear_signature::{Effect as RemoveEffect, Kind as RemoveKind, Removable, Target as RemoveTarget};

use super::dialog::title;
use super::{Align, Layout, PdfCraftApp, SignatureStatus, Tokens, sign, signature_status_style, theme, widgets};

/// Remove Signature (Acrobat: Clear Signature): the document's signatures that can be listed,
/// the one chosen, and whether its field stays. Nothing changes until a new file is chosen.
#[derive(Clone, Debug)]
pub struct RemoveSigDraft {
    pub doc: DocId,
    pub items: Vec<Removable>,
    pub selected: Option<usize>,
    /// Keep an empty signature field to sign again, instead of removing the field.
    pub keep_field: bool,
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
                self.dialog = Some(crate::Dialog::RemoveSignature);
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
        let stem = std::path::Path::new(&doc.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
        let keep_field = draft.keep_field;
        let id = draft.doc;
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Tests and scripts choose the destination ahead, as for Save As.
            if let Some(dest) = self.save_override.clone() {
                self.remove_signature_to(id, &chosen, keep_field, &dest);
                return;
            }
            let name = crate::i18n::fmt(tl!("{name}_unsigned.pdf"), &[("name", &stem)]);
            let dialog = rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(name);
            self.ask_one(crate::pickers::Ask::Save(dialog), Some(id), move |app, dest| {
                app.remove_signature_to(id, &chosen, keep_field, &dest.to_string_lossy());
            });
        }
        // The browser saves by downloading, with nowhere to choose first: remove, then download,
        // and undo the removal if the download fails.
        #[cfg(target_arch = "wasm32")]
        {
            let _ = stem;
            let mark = match self.apply_removal(id, &chosen, keep_field) {
                Ok(mark) => mark,
                Err(e) => {
                    self.notify_fmt("The signature was not removed: {e}", &[("e", &e)]);
                    return;
                }
            };
            let saved = self.views.iter().position(|v| v.id == id).is_some_and(|index| self.save_view(index, crate::SaveTarget::As));
            if !saved {
                self.undo_back_to(id, mark);
            }
        }
    }

    /// Remove `chosen` from document `id` (undoable) and check that it is gone; if it isn't,
    /// undo it again. Returns the edit position before the removal, for [`Self::undo_back_to`].
    fn apply_removal(&mut self, id: DocId, chosen: &Removable, keep_field: bool) -> Result<u64, String> {
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
        self.refresh_view_of(id);
        if !gone {
            let _ = self.session.undo(id);
            self.refresh_view_of(id);
            return Err(tl!("it was still in the document afterwards, so the change was undone").to_string());
        }
        Ok(mark)
    }

    /// Undo edits back to edit position `mark`, noted before the removal: the removal, and
    /// whatever saving added after it (flattening Fill & Sign marks, when that preference is
    /// on), and never an edit the user made before it.
    fn undo_back_to(&mut self, id: DocId, mark: u64) {
        while self.session.get(id).and_then(|doc| doc.edit_position()).is_some_and(|p| p > mark) {
            if self.session.undo(id).is_err() {
                break;
            }
        }
        self.refresh_view_of(id);
    }

    fn refresh_view_of(&mut self, id: DocId) {
        if let Some(doc) = self.session.get(id)
            && let Some(view) = self.views.iter_mut().find(|v| v.id == id)
        {
            view.document_changed(&doc.info);
        }
    }

    /// Remove `chosen` and save the document to `dest`, a file other than its own. When the
    /// save fails the removal is undone, so the document is as it was and nothing is claimed.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn remove_signature_to(&mut self, id: DocId, chosen: &Removable, keep_field: bool, dest: &str) {
        let Some(doc) = self.session.get(id) else { return };
        if doc.path.as_deref().is_some_and(|p| crate::same_file(p, dest)) {
            self.notify_tr("Choose another file: the original is kept as it is, and the version without the signature is saved next to it.");
            return;
        }
        let mark = match self.apply_removal(id, chosen, keep_field) {
            Ok(mark) => mark,
            Err(e) => {
                self.notify_fmt("The signature was not removed: {e}", &[("e", &e)]);
                return;
            }
        };
        if !self.save_doc_to(id, dest, crate::SaveTarget::As) {
            // The save said why; the document goes back to how it was.
            self.undo_back_to(id, mark);
            return;
        }
        let Some(doc) = self.session.get(id) else { return };
        let file = std::path::Path::new(dest).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dest.to_string());
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

/// The Remove Signature dialog. Returns `true` when it closes.
pub(crate) fn remove_dialog(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    let dirty = app.remove_sig.as_ref().and_then(|d| app.session.get(d.doc)).is_some_and(|d| d.dirty);
    let Some(d) = app.remove_sig.as_mut() else { return true };
    title(ui, tl!("Remove Signature"));
    if d.items.is_empty() {
        ui.label(tl!("This document has no signature that can be removed."));
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(tl!(
                    "A signature that is part of a page's content, like a scanned page, is not a field or an annotation, so it can't be removed on its own, and PdfCraft doesn't erase parts of a page to hide it. To take it out, redact that area (Redact), or delete the picture with Edit PDF when the signature is an image of its own."
                ))
                .color(t.text_muted),
            )
            .wrap(),
        );
        ui.add_space(12.0);
        let mut close = false;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| close = widgets::pill_button(ui, tl!("Close"), true).clicked());
        return close;
    }
    ui.label(tl!("Choose the signature to remove:"));
    ui.add_space(4.0);
    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
        for (i, r) in d.items.iter().enumerate() {
            let mut head = kind_label(r);
            match (r.signer.as_deref(), &r.kind) {
                (Some(signer), _) => head.push_str(&format!(" — {signer}")),
                (None, RemoveKind::Visual { .. }) => {}
                (None, _) => head.push_str(&format!(" — {}", tl!("unknown signer"))),
            }
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
            let enabled = r.blocked.is_none();
            ui.add_enabled_ui(enabled, |ui| {
                if ui.add(widgets::radio(d.selected == Some(i), egui::RichText::new(head).font(theme::semibold(12.5)))).clicked() {
                    d.selected = Some(i);
                }
            });
            ui.indent(("remove-sig-row", i), |ui| {
                ui.add(egui::Label::new(egui::RichText::new(sub.join(" · ")).font(theme::regular(11.5)).color(t.text_muted)).wrap());
                if let Some(b) = &r.blocked {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(said(b.wording()))
                                .font(theme::regular(11.5))
                                .color(signature_status_style(SignatureStatus::Invalid).1),
                        )
                        .wrap(),
                    );
                }
            });
            ui.add_space(4.0);
        }
    });
    let Some(chosen) = d.chosen().cloned() else {
        // Nothing chosen yet: a blocked one was asked for (from its row in the Signatures panel),
        // or none can be removed at all.
        let any = d.items.iter().any(|r| r.blocked.is_none());
        ui.add_space(8.0);
        let text = if any { tl!("Choose a signature that can be removed.") } else { tl!("None of these signatures can be removed.") };
        ui.label(egui::RichText::new(text).color(t.text_muted));
        ui.add_space(12.0);
        let mut close = false;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            close = widgets::pill_button(ui, if any { tl!("Cancel") } else { tl!("Close") }, !any).clicked();
        });
        if close {
            app.remove_sig = None;
        }
        return close;
    };
    ui.add_space(8.0);
    let digital = matches!(chosen.target, RemoveTarget::Field(_));
    if digital {
        ui.add(widgets::checkbox(&mut d.keep_field, tl!("Keep an empty signature field to sign again")));
        ui.add_space(6.0);
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
    ui.add(egui::Label::new(egui::RichText::new(warning).font(theme::semibold(12.0))).wrap());
    for e in &chosen.effects {
        ui.add(egui::Label::new(egui::RichText::new(format!("• {}", said(e.wording()))).font(theme::regular(11.5)).color(t.text_muted)).wrap());
    }
    if dirty {
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("• {}", tl!("Unsaved changes to this document are saved in the new file too.")))
                    .font(theme::regular(11.5))
                    .color(t.text_muted),
            )
            .wrap(),
        );
    }
    ui.add_space(4.0);
    ui.add(egui::Label::new(egui::RichText::new(tl!("You choose where the new version is saved. Continue?")).font(theme::regular(12.0))).wrap());
    ui.add_space(12.0);
    let (mut go, mut close) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        go = widgets::pill_button(ui, tl!("Remove and Save As…"), true).clicked();
        close = widgets::pill_button(ui, tl!("Cancel"), false).clicked();
    });
    if go {
        app.confirm_remove_signature();
        return true;
    }
    if close {
        app.remove_sig = None;
    }
    close
}
