//! Protect Using Password (Acrobat's simplified dialog, screenshot 13b), with the classic
//! Password Security settings behind "Advanced Options" (compatibility, what to encrypt,
//! printing, changes, copying, screen readers). Execution plan M8.1.

use craft::ui::choice::{Checkbox, Radio, RadioGroup};
use craft::ui::contract::{Bind, Entry, Props as _, Show as _, ShowContainer as _};
use craft::ui::controls::disclosure::Disclosure;
use craft::ui::dialog::{DialogId, DialogResult, DialogStack, StackDialog, dialog_id};
use craft::ui::display::{Shade, Text};
use craft::ui::form::Field;
use craft::ui::layout_components::Row;
use craft::ui::patterns::form_dialogs::{FormDialog, PasswordFields};
use craft::ui::pickers::Select;
use craft::ui::runtime::env::Label;
use craft::ui::style::TypeRole;
use egui::Ui;
use pdfcraft_engine::{Algorithm, Changes, Edit, Printing, Protection};

use crate::PdfCraftApp;

/// The dialog's state while it is open.
#[derive(Clone, Debug, PartialEq)]
pub struct ProtectDraft {
    /// `true`: password required for Viewing; `false`: for Editing.
    pub viewing: bool,
    pub password: String,
    pub confirm: String,
    pub advanced: bool,
    pub protection: Protection,
}

impl Default for ProtectDraft {
    fn default() -> Self {
        Self { viewing: true, password: String::new(), confirm: String::new(), advanced: false, protection: Protection::default() }
    }
}

impl Drop for ProtectDraft {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        self.password.zeroize();
        self.confirm.zeroize();
        if let Some(pw) = &mut self.protection.open_password {
            pw.zeroize();
        }
        if let Some(pw) = &mut self.protection.permissions_password {
            pw.zeroize();
        }
    }
}

impl ProtectDraft {
    /// Why Apply is disabled, if it is.
    pub fn problem(&self) -> Option<&'static str> {
        if self.password.is_empty() {
            Some("Type a password.")
        } else if self.password != self.confirm {
            Some("The passwords don't match.")
        } else if self.protection.algorithm != pdfcraft_engine::Algorithm::Aes256 && !self.password.chars().all(|c| (' '..='~').contains(&c)) {
            Some("This compatibility level supports only plain ASCII passwords.")
        } else {
            None
        }
    }

    /// The edit Apply makes.
    pub fn edit(&self) -> Edit {
        let mut p = self.protection.clone();
        if self.viewing {
            p.open_password = Some(self.password.clone());
            p.permissions_password = None;
        } else {
            p.open_password = None;
            p.permissions_password = Some(self.password.clone());
        }
        Edit::Protect(p)
    }
}

/// Protect Using Password, owned by the dialog host. The draft, passwords included, dies with the dialog.
pub(crate) struct ProtectDialog {
    id: DialogId,
    draft: ProtectDraft,
}

impl ProtectDialog {
    fn new() -> Self {
        ProtectDialog { id: dialog_id("protect"), draft: ProtectDraft::default() }
    }
}

fn text(s: &str) -> String {
    tl!(s).to_string()
}

/// The dialog's rows; returns whether Apply can be pressed.
fn body(ui: &mut Ui, d: &mut ProtectDraft) -> bool {
    let error = d.problem().filter(|_| !d.password.is_empty() || !d.confirm.is_empty()).map(|p| Label::from(text(p)));
    let requires = text("Requires user to enter a password for:");
    Field::new(requires.clone()).show(ui, |ui, _| {
        RadioGroup::new(requires.clone(), Bind::bound(&mut d.viewing)).show(ui, |ui, _| {
            Radio::new(true).label(text("Viewing")).show(ui);
            Radio::new(false).label(text("Editing")).show(ui);
        });
    });
    PasswordFields::new(text("Type Password"), Bind::bound(&mut d.password)).confirm(Bind::bound(&mut d.confirm)).error(error).show(ui);
    let advanced = text("Advanced Options");
    Row::new().show(ui, |ui, _| {
        Disclosure::new(Bind::bound(&mut d.advanced)).label(advanced.clone()).show(ui);
        Text::new(advanced.clone()).show(ui);
    });
    if d.advanced {
        advanced_rows(ui, d);
    }
    let note = if d.viewing {
        text("Anyone opening the document will need this password. It's applied when you save.")
    } else {
        text("The document opens without a password; this password is needed to change the restrictions. It's applied when you save.")
    };
    Text::new(note).role(TypeRole::Caption).shade(Shade::Muted).show(ui);
    d.problem().is_none()
}

