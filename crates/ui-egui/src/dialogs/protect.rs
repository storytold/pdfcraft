//! Protect Using Password (Acrobat's simplified dialog, screenshot 13b), with the classic
//! Password Security settings behind "Advanced Options" (compatibility, what to encrypt,
//! printing, changes, copying, screen readers). Execution plan M8.1.
//!
//! Advanced Options can also set the other password so an open password and a permissions
//! password can be configured together.

use craft::theme::variant::Tone;
use craft::ui::choice::{Checkbox, Radio, RadioGroup};
use craft::ui::contract::{Bind, Entry, Props as _, Show as _, ShowContainer as _};
use craft::ui::controls::disclosure::Disclosure;
use craft::ui::dialog::{DialogId, DialogResult, DialogStack, StackDialog, dialog_id};
use craft::ui::display::{Shade, Text};
use craft::ui::form::{Field, Help};
use craft::ui::layout_components::Row;
use craft::ui::patterns::form_dialogs::{FormDialog, PasswordFields};
use craft::ui::pickers::Select;
use craft::ui::runtime::env::Label;
use craft::ui::style::TypeRole;
use craft::ui::text::TextField;
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
    /// Advanced Options: also set the other password — a permissions password when the password
    /// above is for Viewing, an open password when it is for Editing.
    pub both: bool,
    /// The other password and its confirmation (used only with `both`).
    pub second: String,
    pub second_confirm: String,
    pub protection: Protection,
}

impl Default for ProtectDraft {
    fn default() -> Self {
        Self {
            viewing: true,
            password: String::new(),
            confirm: String::new(),
            advanced: false,
            both: false,
            second: String::new(),
            second_confirm: String::new(),
            protection: Protection::default(),
        }
    }
}

