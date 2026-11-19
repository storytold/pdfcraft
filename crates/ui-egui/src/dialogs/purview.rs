use egui::{Align, Color32, Layout};

use crate::PdfCraftApp;
use crate::theme;
use crate::theme::Tokens;

pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, tokens: &Tokens) -> (bool, bool) {
    ui.vertical(|ui| {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(tl!("Microsoft Purview sensitivity label")).font(theme::semibold(18.0)));
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);

        let status = app.session.purview_status();
        let status_text = match status {
            pdfcraft_engine::PurviewStatus::Ready => tl!("Signed in"),
            pdfcraft_engine::PurviewStatus::SignedOut => tl!("Signed out"),
            pdfcraft_engine::PurviewStatus::SigningIn => tl!("Signing in"),
            pdfcraft_engine::PurviewStatus::NotConfigured => tl!("Not configured"),
            pdfcraft_engine::PurviewStatus::SdkUnavailable => tl!("MIP SDK unavailable"),
        };
        ui.horizontal(|ui| {
            ui.label(tl!("Account status"));
            ui.label(egui::RichText::new(status_text).color(if matches!(status, pdfcraft_engine::PurviewStatus::Ready) {
                egui::Color32::from_rgb(0x2D, 0x9D, 0x5B)
            } else {
                tokens.text_muted
            }));
        });
        if let Some(error) = app.purview_signin_prompt.as_ref().and_then(|prompt| prompt.error.as_deref()) {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(error).color(Color32::from_rgb(0xD3, 0x2F, 0x2F)));
        }

        if let Some((_, id)) = app.active_ids()
            && let Some(doc) = app.session.get(id)
            && let Some(purview) = doc.purview()
        {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(if purview.protected { tl!("Protected by Microsoft Purview") } else { tl!("Microsoft Purview label metadata") })
                    .font(theme::medium(13.0)),
            );
            if let Some(label) = &purview.label {
                ui.label(format!(
                    "{} — {}",
                    label.name,
                    if label.description.is_empty() { label.tooltip.as_str() } else { label.description.as_str() }
                ));
            }
            let rights = [
                ("View", purview.rights.view),
                ("Edit", purview.rights.can_edit()),
                ("Save", purview.rights.can_save()),
                ("Print", purview.rights.can_print()),
                ("Copy", purview.rights.can_copy()),
                ("Export", purview.rights.can_export()),
            ]
            .into_iter()
            .filter(|(_, allowed)| *allowed)
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
            ui.label(format!("{} {}", tl!("Rights:"), rights.join(", ")));
        }

        ui.add_space(8.0);
        let username_label = ui.label(tl!("Microsoft 365 account"));
        ui.add(crate::widgets::line(&mut app.purview_username_hint).desired_width(290.0).id_salt("purview-username")).labelled_by(username_label.id);

        ui.horizontal(|ui| {
            let can_sign_in = matches!(
                status,
                pdfcraft_engine::PurviewStatus::SignedOut
                    | pdfcraft_engine::PurviewStatus::NotConfigured
                    | pdfcraft_engine::PurviewStatus::SdkUnavailable
            );
            if ui.add_enabled(can_sign_in, egui::Button::new(tl!("Sign in"))).clicked() {
                let hint = if app.purview_username_hint.trim().is_empty() { None } else { Some(app.purview_username_hint.as_str()) };
                match app.session.purview_sign_in(hint) {
                    Ok(()) => {
                        app.notify_tr("Signed in to Microsoft Purview");
                        app.retry_purview_signin();
                        match app.session.purview_labels() {
                            Ok(labels) => app.purview_labels = labels,
                            Err(error) => app.notify_fmt("Couldn't load Microsoft Purview labels: {e}", &[("e", &error.to_string())]),
                        }
                    }
                    Err(error) => app.notify_fmt("Microsoft Purview sign-in failed: {e}", &[("e", &error.to_string())]),
                }
            }
            if ui.add_enabled(matches!(status, pdfcraft_engine::PurviewStatus::Ready), egui::Button::new(tl!("Sign out"))).clicked() {
                match app.session.purview_sign_out() {
                    Ok(()) => app.notify_tr("Signed out of Microsoft Purview"),
                    Err(error) => app.notify_fmt("Microsoft Purview sign-out failed: {e}", &[("e", &error.to_string())]),
                }
            }
        });

        ui.add_space(8.0);
        if matches!(status, pdfcraft_engine::PurviewStatus::Ready) && app.purview_labels.is_empty() {
            match app.session.purview_labels() {
                Ok(labels) => app.purview_labels = labels,
                Err(error) => app.notify_fmt("Couldn't load Microsoft Purview labels: {e}", &[("e", &error.to_string())]),
            }
        }

        if app.purview_labels.is_empty() {
            ui.label(tl!("No sensitivity labels are available. Sign in with a Microsoft 365 account, then try again."));
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
    });

    let (cancel, apply, remove_label, remove_protection) = ui
        .with_layout(Layout::right_to_left(Align::Center), |ui| {
            let cancel = ui.button(tl!("Cancel")).clicked();
            let apply = ui.add_enabled(!app.purview_label_id.is_empty(), egui::Button::new(tl!("Apply label"))).clicked();
            let remove_label = ui.button(tl!("Remove label")).clicked();
            let remove_protection = ui.button(tl!("Remove protection")).clicked();
            (cancel, apply, remove_label, remove_protection)
        })
        .inner;
    if apply && let Some((_, id)) = app.active_ids() {
        let justification = if app.purview_justification.trim().is_empty() { None } else { Some(app.purview_justification.as_str()) };
        match app.session.purview_apply_label(id, &app.purview_label_id, justification) {
            Ok(()) => app.notify_tr("Microsoft Purview label applied"),
            Err(error) => app.notify_fmt("Couldn't apply the Microsoft Purview label: {e}", &[("e", &error.to_string())]),
        }
    }
    if remove_label && let Some((_, id)) = app.active_ids() {
        let justification = if app.purview_justification.trim().is_empty() { None } else { Some(app.purview_justification.as_str()) };
        match app.session.purview_remove_label(id, justification) {
            Ok(()) => app.notify_tr("Microsoft Purview label removed"),
            Err(error) => app.notify_fmt("Couldn't remove the Microsoft Purview label: {e}", &[("e", &error.to_string())]),
        }
    }
    if remove_protection && let Some((_, id)) = app.active_ids() {
        match app.session.purview_remove_protection(id) {
            Ok(()) => app.notify_tr("Microsoft Purview protection removed"),
            Err(error) => app.notify_fmt("Couldn't remove Microsoft Purview protection: {e}", &[("e", &error.to_string())]),
        }
    }
    (apply, cancel)
}
