use egui::{Align, Color32, Layout};

use crate::PdfCraftApp;
use crate::theme;
use crate::theme::Tokens;

fn account_secondary(account: &pdfcraft_engine::PurviewAccount) -> String {
    if !account.email.trim().is_empty() {
        account.email.trim().to_owned()
    } else if !account.principal_name.trim().is_empty() {
        account.principal_name.trim().to_owned()
    } else {
        String::new()
    }
}

fn account_card(ui: &mut egui::Ui, app: &mut PdfCraftApp, tokens: &Tokens) {
    let active = app.session.purview_account();
    let accounts = app.session.purview_accounts();
    let Some(active) = active else { return };

    egui::Frame::group(ui.style())
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 18.0, egui::Color32::from_rgb(0x35, 0x70, 0xA5));
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    active.initials(),
                    egui::FontId::proportional(13.0),
                    egui::Color32::WHITE,
                );
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(if active.display_name.trim().is_empty() { "Microsoft account" } else { active.display_name.trim() })
                            .font(theme::medium(13.0))
                            .color(tokens.text),
                    );
                    let secondary = account_secondary(&active);
                    if !secondary.is_empty() {
                        ui.label(egui::RichText::new(secondary).font(theme::regular(11.5)).color(tokens.text_muted));
                    }
                });
            });
            if accounts.len() > 1 {
                ui.add_space(6.0);
                let active_id = active.id.clone();
                let mut selected = active_id.clone();
                egui::ComboBox::from_id_salt("purview-account-picker").selected_text(active.label()).width(290.0).show_ui(ui, |ui| {
                    for account in &accounts {
                        let is_active = account.id == active_id;
                        if ui.selectable_label(is_active, account.label()).clicked() && !is_active {
                            selected = account.id.clone();
                        }
                    }
                });
                if selected != active_id {
                    match app.session.purview_switch_account(&selected) {
                        Ok(()) => {
                            app.purview_labels.clear();
                            app.purview_label_id.clear();
                            match app.session.purview_labels() {
                                Ok(labels) => app.purview_labels = labels,
                                Err(error) => app.notify_fmt("Couldn't load sensitivity labels: {e}", &[("e", &error.to_string())]),
                            }
                        }
                        Err(error) => app.notify_fmt("Couldn't switch Microsoft account: {e}", &[("e", &error.to_string())]),
                    }
                }
            }
        });
}

pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, tokens: &Tokens) -> (bool, bool) {
    ui.vertical(|ui| {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(tl!("Sensitivity label")).font(theme::semibold(18.0)));
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);

        let status = app.session.purview_status();
        match status {
            pdfcraft_engine::PurviewStatus::Ready => {
                account_card(ui, app, tokens);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(tl!("Add account"))).clicked() {
                        match app.session.purview_sign_in(None) {
                            Ok(()) => {
                                app.notify_tr("Signed in");
                                app.retry_purview_signin();
                                match app.session.purview_labels() {
                                    Ok(labels) => app.purview_labels = labels,
                                    Err(error) => app.notify_fmt("Couldn't load sensitivity labels: {e}", &[("e", &error.to_string())]),
                                }
                            }
                            Err(error) => app.notify_fmt("Sign-in failed: {e}", &[("e", &error.to_string())]),
                        }
                    }
                    if ui.add(egui::Button::new(tl!("Sign out"))).clicked() {
                        match app.session.purview_sign_out() {
                            Ok(()) => app.notify_tr("Signed out"),
                            Err(error) => app.notify_fmt("Sign-out failed: {e}", &[("e", &error.to_string())]),
                        }
                    }
                });
            }
            pdfcraft_engine::PurviewStatus::SignedOut => {
                egui::Frame::group(ui.style())
                    .fill(ui.visuals().faint_bg_color)
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(tl!("Sign in with Microsoft")).font(theme::medium(13.0)).color(tokens.text));
                        ui.add_space(5.0);
                        if ui.add(egui::Button::new(tl!("Sign in")).fill(tokens.accent)).clicked() {
                            match app.session.purview_sign_in(None) {
                                Ok(()) => {
                                    app.notify_tr("Signed in");
                                    app.retry_purview_signin();
                                    match app.session.purview_labels() {
                                        Ok(labels) => app.purview_labels = labels,
                                        Err(error) => app.notify_fmt("Couldn't load sensitivity labels: {e}", &[("e", &error.to_string())]),
                                    }
                                }
                                Err(error) => app.notify_fmt("Sign-in failed: {e}", &[("e", &error.to_string())]),
                            }
                        }
                    });
            }
            pdfcraft_engine::PurviewStatus::SigningIn => {
                ui.label(egui::RichText::new(tl!("Opening Microsoft sign-in")).color(tokens.text_muted));
            }
            pdfcraft_engine::PurviewStatus::NotConfigured => {
                ui.label(egui::RichText::new(tl!("Microsoft account sign-in is not configured")).color(tokens.text_muted));
            }
            pdfcraft_engine::PurviewStatus::SdkUnavailable => {
                ui.label(egui::RichText::new(tl!("Protection SDK unavailable")).color(tokens.text_muted));
            }
        }
        if let Some(error) = app.purview_signin_prompt.as_ref().and_then(|prompt| prompt.error.as_deref()) {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(error).color(Color32::from_rgb(0xD3, 0x2F, 0x2F)));
        }

        if let Some((_, id)) = app.active_ids()
            && let Some(doc) = app.session.get(id)
            && let Some(purview) = doc.purview()
        {
            ui.add_space(6.0);
            if purview.labeling_required {
                ui.label(egui::RichText::new(tl!("A sensitivity label is required before save")).font(theme::medium(13.0)).color(tokens.text_muted));
            }
            if let Some(default_label_id) = purview.default_label_id.as_ref()
                && purview.label.as_ref().is_none_or(|label| label.id != *default_label_id)
            {
                ui.label(
                    egui::RichText::new(tl!("A default label may be applied when the document is saved"))
                        .font(theme::regular(12.0))
                        .color(tokens.text_muted),
                );
            }
            ui.label(
                egui::RichText::new(if purview.protected { tl!("Protected document") } else { tl!("Sensitivity label") }).font(theme::medium(13.0)),
            );
            if let Some(label) = &purview.label {
                ui.label(format!(
                    "{} — {}",
                    label.name,
                    if label.description.is_empty() { label.tooltip.as_str() } else { label.description.as_str() }
                ));
            }
            ui.add_space(4.0);
            ui.label(egui::RichText::new(tl!("Effective permissions")).font(theme::medium(12.5)));
            egui::Grid::new("purview-effective-rights").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                let rights = [
                    ("View", purview.rights.view),
                    ("Edit", purview.rights.can_edit()),
                    ("Save", purview.rights.can_save()),
                    ("Print", purview.rights.can_print()),
                    ("Copy / extract", purview.rights.can_copy()),
                    ("Export", purview.rights.can_export()),
                    ("Comment", purview.rights.can_comment()),
                    ("Change security", purview.rights.can_change_security()),
                ];
                for (name, allowed) in rights {
                    ui.label(tl!(name));
                    ui.label(egui::RichText::new(tl!(if allowed { "Allowed" } else { "Not allowed" })).color(if allowed {
                        egui::Color32::from_rgb(0x2D, 0x9D, 0x5B)
                    } else {
                        tokens.text_muted
                    }));
                    ui.end_row();
                }
            });
        }

        if matches!(status, pdfcraft_engine::PurviewStatus::Ready) {
            ui.add_space(8.0);
            if app.purview_labels.is_empty() {
                match app.session.purview_labels() {
                    Ok(labels) => app.purview_labels = labels,
                    Err(error) => app.notify_fmt("Couldn't load sensitivity labels: {e}", &[("e", &error.to_string())]),
                }
            }

            if app.purview_labels.is_empty() {
                ui.label(tl!("No sensitivity labels are available for this account."));
            } else {
                let mut selected = app.purview_label_id.clone();
                for label in &app.purview_labels {
                    if !label.active {
                        continue;
                    }
                    let text = if label.description.is_empty() { label.name.clone() } else { format!("{} — {}", label.name, label.description) };
                    ui.radio_value(&mut selected, label.id.clone(), text);
                }
                app.purview_label_id = selected;
            }

            ui.add_space(8.0);
            let justification_label = ui.label(tl!("Justification (required for policy downgrades or removals)"));
            ui.add(crate::widgets::block(&mut app.purview_justification).desired_rows(3).desired_width(290.0).id_salt("purview-justification"))
                .labelled_by(justification_label.id);
        }
    });

    let (cancel, apply, remove_label, remove_protection) = ui
        .with_layout(Layout::right_to_left(Align::Center), |ui| {
            let cancel = ui.button(tl!("Cancel")).clicked();
            let ready = matches!(app.session.purview_status(), pdfcraft_engine::PurviewStatus::Ready);
            let apply = ui.add_enabled(ready && !app.purview_label_id.is_empty(), egui::Button::new(tl!("Apply label"))).clicked();
            let remove_label = ui.add_enabled(ready, egui::Button::new(tl!("Remove label"))).clicked();
            let remove_protection = ui.add_enabled(ready, egui::Button::new(tl!("Remove protection"))).clicked();
            (cancel, apply, remove_label, remove_protection)
        })
        .inner;
    if apply && let Some((_, id)) = app.active_ids() {
        let justification = if app.purview_justification.trim().is_empty() { None } else { Some(app.purview_justification.as_str()) };
        match app.session.purview_apply_label(id, &app.purview_label_id, justification) {
            Ok(()) => app.notify_tr("Sensitivity label applied"),
            Err(error) => app.notify_fmt("Couldn't apply the sensitivity label: {e}", &[("e", &error.to_string())]),
        }
    }
    if remove_label && let Some((_, id)) = app.active_ids() {
        let justification = if app.purview_justification.trim().is_empty() { None } else { Some(app.purview_justification.as_str()) };
        match app.session.purview_remove_label(id, justification) {
            Ok(()) => app.notify_tr("Sensitivity label removed"),
            Err(error) => app.notify_fmt("Couldn't remove the sensitivity label: {e}", &[("e", &error.to_string())]),
        }
    }
    if remove_protection && let Some((_, id)) = app.active_ids() {
        match app.session.purview_remove_protection(id) {
            Ok(()) => app.notify_tr("Protection removed"),
            Err(error) => app.notify_fmt("Couldn't remove protection: {e}", &[("e", &error.to_string())]),
        }
    }
    (apply, cancel)
}