impl Drop for ProtectDraft {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        self.password.zeroize();
        self.confirm.zeroize();
        self.second.zeroize();
        self.second_confirm.zeroize();
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
        let ascii_only = |pw: &str| self.protection.algorithm != pdfcraft_engine::Algorithm::Aes256 && !pw.chars().all(|c| (' '..='~').contains(&c));
        if self.password.is_empty() {
            Some("Type a password.")
        } else if self.password != self.confirm {
            Some("The passwords don't match.")
        } else if ascii_only(&self.password) || (self.both && ascii_only(&self.second)) {
            Some("This compatibility level supports only plain ASCII passwords.")
        } else if self.same_to_the_file(&self.password, "") {
            Some("The password is made only of characters that passwords ignore, such as soft hyphens.")
        } else if self.both && self.second.is_empty() {
            Some(if self.viewing { "Type a permissions password." } else { "Type a document open password." })
        } else if self.both && self.second != self.second_confirm {
            Some(if self.viewing { "The permissions passwords don't match." } else { "The document open passwords don't match." })
        } else if self.both && self.same_to_the_file(&self.second, "") {
            Some("The second password is made only of characters that passwords ignore, such as soft hyphens.")
        } else if self.both && self.same_to_the_file(&self.second, &self.password) {
            Some("The document open password and the permissions password must be different.")
        } else {
            None
        }
    }

    /// Whether the file would take `a` and `b` as one password: encryption keys on a password as
    /// [`pdfcraft_cos::effective_password`] gives it (128-bit AES and RC4 keep its first 32
    /// bytes, 256-bit AES its SASLprep), and the engine refuses two that come out the same. Checked
    /// here so Apply stays disabled and nothing typed is lost.
    fn same_to_the_file(&self, a: &str, b: &str) -> bool {
        let algorithm = self.protection.algorithm;
        pdfcraft_cos::effective_password(algorithm, a) == pdfcraft_cos::effective_password(algorithm, b)
    }

    /// Whether the edit restricts printing, changes and copying (a permissions password is set).
    pub fn restricts(&self) -> bool {
        !self.viewing || self.both
    }

    /// The edit Apply makes.
    pub fn edit(&self) -> Edit {
        let mut p = self.protection.clone();
        let (open, permissions) = match (self.viewing, self.both) {
            (true, false) => (Some(self.password.clone()), None),
            (false, false) => (None, Some(self.password.clone())),
            (true, true) => (Some(self.password.clone()), Some(self.second.clone())),
            (false, true) => (Some(self.second.clone()), Some(self.password.clone())),
        };
        p.open_password = open;
        p.permissions_password = permissions;
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
    let note = if d.both {
        text("The document needs its open password to open, and its permissions password to change the restrictions. Both are applied when you save.")
    } else if d.viewing {
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
    let restricts = d.restricts();
    let algos = [
        Entry::new(A::Aes256, text("Acrobat X and later (256-bit AES)")),
        Entry::new(A::Aes128, text("Acrobat 7.0 and later (128-bit AES)")),
        Entry::new(A::Rc4_128, text("Acrobat 5.0 and later (128-bit RC4)")),
        Entry::new(A::Rc4_40, text("Acrobat 3.0 and later (40-bit RC4)")),
    ];
    let label = text("Compatibility");
    Field::new(label.clone()).show(ui, |ui, _| {
        if let Some(a) = Select::new(label.clone(), Bind::given(d.protection.algorithm), &algos).show(ui).value().copied() {
            d.protection.algorithm = a;
        }
    });
    // Unencrypted metadata needs crypt filters (Acrobat 6.0 and later), which RC4 lacks.
    let rc4 = d.protection.algorithm == A::Rc4_40 || d.protection.algorithm == A::Rc4_128;
    if rc4 {
        d.protection.encrypt_metadata = true;
    }
    let mut what = vec![Entry::new(true, text("All document contents"))];
    if !rc4 {
        what.push(Entry::new(false, text("All document contents except metadata")));
    }
    let label = text("Encrypt");
    Field::new(label.clone()).show(ui, |ui, _| {
        if let Some(all) = Select::new(label.clone(), Bind::given(d.protection.encrypt_metadata), &what).show(ui).value().copied() {
            d.protection.encrypt_metadata = all;
        }
    });
    let both_label = text(if d.viewing { "Restrict editing and printing of the document" } else { "Require a password to open the document" });
    Checkbox::new(Bind::bound(&mut d.both)).label(both_label).show(ui);
    if d.both {
        let (first_label, second_label) = if d.viewing {
            (text("Change Permissions Password"), text("Re-type Permissions Password"))
        } else {
            (text("Document Open Password"), text("Re-type Document Open Password"))
        };
        let second_error = d.problem().filter(|_| !d.second.is_empty() || !d.second_confirm.is_empty()).map(|p| Label::from(text(p)));
        let mut first_field = Field::new(first_label.clone());
        if let Some(err) = second_error {
            first_field = first_field.help(Help::Status(Tone::Bad, err));
        }
        first_field.show(ui, |ui, _| {
            TextField::new(first_label.clone(), Bind::bound(&mut d.second)).masked().show(ui);
        });
        let mismatch = (!d.second_confirm.is_empty() && d.second != d.second_confirm).then(|| Label::from(text("The passwords don't match.")));
        let mut second_field = Field::new(second_label.clone());
        if let Some(mismatch) = mismatch {
            second_field = second_field.help(Help::Status(Tone::Bad, mismatch));
        }
        second_field.show(ui, |ui, _| {
            TextField::new(second_label.clone(), Bind::bound(&mut d.second_confirm)).masked().show(ui);
        });
    }
    if !restricts {
        return;
    }
    let p = &mut d.protection;
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
    if p.algorithm == A::Rc4_40 && p.printing == Printing::Low {
        p.printing = Printing::None;
    }
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
        // Advanced Options can set the other password too.
        d.both = true;
        assert_eq!(d.problem(), Some("Type a document open password."));
        d.second = "open".into();
        d.second_confirm = "opem".into();
        assert_eq!(d.problem(), Some("The document open passwords don't match."));
        d.second_confirm = "open".into();
        assert_eq!(d.problem(), None);
        match d.edit() {
            Edit::Protect(p) => assert_eq!((p.open_password.as_deref(), p.permissions_password.as_deref()), (Some("open"), Some("abc"))),
            other => panic!("{other:?}"),
        }
        d.viewing = true;
        assert_eq!(d.problem(), None);
        match d.edit() {
            Edit::Protect(p) => assert_eq!((p.open_password.as_deref(), p.permissions_password.as_deref()), (Some("abc"), Some("open"))),
            other => panic!("{other:?}"),
        }
        d.second = "abc".into();
        d.second_confirm = "abc".into();
        assert_eq!(d.problem(), Some("The document open password and the permissions password must be different."));
        // 128-bit AES keeps a password's first 32 bytes: two that differ only after them are one
        // password to the file, which the engine refuses, so Apply stays disabled.
        let a32 = "a".repeat(32);
        d.protection.algorithm = pdfcraft_engine::Algorithm::Aes128;
        (d.password, d.confirm) = (format!("{a32}1"), format!("{a32}1"));
        (d.second, d.second_confirm) = (format!("{a32}2"), format!("{a32}2"));
        assert_eq!(d.problem(), Some("The document open password and the permissions password must be different."));
        d.protection.algorithm = pdfcraft_engine::Algorithm::Aes256;
        assert_eq!(d.problem(), None, "256-bit AES keeps them apart");
        // Soft hyphens (U+00AD) are dropped by SASLprep, resulting in an empty effective password.
        d.second = "\u{00AD}".into();
        d.second_confirm = "\u{00AD}".into();
        assert_eq!(d.problem(), Some("The second password is made only of characters that passwords ignore, such as soft hyphens."));
        // The primary password must also reject an empty effective password in both viewing and editing roles.
        d.both = false;
        d.viewing = true;
        d.password = "\u{00AD}".into();
        d.confirm = "\u{00AD}".into();
        assert_eq!(d.problem(), Some("The password is made only of characters that passwords ignore, such as soft hyphens."));
        d.viewing = false;
        assert_eq!(d.problem(), Some("The password is made only of characters that passwords ignore, such as soft hyphens."));
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
