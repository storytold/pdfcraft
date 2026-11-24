use egui::{Align, Layout};

use crate::PdfCraftApp;
use crate::theme;
use crate::theme::Tokens;

pub(super) fn properties(
    ui: &mut egui::Ui,
    row: &impl Fn(&mut egui::Ui, &str, String),
    purview: Option<&pdfcraft_engine::PurviewDocument>,
    status: pdfcraft_engine::PurviewStatus,
    req: &mut super::Pending,
) {
    let Some(purview) = purview else { return };
    row(
        ui,
        "Microsoft Purview",
        if purview.protected {
            tl!("Protected").to_string()
        } else if purview.label.is_some() {
            tl!("Sensitivity label").to_string()
        } else {
            tl!("Detected").to_string()
        },
    );
    if let Some(label) = &purview.label {
        row(
            ui,
            "Sensitivity label",
            format!("{}{}", label.name, if label.description.is_empty() { String::new() } else { format!(" — {}", label.description) }),
        );
    }
    if let Some(owner) = purview.owner.as_ref().filter(|owner| !owner.trim().is_empty()) {
        row(ui, "Microsoft Purview owner", owner.clone());
    }
    let mip_yes = |b: bool| tl!(if b { "Allowed" } else { "Not allowed" }).to_string();
    row(ui, "MIP view", mip_yes(purview.rights.view));
    row(ui, "MIP edit/save", mip_yes(purview.rights.can_edit() || purview.rights.can_save()));
    row(ui, "MIP print", mip_yes(purview.rights.can_print()));
    row(ui, "MIP copy/extract", mip_yes(purview.rights.can_copy()));
    row(ui, "MIP export/change security", mip_yes(purview.rights.can_export() || purview.rights.can_change_security()));
    ui.horizontal(|ui| match status {
        pdfcraft_engine::PurviewStatus::Ready if purview.protected => {
            ui.label(tl!("Changing labels or protection on protected documents is not available yet."));
        }
        pdfcraft_engine::PurviewStatus::Ready => {
            if purview.label.is_some() {
                if ui.button(tl!("Change sensitivity label…")).clicked() {
                    req.link_command = Some("purview.apply_label");
                }
            } else if ui.button(tl!("Apply sensitivity label…")).clicked() {
                req.link_command = Some("purview.apply_label");
            }
            if purview.label.is_some() && purview.rights.can_change_security() && ui.button(tl!("Remove label")).clicked() {
                req.link_command = Some("purview.remove_label");
            }
        }
        pdfcraft_engine::PurviewStatus::SignedOut => {
            if ui.button(tl!("Sign in to Microsoft…")).clicked() {
                req.link_command = Some("purview.sign_in");
            }
        }
        pdfcraft_engine::PurviewStatus::SigningIn => {
            ui.label(tl!("Opening Microsoft sign-in"));
        }
        pdfcraft_engine::PurviewStatus::NotConfigured => {
            ui.label(tl!("Microsoft account sign-in is not configured"));
        }
        pdfcraft_engine::PurviewStatus::SdkUnavailable => {
            ui.label(tl!("Protection SDK unavailable"));
        }
    });
}

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
            if accounts.len() > 1 && app.purview_account_change_reason().is_none() {
                ui.add_space(6.0);
                let active_id = active.id.clone();
                let mut selected = active_id.clone();
                craft_ui::ComboBox::new("purview-account-picker", "Microsoft account").selected_text(active.label()).width(290.0).show_ui(ui, |ui| {
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

fn document_protection_card(ui: &mut egui::Ui, app: &PdfCraftApp, tokens: &Tokens) {
    let Some((_, id)) = app.active_ids() else { return };
    let Some(doc) = app.session.get(id) else { return };
    let Some(purview) = doc.purview() else { return };

    egui::Frame::group(ui.style())
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let title = if purview.protected {
                    "Protected"
                } else if purview.label.is_some() {
                    "Sensitivity label"
                } else {
                    "Unlabeled"
                };
                ui.label(egui::RichText::new(title).font(theme::medium(13.0)).color(tokens.text));
                if purview.protected {
                    ui.label(egui::RichText::new("IRM").font(theme::regular(10.5)).color(tokens.accent_text));
                }
            });

            if let Some(label) = &purview.label {
                ui.add_space(5.0);
                ui.label(egui::RichText::new(&label.name).font(theme::semibold(14.0)).color(tokens.text));
                let detail = if label.description.is_empty() { label.tooltip.as_str() } else { label.description.as_str() };
                if !detail.is_empty() {
                    ui.label(egui::RichText::new(detail).font(theme::regular(11.5)).color(tokens.text_muted));
                }
                if label.sensitivity != 0 {
                    ui.label(egui::RichText::new(format!("Sensitivity: {}", label.sensitivity)).font(theme::regular(11.0)).color(tokens.text_muted));
                }
                if label.protection {
                    ui.label(egui::RichText::new("Protection policy: enabled").font(theme::regular(11.0)).color(tokens.text_muted));
                }
            } else {
                ui.add_space(5.0);
                ui.label(
                    egui::RichText::new("No sensitivity label is attached to this document").font(theme::regular(11.5)).color(tokens.text_muted),
                );
            }

            if purview.labeling_required {
                ui.add_space(5.0);
                ui.label(egui::RichText::new("A sensitivity label is required before saving").font(theme::medium(11.5)).color(tokens.text_muted));
            }
            if let Some(default_label_id) = purview.default_label_id.as_ref()
                && purview.label.as_ref().is_none_or(|label| label.id != *default_label_id)
            {
                ui.label(
                    egui::RichText::new("A default label is configured but is not applied automatically.")
                        .font(theme::regular(11.0))
                        .color(tokens.text_muted),
                );
            }
            if purview.downgrade_justification_required {
                ui.label(egui::RichText::new("Downgrade justification is required").font(theme::regular(11.0)).color(tokens.text_muted));
            }
            if let Some(owner) = purview.owner.as_ref().filter(|owner| !owner.trim().is_empty()) {
                ui.add_space(5.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Owner").font(theme::medium(11.0)).color(tokens.text_muted));
                    ui.label(egui::RichText::new(owner).font(theme::regular(11.0)).color(tokens.text));
                });
            }
            if let Some(content_id) = purview.content_id.as_ref().filter(|id| !id.trim().is_empty()) {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Policy").font(theme::medium(11.0)).color(tokens.text_muted));
                    ui.label(egui::RichText::new(content_id).font(theme::regular(11.0)).color(tokens.text_muted));
                });
            }
        });
}