/// The classic Password Security settings.
fn advanced_rows(ui: &mut Ui, d: &mut ProtectDraft) {
    use Algorithm as A;
    let viewing = d.viewing;
    let p = &mut d.protection;
    let algos = [
        Entry::new(A::Aes256, text("Acrobat X and later (256-bit AES)")),
        Entry::new(A::Aes128, text("Acrobat 7.0 and later (128-bit AES)")),
        Entry::new(A::Rc4_128, text("Acrobat 5.0 and later (128-bit RC4)")),
        Entry::new(A::Rc4_40, text("Acrobat 3.0 and later (40-bit RC4)")),
    ];
    let label = text("Compatibility");
    Field::new(label.clone()).show(ui, |ui, _| {
        if let Some(a) = Select::new(label.clone(), Bind::given(p.algorithm), &algos).show(ui).value().copied() {
            p.algorithm = a;
        }
    });
    // Unencrypted metadata needs crypt filters (Acrobat 6.0 and later), which RC4 lacks.
    let rc4 = p.algorithm == A::Rc4_40 || p.algorithm == A::Rc4_128;
    if rc4 {
        p.encrypt_metadata = true;
    }
    let mut what = vec![Entry::new(true, text("All document contents"))];
    if !rc4 {
        what.push(Entry::new(false, text("All document contents except metadata")));
    }
    let label = text("Encrypt");
    Field::new(label.clone()).show(ui, |ui, _| {
        if let Some(all) = Select::new(label.clone(), Bind::given(p.encrypt_metadata), &what).show(ui).value().copied() {
            p.encrypt_metadata = all;
        }
    });
    if viewing {
        return;
    }
    let printing = [
        Entry::new(Printing::None, text("None")),
        Entry::new(Printing::Low, text("Low Resolution (150 dpi)")),
        Entry::new(Printing::High, text("High Resolution")),
    ];
    let label = text("Printing allowed");
    Field::new(label.clone()).show(ui, |ui, _| {
        if let Some(v) = Select::new(label.clone(), Bind::given(p.printing), &printing).show(ui).value().copied() {
            p.printing = v;
        }
    });
    let changes = [
        Entry::new(Changes::None, text("None")),
        Entry::new(Changes::Pages, text("Inserting, deleting, and rotating pages")),
        Entry::new(Changes::FillSign, text("Filling in form fields and signing existing signature fields")),
        Entry::new(Changes::CommentFillSign, text("Commenting, filling in form fields, and signing existing signature fields")),
        Entry::new(Changes::AnyExceptExtract, text("Any except extracting pages")),
    ];
    let label = text("Changes allowed");
    Field::new(label.clone()).show(ui, |ui, _| {
        if let Some(v) = Select::new(label.clone(), Bind::given(p.changes), &changes).show(ui).value().copied() {
            p.changes = v;
        }
    });
    Checkbox::new(Bind::bound(&mut p.copy)).label(text("Enable copying of text, images, and other content")).show(ui);
    Checkbox::new(Bind::bound(&mut p.accessibility)).label(text("Enable text access for screen reader devices for the visually impaired")).show(ui);
}

impl StackDialog for ProtectDialog {
    type Result = ProtectDraft;

    fn id(&self) -> DialogId {
        self.id
    }

    fn show(&mut self, ui: &mut Ui, _: &DialogStack) -> Option<DialogResult<ProtectDraft>> {
        let problem = self.draft.problem();
        let mut form = FormDialog::new(text("Protect Using Password"), Bind::bound(&mut self.draft)).ok_label(text("Apply"));
        if let Some(problem) = problem {
            form = form.ok_reason(text(problem));
        }
        let shown = form.show(ui, |ui, _, draft| body(ui, draft));
        match (shown.accepted, shown.closed) {
            (Some(draft), _) => Some(DialogResult::Ok(draft)),
            (None, Some(_)) => Some(DialogResult::Cancel),
            (None, None) => None,
        }
    }
}

