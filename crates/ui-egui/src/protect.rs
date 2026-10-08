//! Protect Using Password (Acrobat's simplified dialog, screenshot 13b), with the classic
//! Password Security settings behind "Advanced Options" (compatibility, what to encrypt,
//! printing, changes, copying, screen readers). Execution plan M8.1.

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
    pub protection: Protection,
}

impl Default for ProtectDraft {
    fn default() -> Self {
        Self { viewing: true, password: String::new(), confirm: String::new(), advanced: false, protection: Protection::default() }
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

/// A rough password strength, as Acrobat's meter shows (Weak / Medium / Strong).
pub fn strength(pw: &str) -> (&'static str, Color32) {
    let classes = [
        pw.chars().any(|c| c.is_lowercase()),
        pw.chars().any(|c| c.is_uppercase()),
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
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, info.clone()));
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
    ui.label(tl!("Requires user to enter a password for:"));
    ui.add_space(4.0);
    if radio(ui, t, d.viewing, "Viewing").clicked() {
        d.viewing = true;
    }
    if radio(ui, t, !d.viewing, "Editing").clicked() {
        d.viewing = false;
    }
    ui.add_space(10.0);
    // Bordered fields (Acrobat: 1 pt #B1B1B1, radius 4).
    let field = |ui: &mut egui::Ui, label: &str, text: &mut String, id: &str| {
        let l = ui.label(tl!(label));
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, t.border))
            .corner_radius(egui::CornerRadius::same(4))
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.add(egui::TextEdit::singleline(text).password(true).desired_width(290.0).frame(egui::Frame::NONE).id_salt(id)).labelled_by(l.id)
            })
            .inner
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
            egui::ComboBox::from_id_salt("protect-algo").width(300.0).selected_text(cur).show_ui(ui, |ui| {
                for (a, l) in algos {
                    ui.selectable_value(&mut p.algorithm, a, l);
                }
            });
            ui.end_row();
            ui.label(tl!("Encrypt"));
            ui.vertical(|ui| {
                ui.radio_value(&mut p.encrypt_metadata, true, tl!("All document contents"));
                // Unencrypted metadata needs crypt filters (Acrobat 6.0 and later).
                ui.add_enabled_ui(p.algorithm != A::Rc4_40 && p.algorithm != A::Rc4_128, |ui| {
                    ui.radio_value(&mut p.encrypt_metadata, false, tl!("All document contents except metadata"));
                });
            });
            ui.end_row();
            if !d.viewing {
                ui.label(tl!("Printing allowed"));
                let opts =
                    [(Printing::None, tl!("None")), (Printing::Low, tl!("Low Resolution (150 dpi)")), (Printing::High, tl!("High Resolution"))];
                let cur = opts.iter().find(|(o, _)| *o == p.printing).map_or(tl!("High Resolution"), |(_, l)| *l);
                egui::ComboBox::from_id_salt("protect-print").width(300.0).selected_text(cur).show_ui(ui, |ui| {
                    for (o, l) in opts {
                        ui.selectable_value(&mut p.printing, o, l);
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
                egui::ComboBox::from_id_salt("protect-changes").width(300.0).selected_text(cur).show_ui(ui, |ui| {
                    for (o, l) in opts {
                        ui.selectable_value(&mut p.changes, o, l);
                    }
                });
                ui.end_row();
                ui.label("");
                ui.vertical(|ui| {
                    ui.checkbox(&mut p.copy, tl!("Enable copying of text, images, and other content"));
                    ui.checkbox(&mut p.accessibility, tl!("Enable text access for screen reader devices for the visually impaired"));
                });
                ui.end_row();
            }
        });
        if p.algorithm == A::Rc4_40 || p.algorithm == A::Rc4_128 {
            p.encrypt_metadata = true;
        }
    }
    ui.add_space(8.0);
    if let Some(problem) = d.problem().filter(|_| !d.password.is_empty() || !d.confirm.is_empty()) {
        // problem() stays English (tested); only the display translates.
        ui.label(egui::RichText::new(tl!(problem)).color(t.text_muted).font(theme::regular(12.0)));
    }
    ui.label(
        egui::RichText::new(if d.viewing {
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
        assert_eq!(strength("abc").0, "Weak");
        assert_eq!(strength("Abcdefg1").0, "Medium");
        assert_eq!(strength("Correct-Horse-9").0, "Strong");
    }
}