fn capability_row(ui: &mut egui::Ui, tokens: &Tokens, name: &str, allowed: bool) {
    ui.label(tl!(name));
    ui.horizontal(|ui| {
        ui.painter().circle_filled(
            egui::pos2(ui.cursor().left() + 4.0, ui.cursor().center().y),
            3.5,
            if allowed { egui::Color32::from_rgb(0x2D, 0x9D, 0x5B) } else { egui::Color32::from_rgb(0xB5, 0xB5, 0xBC) },
        );
        ui.add_space(11.0);
        ui.label(egui::RichText::new(tl!(if allowed { "On" } else { "Off" })).color(if allowed {
            egui::Color32::from_rgb(0x2D, 0x9D, 0x5B)
        } else {
            tokens.text_muted
        }));
    });
    ui.end_row();
}

fn document_capabilities_card(ui: &mut egui::Ui, app: &PdfCraftApp, tokens: &Tokens) {
    let Some((_, id)) = app.active_ids() else { return };
    let Some(doc) = app.session.get(id) else { return };
    let Some(purview) = doc.purview() else { return };
    if !purview.protected {
        return;
    }

    egui::Frame::group(ui.style())
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Document capabilities").font(theme::medium(12.5)).color(tokens.text));
            ui.add_space(4.0);
            egui::Grid::new("purview-document-capabilities").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                capability_row(ui, tokens, "View", purview.rights.view);
                capability_row(ui, tokens, "Edit", purview.rights.can_edit());
                capability_row(ui, tokens, "Save", purview.rights.can_save());
                capability_row(ui, tokens, "Print", purview.rights.can_print());
                capability_row(ui, tokens, "Copy / extract", purview.rights.can_copy());
                capability_row(ui, tokens, "Comment", purview.rights.can_comment());
                capability_row(ui, tokens, "Export", purview.rights.can_export());
                capability_row(ui, tokens, "Change security", purview.rights.can_change_security());
            });
            if !purview.rights.can_export() {
                ui.label(tl!("Export permission is required for unprotected files and page or object copies."));
            }
            ui.label(tl!("Digital signing is not available for protected documents yet."));
        });
}

fn signed_out_account_panel(ui: &mut egui::Ui, app: &mut PdfCraftApp, tokens: &Tokens) {
    egui::Frame::group(ui.style())
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            if ui.add(egui::Button::new("Sign in").fill(tokens.accent)).clicked() {
                app.start_purview_sign_in();
            }
        });
}

pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, tokens: &Tokens) -> (bool, bool) {
    let current_is_protected =
        app.active_ids().and_then(|(_, id)| app.session.get(id)).and_then(|doc| doc.purview().map(|purview| purview.protected)).unwrap_or(false);
    ui.vertical(|ui| {
        ui.add_space(4.0);
        let status = app.purview_status();
        let title = if matches!(status, pdfcraft_engine::PurviewStatus::Ready) { "Sensitivity label" } else { "Microsoft account" };
        ui.label(egui::RichText::new(title).font(theme::semibold(18.0)));
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);

        match status {
            pdfcraft_engine::PurviewStatus::Ready => {
                account_card(ui, app, tokens);
                ui.add_space(8.0);
                if let Some(reason) = app.purview_account_change_reason() {
                    ui.label(tl!(reason));
                } else {
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new("Add account")).clicked() {
                            app.start_purview_sign_in();
                        }
                        if ui.add(egui::Button::new("Sign out")).clicked() {
                            match app.session.purview_sign_out() {
                                Ok(()) => app.notify_tr("Signed out"),
                                Err(error) => app.notify_fmt("Sign-out failed: {e}", &[("e", &error.to_string())]),
                            }
                        }
                    });
                }
            }
            pdfcraft_engine::PurviewStatus::SignedOut => signed_out_account_panel(ui, app, tokens),
            pdfcraft_engine::PurviewStatus::SigningIn => {
                ui.label(egui::RichText::new("Opening Microsoft sign-in").font(theme::medium(13.0)).color(tokens.text_muted));
                ui.label(tl!("Microsoft sign-in and policy loading continue if you close this window."));
            }
            pdfcraft_engine::PurviewStatus::NotConfigured => {
                ui.label(egui::RichText::new("Microsoft account sign-in is not configured").font(theme::medium(13.0)).color(tokens.text_muted));
            }
            pdfcraft_engine::PurviewStatus::SdkUnavailable => {
                ui.label(egui::RichText::new("Protection SDK unavailable").font(theme::medium(13.0)).color(tokens.text_muted));
            }
        }

        if let Some(error) = app.purview_signin_prompt.as_ref().and_then(|prompt| prompt.error.as_deref()) {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(error).color(egui::Color32::from_rgb(0xD3, 0x2F, 0x2F)));
        }

        if matches!(status, pdfcraft_engine::PurviewStatus::Ready) {
            ui.add_space(8.0);
            document_protection_card(ui, app, tokens);
            ui.add_space(8.0);
            document_capabilities_card(ui, app, tokens);
            ui.add_space(8.0);

            if current_is_protected {
                ui.label(tl!("Changing labels or protection on protected documents is not available yet."));
            }

            ui.label(egui::RichText::new("Sensitivity labels").font(theme::medium(12.5)).color(tokens.text));
            if app.purview_labels.is_empty() {
                ui.label(
                    egui::RichText::new("No sensitivity labels are available to this account or tenant policy.")
                        .font(theme::regular(11.5))
                        .color(tokens.text_muted),
                );
            } else {
                let mut selected = app.purview_label_id.clone();
                for label in &app.purview_labels {
                    if !label.active {
                        continue;
                    }
                    let detail = if label.description.is_empty() { label.tooltip.as_str() } else { label.description.as_str() };
                    let policy = if label.protection { "Protection policy" } else { "Label only" };
                    let text =
                        if detail.is_empty() { format!("{} - {}", label.name, policy) } else { format!("{} - {} - {}", label.name, policy, detail) };
                    crate::widgets::radio_value(ui, &mut selected, label.id.clone(), text);
                }
                app.purview_label_id = selected;
            }

            ui.add_space(8.0);
            let justification_label = ui.label("Justification (required for policy downgrades or removals)");
            ui.add(crate::widgets::block(&mut app.purview_justification).desired_rows(3).desired_width(290.0).id_salt("purview-justification"))
                .labelled_by(justification_label.id);
        }
    });

    let status = app.purview_status();
    let ready = matches!(status, pdfcraft_engine::PurviewStatus::Ready);
    let active_document_rights =
        app.active_ids().and_then(|(_, id)| app.session.get(id)).and_then(|doc| doc.purview().map(|purview| purview.rights.clone()));
    let current_has_label = app
        .active_ids()
        .and_then(|(_, id)| app.session.get(id))
        .and_then(|doc| doc.purview().map(|purview| purview.label.is_some()))
        .unwrap_or(false);
    let can_change_security = active_document_rights.as_ref().is_some_and(|rights| rights.can_change_security());

    let (cancel, apply, remove_label) = if ready {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let cancel = ui.button("Cancel").clicked();
            let apply = if !current_is_protected && !app.purview_label_id.is_empty() { ui.button("Apply label").clicked() } else { false };
            let remove_label =
                if !current_is_protected && current_has_label && can_change_security { ui.button("Remove label").clicked() } else { false };
            (cancel, apply, remove_label)
        })
        .inner
    } else {
        let close_label = if matches!(status, pdfcraft_engine::PurviewStatus::SigningIn) { "Close" } else { "Cancel" };
        let cancel = ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.button(tl!(close_label)).clicked()).inner;
        (cancel, false, false)
    };

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
    (apply, cancel)
}