impl PdfCraftApp {
    /// Opens Protect Using Password with an empty draft.
    pub(crate) fn open_protect(&mut self) {
        // PLAN: assumed `fn open<D: StackDialog>(app: &mut PdfCraftApp, dialog: D)` (pdf-hub-w2)
        crate::dialogs::host::open(self, ProtectDialog::new());
    }

    /// Applies the draft of a Protect dialog that finished with Apply.
    pub(crate) fn protect_pass(&mut self, ctx: &egui::Context) {
        // PLAN: assumed `self.dialogs: DialogHost` with `fn take<D: StackDialog>(&mut self) -> Option<DialogResult<D::Result>>` (pdf-hub-w1 field, pdf-hub-w2 host)
        if let Some(DialogResult::Ok(draft)) = self.dialogs.take::<ProtectDialog>()
            && self.apply_edit(ctx, draft.edit())
        {
            self.notify_tr("Password protection will be applied when you save");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn drafts_validate_and_map_to_the_right_password() {
        let mut d = ProtectDraft::default();
        assert!(d.problem().is_some());
        d.password = "abc".into();
        d.confirm = "abd".into();
        assert_eq!(d.problem(), Some("The passwords don't match."));
        d.confirm = "abc".into();
        assert_eq!(d.problem(), None);
        match d.edit() {
            Edit::Protect(p) => assert_eq!((p.open_password.as_deref(), p.permissions_password), (Some("abc"), None)),
            other => panic!("{other:?}"),
        }
        d.viewing = false;
        match d.edit() {
            Edit::Protect(p) => assert_eq!((p.open_password, p.permissions_password.as_deref()), (None, Some("abc"))),
            other => panic!("{other:?}"),
        }
    }

    /// Types into the password field `label`: its input, not the field group or its label that share the name.
    fn type_into(h: &mut Harness<'static, PdfCraftApp>, label: &str, text: &str) {
        use egui::accesskit::Role;
        h.get_by(|n| n.label().as_deref() == Some(label) && matches!(n.role(), Role::PasswordInput | Role::TextInput)).click();
        h.run_steps(2);
        h.get_by(|n| n.label().as_deref() == Some(label) && matches!(n.role(), Role::PasswordInput | Role::TextInput)).type_text(text);
        h.run_steps(2);
    }

    fn open_and_fill(h: &mut Harness<'static, PdfCraftApp>, password: &str) {
        h.state_mut().open_protect();
        h.run_steps(2);
        type_into(h, "Type Password", password);
        type_into(h, "Re-type Password", password);
        assert!(h.query_by(|n| n.label().as_deref() == Some("Apply") && !n.is_disabled()).is_some(), "matching passwords enable Apply");
    }

    fn assert_fresh(h: &mut Harness<'static, PdfCraftApp>) {
        h.state_mut().open_protect();
        h.run_steps(2);
        assert!(
            h.query_by(|n| n.label().as_deref() == Some("Apply") && n.is_disabled()).is_some(),
            "a reopened dialog starts with empty passwords, so Apply is disabled"
        );
    }

    #[test]
    fn closing_the_protect_dialog_clears_the_password_draft() {
        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|cc| {
            crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
            {
                let mut app = PdfCraftApp::for_unit_tests();
                app.set_option(&cc.egui_ctx, "language", "en").unwrap();
                app
            }
        });

        open_and_fill(&mut h, "TopSecret123!");
        h.get_by_label("Cancel").click();
        h.run_steps(2);
        assert!(h.query_by_label("Apply").is_none(), "Cancel closes the dialog");
        assert_fresh(&mut h);

        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        assert!(h.query_by_label("Apply").is_none(), "Escape closes the dialog");

        open_and_fill(&mut h, "AnotherSecret!");
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        assert!(h.query_by_label("Apply").is_none());
        assert_fresh(&mut h);
    }
}
