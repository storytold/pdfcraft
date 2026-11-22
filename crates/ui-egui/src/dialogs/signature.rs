//! The signature pad and the lock question, both shown on the dialog host.

use craft::review::ui::signature::{SignaturePadDialog, SignaturePadResult};
use craft::review::{SavedSignature, SignatureDraft, SignatureTab};
use craft::ui::dialog::{DialogResult, PromptAnswer, PromptChoice, StandardPrompt};

use crate::PdfCraftApp;
use crate::editing::PendingLock;
use crate::fill_sign::FillTool;
// PLAN: `fill_sign::scan` must be `pub(crate) mod scan` (pdf-forms-w5 owns `fill_sign.rs`).
use crate::fill_sign::scan;

/// Opens the signature pad (initials, else the signature), filled from the saved slot when there is one. The draft
/// holds a copy, so Cancel leaves the slot alone. `editing` makes the title say Change.
pub(crate) fn open(app: &mut PdfCraftApp, initials: bool, editing: bool) {
    let mut draft = SignatureDraft::new(initials, &app.comment_prefs.author);
    let saved = if initials { app.initials.as_ref() } else { app.signature.as_ref() };
    match saved {
        Some(SavedSignature::Typed(text)) => {
            draft.tab = SignatureTab::Type;
            draft.text = text.clone();
        }
        Some(SavedSignature::Drawn(strokes)) => {
            draft.tab = SignatureTab::Draw;
            draft.strokes = strokes.clone();
        }
        Some(SavedSignature::Image(image)) => {
            draft.tab = SignatureTab::Image;
            draft.image = Some(image.clone());
        }
        None => {}
    }
    draft.editing = editing;
    let tab = scan::tab(app.host.clone(), app.wake.clone());
    // PLAN: assumed `dialogs::host::open<D: StackDialog>(app: &mut PdfCraftApp, dialog: D) -> DialogId` (pdf-hub-w2).
    crate::dialogs::host::open(app, SignaturePadDialog::new(draft).scan(tab));
}

/// The pad's answer: Apply stores the slot and selects its Fill & Sign tool; Cancel changes nothing.
pub(crate) fn finish(app: &mut PdfCraftApp, result: DialogResult<SignaturePadResult>) {
    let DialogResult::Ok(done) = result else { return };
    let tool = if done.initials {
        app.initials = Some(done.signature);
        FillTool::Initials
    } else {
        app.signature = Some(done.signature);
        FillTool::Signature
    };
    app.fill_tool = tool;
    app.quick_tool = crate::QuickTool::Fill(tool);
}

/// Asks whether to lock the placed signatures into the page before the save `pending` holds. The save waits in
/// `pending_lock` until [`finish_lock`] answers.
pub(crate) fn ask_lock(app: &mut PdfCraftApp, pending: PendingLock) {
    let id = pending.id;
    app.pending_lock = Some(pending);
    let prompt = StandardPrompt::custom(
        tl!("A placed signature is a comment until it is locked: anyone with a PDF editor can move or remove it. Locking makes your Fill & Sign text, marks and signatures part of the page.")
            .to_string(),
    )
    .title(tl!("Lock your signature into the page?").to_string())
    .icon(craft::behavior::IconId::new("lock"))
    .button(PromptChoice::Proceed, tl!("Lock and save").to_string())
    .button(PromptChoice::Alternate, tl!("Keep editable").to_string())
    .button(PromptChoice::Cancel, tl!("Cancel").to_string())
    .dont_show_again()
    .tag(id)
    .instance("lock-signatures");
    app.prompts.lock_signatures = Some(crate::dialogs::host::open(app, prompt));
}

/// The lock question's answer. Lock and Keep editable save as asked, and the box remembers the choice; Cancel drops
/// the save.
pub(crate) fn finish_lock(ctx: &egui::Context, app: &mut PdfCraftApp, answer: PromptAnswer) {
    match answer.choice {
        PromptChoice::Proceed => app.answer_lock_signatures(ctx, true, answer.skip),
        PromptChoice::Alternate => app.answer_lock_signatures(ctx, false, answer.skip),
        _ => app.pending_lock = None,
    }
}
