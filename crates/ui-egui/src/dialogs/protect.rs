//! Protect Using Password (Acrobat's simplified dialog, screenshot 13b), with the classic
//! Password Security settings behind "Advanced Options" (compatibility, what to encrypt, the other
//! password, printing, changes, copying, screen readers). Execution plan M8.1.
//!
//! As the Acrobat user guide's Password Security settings describe them: a document can have an
//! open password and a permissions password together ("setting both types of passwords is often
//! beneficial"); with 40-bit RC4 the options it says need 128-bit encryption are unavailable; and
//! a document that already has security asks first whether to change it.

use craft::ui::choice::{Checkbox, Radio, RadioGroup};
use craft::ui::contract::{Bind, Entry, Props as _, Show as _, ShowContainer as _};
use craft::ui::controls::disclosure::Disclosure;
use craft::ui::dialog::{DialogId, DialogResult, DialogStack, PromptChoice, StackDialog, StandardPrompt, dialog_id};
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
        self.first_problem().or_else(|| self.second_problem())
    }

    /// What is wrong with the password and its re-type, if anything.
    fn first_problem(&self) -> Option<&'static str> {
        if self.password.is_empty() {
            Some("Type a password.")
        } else if self.password != self.confirm {
            Some("The passwords don't match.")
        } else if self.ascii_only(&self.password) {
            Some(ASCII_ONLY)
        } else {
            None
        }
    }

    /// What is wrong with the other password (Advanced Options), if it is set and anything is.
    fn second_problem(&self) -> Option<&'static str> {
        if !self.both {
            None
        } else if self.ascii_only(&self.second) {
            Some(ASCII_ONLY)
        } else if self.second.is_empty() {
            Some(if self.viewing { "Type a permissions password." } else { "Type a document open password." })
        } else if self.second != self.second_confirm {
            Some(if self.viewing { "The permissions passwords don't match." } else { "The document open passwords don't match." })
        } else if self.same_to_the_file(&self.second, &self.password) {
            Some("The document open password and the permissions password must be different.")
        } else {
            None
        }
    }

    /// Whether the compatibility level refuses `pw`: all but 256-bit AES take plain ASCII only.
    fn ascii_only(&self, pw: &str) -> bool {
        self.protection.algorithm != Algorithm::Aes256 && !pw.chars().all(|c| (' '..='~').contains(&c))
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
        let second = self.both.then(|| self.second.clone());
        if self.viewing {
            p.open_password = Some(self.password.clone());
            p.permissions_password = second;
        } else {
            p.open_password = second;
            p.permissions_password = Some(self.password.clone());
        }
        Edit::Protect(p)
    }

    /// Keep the restrictions to what 40-bit RC4 can express: the guide offers low-resolution
    /// printing, page changes, filling in forms and screen-reader access only with 128-bit
    /// encryption (revision 2 has no bits for them, ISO 32000-2 Table 22), and there screen
    /// readers follow copying. A choice it can't keep falls back to the stricter "None".
    pub fn keep_to_compatibility(&mut self) {
        let p = &mut self.protection;
        if p.algorithm != Algorithm::Rc4_40 {
            return;
        }
        if p.printing == Printing::Low {
            p.printing = Printing::None;
        }
        if matches!(p.changes, Changes::Pages | Changes::FillSign) {
            p.changes = Changes::None;
        }
        p.accessibility = p.copy;
    }
}

const ASCII_ONLY: &str = "This compatibility level supports only plain ASCII passwords.";

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
    let error = d.first_problem().filter(|_| !d.password.is_empty() || !d.confirm.is_empty()).map(|p| Label::from(text(p)));
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
    compatibility_rows(ui, &mut d.protection);
    // The other password, as the full Password Security settings offer both at once.
    let other = if d.viewing { text("Restrict editing and printing of the document") } else { text("Require a password to open the document") };
    Checkbox::new(Bind::bound(&mut d.both)).label(other).show(ui);
    if d.both {
        let (pw, again) = if d.viewing {
            (text("Change Permissions Password"), text("Re-type Permissions Password"))
        } else {
            (text("Document Open Password"), text("Re-type Document Open Password"))
        };
        // Said once anything is typed, as for the password above.
        let typed = !d.password.is_empty() || !d.second.is_empty() || !d.second_confirm.is_empty();
        let error = d.second_problem().filter(|_| typed).map(|p| Label::from(text(p)));
        PasswordFields::new(pw, Bind::bound(&mut d.second)).show(ui);
        PasswordFields::new(again, Bind::bound(&mut d.second_confirm)).error(error).show(ui);
    }
    if d.restricts() {
        restriction_rows(ui, &mut d.protection);
    }
    d.keep_to_compatibility();
}

/// Compatibility and what to encrypt.
fn compatibility_rows(ui: &mut Ui, p: &mut Protection) {
    use Algorithm as A;
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
}

/// Printing, changes, copying and screen readers: what a permissions password restricts.
fn restriction_rows(ui: &mut Ui, p: &mut Protection) {
    // Only high (128-bit RC4 or AES) encryption has low-resolution printing, page changes, form
    // filling and screen-reader access on their own.
    let high = p.algorithm != Algorithm::Rc4_40;
    let printing = [
        Entry::new(Printing::None, text("None")),
        Entry::new(Printing::Low, text("Low Resolution (150 dpi)")).enabled(high),
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
        Entry::new(Changes::Pages, text("Inserting, deleting, and rotating pages")).enabled(high),
        Entry::new(Changes::FillSign, text("Filling in form fields and signing existing signature fields")).enabled(high),
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
    Checkbox::new(Bind::bound(&mut p.accessibility))
        .label(text("Enable text access for screen reader devices for the visually impaired"))
        .enabled(high)
        .show(ui);
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
    /// Protect Using Password from the menu: a document that already has security asks first
    /// whether to change it ("If you receive a prompt, click Yes to change the security"); one
    /// without goes straight to the settings.
    pub(crate) fn request_protect(&mut self) {
        let secured = self.active_ids().and_then(|(_, id)| self.session.get(id)).is_some_and(|d| d.security_summary().is_some());
        if !secured {
            self.open_protect();
            return;
        }
        let prompt = StandardPrompt::custom(text("This document already has security settings. Do you want to change them?"))
            .title(text("Protect Using Password"))
            .button(PromptChoice::Proceed, text("Yes"))
            .button(PromptChoice::Cancel, text("No"))
            .instance("change-security");
        self.prompts.change_security = Some(self.dialogs.open(prompt));
    }

    /// Opens Protect Using Password with an empty draft.
    pub(crate) fn open_protect(&mut self) {
        // PLAN: assumed `fn open<D: StackDialog>(app: &mut PdfCraftApp, dialog: D)` (pdf-hub-w2)
        crate::dialogs::host::open(self, ProtectDialog::new());
    }

    /// Applies the draft of a Protect dialog that finished with Apply.
    pub(crate) fn protect_pass(&mut self, ctx: &egui::Context) {
        // The question before changing existing security: Yes goes on to the settings.
        if let Some(dialog) = self.prompts.change_security
            && let Some(answer) = self.dialogs.take_by_id::<StandardPrompt>(dialog)
        {
            self.prompts.change_security = None;
            if matches!(answer, DialogResult::Ok(a) if a.choice == PromptChoice::Proceed) {
                self.open_protect();
            }
        }
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
