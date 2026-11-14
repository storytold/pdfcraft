//! Protect Using Password (Acrobat's simplified dialog, screenshot 13b), with the classic
//! Password Security settings behind "Advanced Options" (compatibility, what to encrypt, the other
//! password, printing, changes, copying, screen readers). Execution plan M8.1.
//!
//! As the Acrobat user guide's Password Security settings describe them: a document can have an
//! open password and a permissions password together ("setting both types of passwords is often
//! beneficial"); with 40-bit RC4 the options it says need 128-bit encryption are unavailable; and
//! a document that already has security asks first whether to change it.

use egui::{Align, Color32, Layout};
use pdfcraft_engine::{Changes, Edit, Printing, Protection};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

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
    /// The document already has security and changing it has not been confirmed yet: the dialog
    /// asks first ("If you receive a prompt, click Yes to change the security").
    pub ask_change: bool,
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
            ask_change: false,
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
        } else if self.both && self.second.is_empty() {
            Some(if self.viewing { "Type a permissions password." } else { "Type a document open password." })
        } else if self.both && self.second != self.second_confirm {
            Some(if self.viewing { "The permissions passwords don't match." } else { "The document open passwords don't match." })
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
        if p.algorithm != pdfcraft_engine::Algorithm::Rc4_40 {
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

/// A rough password strength, as Acrobat's meter shows (Weak / Medium / Strong).
pub fn strength(pw: &str) -> (&'static str, Color32) {
    let classes = [
        pw.chars().any(char::is_lowercase),
        pw.chars().any(char::is_uppercase),
        pw.chars().any(|c| c.is_ascii_digit()),
        pw.chars().any(|c| !c.is_alphanumeric()),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    let n = pw.chars().count();
    if n >= 12 && classes >= 3 {
        ("Strong", Color32::from_rgb(0x2D, 0x9D, 0x5B))
    } else if n >= 8 && classes >= 2 {
        ("Medium", Color32::from_rgb(0xE8, 0x8A, 0x1A))
    } else {
        ("Weak", Color32::from_rgb(0xD3, 0x2F, 0x2F))
    }
}

/// A radio button drawn like Acrobat's: an outlined circle, filled blue with a white dot when on.
fn radio(ui: &mut egui::Ui, t: &Tokens, on: bool, label: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(220.0, 26.0), egui::Sense::click());
    let c = egui::pos2(rect.left() + 9.0, rect.center().y);
    if on {
        ui.painter().circle_filled(c, 8.0, t.accent);
        ui.painter().circle_filled(c, 3.0, Color32::WHITE);
    } else {
        ui.painter().circle_stroke(c, 7.5, egui::Stroke::new(1.5, t.text_muted));
    }
    ui.painter().text(egui::pos2(rect.left() + 26.0, rect.center().y), egui::Align2::LEFT_CENTER, tl!(label), theme::regular(14.0), t.text);
    let info = tl!(label).to_string();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, info.clone()));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Draw the dialog body; returns (apply, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    use pdfcraft_engine::Algorithm as A;
    let d = &mut app.protect_draft;
    ui.label(egui::RichText::new(tl!("Protect Using Password")).font(theme::semibold(18.0)));
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(6.0);
    if d.ask_change {
        ui.label(tl!("This document already has security settings. Do you want to change them?"));
        ui.add_space(12.0);
        let mut cancel = false;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Yes"), true).clicked() {
                d.ask_change = false;
            }
            if widgets::pill_button(ui, tl!("No"), false).clicked() {
                cancel = true;
            }
        });
        return (false, cancel);
    }
    ui.label(tl!("Requires user to enter a password for:"));
    ui.add_space(4.0);
    if radio(ui, t, d.viewing, "Viewing").clicked() {
        d.viewing = true;
    }
    if radio(ui, t, !d.viewing, "Editing").clicked() {
        d.viewing = false;
    }
    ui.add_space(10.0);
    let field = |ui: &mut egui::Ui, label: &str, text: &mut String, id: &str| {
        let l = ui.label(tl!(label));
        ui.add(crate::widgets::line(text).password(true).desired_width(290.0).id_salt(id)).labelled_by(l.id)
    };
    let r = field(ui, "Type Password", &mut d.password, "protect-pw");
    if r.changed() || !d.password.is_empty() {
        let (s, c) = strength(&d.password);
        if !d.password.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Strength: {s}"), &[("s", tl!(s))])).font(theme::medium(11.5)).color(c));
        }
    }
    ui.add_space(6.0);
    field(ui, "Re-type Password", &mut d.confirm, "protect-pw2");
    ui.add_space(6.0);
    let chevron = if d.advanced { "⌃" } else { "⌄" };
    if ui
        .add(
            egui::Button::new(
                egui::RichText::new(crate::i18n::fmt(tl!("Advanced Options {chevron}"), &[("chevron", chevron)])).font(theme::semibold(13.0)),
            )
            .frame(false),
        )
        .clicked()
    {
        d.advanced = !d.advanced;
    }
    if d.advanced {
        let p = &mut d.protection;
        egui::Grid::new("protect-advanced").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(tl!("Compatibility"));
            let algos = [
                (A::Aes256, tl!("Acrobat X and later (256-bit AES)")),
                (A::Aes128, tl!("Acrobat 7.0 and later (128-bit AES)")),
                (A::Rc4_128, tl!("Acrobat 5.0 and later (128-bit RC4)")),
                (A::Rc4_40, tl!("Acrobat 3.0 and later (40-bit RC4)")),
            ];
            let cur = algos.iter().find(|(a, _)| *a == p.algorithm).map_or(algos[0].1, |(_, l)| *l);
            craft_ui::ComboBox::from_id_salt("protect-algo").width(300.0).selected_text(cur).show_ui(ui, |ui| {
                for (a, l) in algos {
                    ui.selectable_value(&mut p.algorithm, a, l);
                }
            });
            ui.end_row();
            ui.label(tl!("Encrypt"));
            ui.vertical(|ui| {
                widgets::radio_value(ui, &mut p.encrypt_metadata, true, tl!("All document contents"));
                // Unencrypted metadata needs crypt filters (Acrobat 6.0 and later).
                ui.add_enabled_ui(p.algorithm != A::Rc4_40 && p.algorithm != A::Rc4_128, |ui| {
                    widgets::radio_value(ui, &mut p.encrypt_metadata, false, tl!("All document contents except metadata"));
                });
            });
            ui.end_row();
            // The other password, as the full Password Security settings offer both at once.
            ui.label("");
            ui.vertical(|ui| {
                let other =
                    if d.viewing { tl!("Restrict editing and printing of the document") } else { tl!("Require a password to open the document") };
                ui.add(widgets::checkbox(&mut d.both, other));
                if d.both {
                    let (pw, again) = if d.viewing {
                        (tl!("Change Permissions Password"), tl!("Re-type Permissions Password"))
                    } else {
                        (tl!("Document Open Password"), tl!("Re-type Document Open Password"))
                    };
                    let l = ui.label(pw);
                    ui.add(crate::widgets::line(&mut d.second).password(true).desired_width(290.0).id_salt("protect-second")).labelled_by(l.id);
                    let l = ui.label(again);
                    ui.add(crate::widgets::line(&mut d.second_confirm).password(true).desired_width(290.0).id_salt("protect-second2"))
                        .labelled_by(l.id);
                }
            });
            ui.end_row();
            // Only high (128-bit RC4 or AES) encryption has low-resolution printing, page
            // changes, form filling and screen-reader access on their own.
            let high = p.algorithm != A::Rc4_40;
            if !d.viewing || d.both {
                ui.label(tl!("Printing allowed"));
                let opts =
                    [(Printing::None, tl!("None")), (Printing::Low, tl!("Low Resolution (150 dpi)")), (Printing::High, tl!("High Resolution"))];
                let cur = opts.iter().find(|(o, _)| *o == p.printing).map_or(tl!("High Resolution"), |(_, l)| *l);
                craft_ui::ComboBox::from_id_salt("protect-print").width(300.0).selected_text(cur).show_ui(ui, |ui| {
                    for (o, l) in opts {
                        ui.add_enabled_ui(high || o != Printing::Low, |ui| ui.selectable_value(&mut p.printing, o, l));
                    }
                });
                ui.end_row();
                ui.label(tl!("Changes allowed"));
                let opts = [
                    (Changes::None, tl!("None")),
                    (Changes::Pages, tl!("Inserting, deleting, and rotating pages")),
                    (Changes::FillSign, tl!("Filling in form fields and signing existing signature fields")),
                    (Changes::CommentFillSign, tl!("Commenting, filling in form fields, and signing existing signature fields")),
                    (Changes::AnyExceptExtract, tl!("Any except extracting pages")),
                ];
                let cur = opts.iter().find(|(o, _)| *o == p.changes).map_or(tl!("None"), |(_, l)| *l);
                craft_ui::ComboBox::from_id_salt("protect-changes").width(300.0).selected_text(cur).show_ui(ui, |ui| {
                    for (o, l) in opts {
                        ui.add_enabled_ui(high || !matches!(o, Changes::Pages | Changes::FillSign), |ui| ui.selectable_value(&mut p.changes, o, l));
                    }
                });
                ui.end_row();
                ui.label("");
                ui.vertical(|ui| {
                    ui.add(widgets::checkbox(&mut p.copy, tl!("Enable copying of text, images, and other content")));
                    ui.add_enabled(
                        high,
                        widgets::checkbox(&mut p.accessibility, tl!("Enable text access for screen reader devices for the visually impaired")),
                    );
                });
                ui.end_row();
            }
        });
        if p.algorithm == A::Rc4_40 || p.algorithm == A::Rc4_128 {
            p.encrypt_metadata = true;
        }
        d.keep_to_compatibility();
    }
    ui.add_space(8.0);
    if let Some(problem) = d.problem().filter(|_| !d.password.is_empty() || !d.confirm.is_empty()) {
        // problem() stays English (tested); only the display translates.
        ui.label(egui::RichText::new(tl!(problem)).color(t.text_muted).font(theme::regular(12.0)));
    }
    ui.label(
        egui::RichText::new(if d.both {
            tl!("The document needs its open password to open, and its permissions password to change the restrictions. Both are applied when you save.")
        } else if d.viewing {
            tl!("Anyone opening the document will need this password. It's applied when you save.")
        } else {
            tl!("The document opens without a password; this password is needed to change the restrictions. It's applied when you save.")
        })
        .color(t.text_faint)
        .font(theme::regular(11.5)),
    );
    ui.add_space(12.0);
    let (mut apply, mut cancel) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let ok = d.problem().is_none();
        if ui.add_enabled_ui(ok, |ui| widgets::pill_button(ui, tl!("Apply"), true)).inner.clicked() {
            apply = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (apply, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(strength("abc").0, "Weak");
        assert_eq!(strength("Abcdefg1").0, "Medium");
        assert_eq!(strength("Correct-Horse-9").0, "Strong");
    }

    #[test]
    fn closing_the_protect_dialog_clears_the_password_draft() {
        use egui_kittest::{Harness, kittest::Queryable};

        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.dialog = Some(crate::Dialog::Protect);
            app.protect_draft.password = "TopSecret123!".into();
            app.protect_draft.confirm = "TopSecret123!".into();
            app
        });
        h.run_steps(2);

        // Clicking Cancel closes Dialog::Protect and clears the draft passwords.
        h.get_by_label("Cancel").click();
        h.run_steps(2);
        assert_eq!(h.state().dialog, None);
        assert!(h.state().protect_draft.password.is_empty(), "password must not linger after cancelling Dialog::Protect");
        assert!(h.state().protect_draft.confirm.is_empty(), "confirm must not linger after cancelling Dialog::Protect");

        // Pressing Escape on Dialog::Protect also clears the draft passwords.
        {
            let app = h.state_mut();
            app.dialog = Some(crate::Dialog::Protect);
            app.protect_draft.password = "AnotherSecret!".into();
            app.protect_draft.confirm = "AnotherSecret!".into();
        }
        h.run_steps(2);
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        assert_eq!(h.state().dialog, None);
        assert!(h.state().protect_draft.password.is_empty(), "password must not linger after closing Dialog::Protect");
        assert!(h.state().protect_draft.confirm.is_empty(), "confirm must not linger after closing Dialog::Protect");
    }
}
