use super::*;

/// Draw the signing dialogs; returns `true` when the dialog should close.
pub(crate) fn dialog(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    let Some(step) = app.sign_draft.as_ref().map(|d| d.step) else { return true };
    match step {
        SignStep::Choose => choose(ui, app, t),
        SignStep::Configure => configure(ui, app, t),
        SignStep::SignAs => sign_as(ui, app, t),
    }
}

pub(super) fn title(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(tl!(text)).font(theme::semibold(18.0)));
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(8.0);
}

fn error(ui: &mut egui::Ui, err: &Option<String>) {
    if let Some(e) = err.as_ref().filter(|e| !e.is_empty()) {
        ui.colored_label(Color32::from_rgb(0xD7, 0x37, 0x3F), tl!(e));
        ui.add_space(4.0);
    }
}

/// Draw the digital-ID chooser, including unusable certificates and removal confirmation.
fn choose(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    title(ui, "Sign with a Digital ID");
    ui.label(tl!("Choose the digital ID that you want to use for signing:"));
    ui.add_space(6.0);
    let ids = app.digital_ids.clone();
    let Some(d) = app.sign_draft.as_mut() else { return true };
    let mut close = false;
    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
        for (i, e) in ids.iter().enumerate() {
            let selected = d.selected == Some(i);
            let usable = e.unusable.is_none();
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), egui::Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, usable, selected, &e.name));
            let fill = if selected {
                t.accent_soft
            } else if resp.hovered() && usable {
                t.hover
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
            // An unusable ID is greyed, with the reason where the issuer would be.
            let (icon, name_color) = if usable { (t.accent, t.text) } else { (t.text_muted, t.text_muted) };
            icons::paint(ui, Rect::from_min_size(rect.min + vec2(10.0, 15.0), vec2(20.0, 20.0)), "badge-check", 18.0, icon);
            // Certificate names can be any length: cut both lines with "…" at the row's edge
            // and show them whole on hover.
            let line = |text: &str, font: egui::FontId, color: Color32| {
                let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
                job.wrap = egui::text::TextWrapping::truncate_at_width((rect.width() - 50.0).max(0.0));
                ui.painter().layout_job(job)
            };
            let name = line(&e.name, theme::semibold(13.0), name_color);
            let store = if e.path.starts_with("keychain:") {
                tl!("Keychain  ·  ").to_string()
            } else if e.path.starts_with("windows:") {
                tl!("Windows store  ·  ").to_string()
            } else {
                String::new()
            };
            let sub = match &e.unusable {
                Some(reason) => format!("{store}{reason}"),
                None => format!(
                    "{store}{email}{issued}{issuer}{expires}{date}",
                    email = if e.email.is_empty() { String::new() } else { format!("{}  ·  ", e.email) },
                    issued = tl!("Issued by: "),
                    issuer = e.issuer,
                    expires = tl!(", Expires: "),
                    date = e.expires,
                ),
            };
            let details = line(&sub, theme::regular(11.5), t.text_muted);
            let elided = name.elided || details.elided;
            ui.painter().galley(rect.min + vec2(40.0, 9.0), name, name_color);
            ui.painter().galley(rect.min + vec2(40.0, 28.0), details, t.text_muted);
            let resp = if elided { resp.on_hover_text(format!("{}\n{sub}", e.name)) } else { resp };
            if !usable {
                continue;
            }
            if resp.clicked() {
                d.selected = Some(i);
                d.confirm_remove = None;
            }
            if resp.double_clicked() {
                d.step = SignStep::SignAs;
            }
        }
        if ids.is_empty() {
            ui.label(egui::RichText::new(tl!("No digital IDs yet.")).color(t.text_muted));
        }
    });
    ui.add_space(10.0);
    // Remove takes a digital ID file off the list after asking (#1396); OS key store IDs stay.
    let mut remove = None;
    if let Some(e) = d.confirm_remove.and_then(|i| ids.get(i)) {
        ui.label(crate::i18n::fmt(tl!("Remove \"{name}\" from this list? The file itself is not deleted."), &[("name", &e.name)]));
        ui.horizontal(|ui| {
            if widgets::pill_button(ui, tl!("Remove"), true).clicked() {
                remove = d.confirm_remove;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                d.confirm_remove = None;
            }
        });
        ui.add_space(10.0);
    }
    let removable = d.selected.filter(|&i| ids.get(i).is_some_and(|e| !e.path.starts_with("keychain:") && !e.path.starts_with("windows:")));
    ui.horizontal(|ui| {
        if widgets::pill_button(ui, tl!("Configure New Digital ID"), false).clicked() {
            d.step = SignStep::Configure;
            d.error = None;
        }
        if d.confirm_remove.is_none() && ui.add_enabled_ui(removable.is_some(), |ui| widgets::pill_button(ui, tl!("Remove"), false)).inner.clicked() {
            d.confirm_remove = removable;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add_enabled_ui(d.selected.is_some(), |ui| widgets::pill_button(ui, tl!("Continue"), true)).inner.clicked() {
                d.step = SignStep::SignAs;
                d.error = None;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                close = true;
            }
        });
    });
    if let Some(i) = remove {
        app.remove_digital_id(i);
    }
    close
}

fn configure(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    title(ui, "Configure a Digital ID for Signing");
    let Some(d) = app.sign_draft.as_mut() else { return true };
    let mut close = false;
    let mut go = false;
    // Browse… is desktop-only.
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut browse = false;
    widgets::radio_value(ui, &mut d.new_id.create, false, tl!("Use a Digital ID from a file"));
    widgets::radio_value(ui, &mut d.new_id.create, true, tl!("Create a new Digital ID (self-signed, saved to a password-protected file)"));
    ui.add_space(8.0);
    let n = &mut d.new_id;
    if n.create {
        egui::Grid::new("new-id").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            for (label, value) in [
                (tl!("Name"), &mut n.name),
                (tl!("Organizational Unit"), &mut n.unit),
                (tl!("Organization Name"), &mut n.organization),
                (tl!("Email Address"), &mut n.email),
            ] {
                let l = ui.label(label);
                ui.add(crate::widgets::line(value).desired_width(260.0)).labelled_by(l.id);
                ui.end_row();
            }
            let l = ui.label(tl!("Country/Region"));
            ui.add(crate::widgets::line(&mut n.country).hint_text("US").char_limit(2).desired_width(60.0)).labelled_by(l.id);
            ui.end_row();
            ui.label(tl!("Key Algorithm"));
            craft_ui::ComboBox::from_id_salt("key-alg").selected_text(KEY_ALGORITHMS[n.key].0).show_ui(ui, |ui| {
                for (i, (label, _)) in KEY_ALGORITHMS.iter().enumerate() {
                    ui.selectable_value(&mut n.key, i, *label);
                }
            });
            ui.end_row();
            let l = ui.label(tl!("Password"));
            ui.add(crate::widgets::line(&mut n.password).password(true).desired_width(200.0)).labelled_by(l.id);
            ui.end_row();
            let l = ui.label(tl!("Confirm Password"));
            ui.add(crate::widgets::line(&mut n.confirm).password(true).desired_width(200.0)).labelled_by(l.id);
            ui.end_row();
        });
        ui.label(
            egui::RichText::new(tl!("Valid for 5 years, for digital signatures. Self-signed IDs are usually not trusted by others."))
                .small()
                .color(t.text_muted),
        );
    } else {
        egui::Grid::new("id-file").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            let l = ui.label(tl!("File (.p12, .pfx)"));
            ui.horizontal(|ui| {
                ui.add(crate::widgets::line(&mut n.file).desired_width(220.0)).labelled_by(l.id);
                #[cfg(not(target_arch = "wasm32"))]
                if ui.button(tl!("Browse…")).clicked() {
                    browse = true;
                }
            });
            ui.end_row();
            let l = ui.label(tl!("Password"));
            ui.add(crate::widgets::line(&mut n.file_password).password(true).desired_width(200.0)).labelled_by(l.id);
            ui.end_row();
        });
    }
    ui.add_space(6.0);
    error(ui, &d.error);
    ui.horizontal(|ui| {
        if !app.digital_ids.is_empty() && widgets::pill_button(ui, tl!("Back"), false).clicked() {
            d.step = SignStep::Choose;
            d.error = None;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, if d.new_id.create { tl!("Save") } else { tl!("Continue") }, true).clicked() {
                go = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                close = true;
            }
        });
    });
    if go {
        let create = app.sign_draft.as_ref().is_some_and(|d| d.new_id.create);
        let result = if create { app.create_digital_id() } else { app.import_digital_id() };
        let Some(d) = app.sign_draft.as_mut() else { return true };
        match result {
            Ok(i) => {
                d.selected = Some(i);
                d.step = SignStep::Choose;
                d.error = None;
                d.new_id = NewIdDraft::default();
            }
            Err(e) => d.error = Some(e),
        }
    }
    // The picker answers on a later frame, into the draft if the same dialog is still open.
    #[cfg(not(target_arch = "wasm32"))]
    if browse {
        let dialog = rfd::AsyncFileDialog::new().add_filter(tl!("Digital ID"), &["p12", "pfx"]);
        let epoch = app.dialog_epoch();
        app.ask_one(crate::pickers::Ask::File(dialog), None, move |app, p| {
            // Not one closed meanwhile, or opened again since.
            if app.dialog_epoch() == epoch
                && let Some(d) = app.sign_draft.as_mut()
            {
                d.new_id.file = p.to_string_lossy().into_owned();
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    let _ = browse;
    close
}

/// The appearance preview: Acrobat's standard layout (name left, details right).
/// The saved signature or image `g` stands for, given the saved Fill & Sign `signature`.
pub(super) fn graphic_of(signature: Option<&SavedSig>, g: &SignGraphic) -> Option<SavedSig> {
    match g {
        SignGraphic::Name => None,
        SignGraphic::FillSign => signature.cloned(),
        SignGraphic::Image(image) => Some(SavedSig::Image(image.clone())),
    }
}

/// The words of a visible signature's labels in `lang`, and whether they fell back to English.
///
/// The labels are stamped into the document, so they are all in `lang` or all in English, never a
/// mix. English when `lang` is English; when it is written right to left (its catalog is kept in
/// display order, not the order a PDF stores text in); when one label has no translation; or when
/// the signature's font can't draw one (Chinese, Japanese, Cyrillic, Greek in Helvetica), which
/// signing would refuse. The English words are the sign crate's own, so they can't drift apart.
pub(crate) fn appearance_labels(lang: crate::i18n::Lang) -> (AppearanceLabels, bool) {
    let english = AppearanceLabels::default();
    if lang == crate::i18n::Lang::EN {
        return (english, false);
    }
    let said = |s: &str| (!lang.rtl() && crate::i18n::has(lang, s)).then(|| crate::i18n::tr(lang, s).to_string());
    let translated = (|| {
        Some(AppearanceLabels {
            signed_by: said(&english.signed_by)?,
            distinguished_name: said(&english.distinguished_name)?,
            reason: said(&english.reason)?,
            location: said(&english.location)?,
            date: said(&english.date)?,
        })
    })();
    match translated.filter(|l| l.first_undrawable().is_none()) {
        Some(labels) => (labels, false),
        None => (english, true),
    }
}

/// The detail lines of a visible signature, as `sign::pdf` stamps them (the distinguished name
/// aside, which the dialog doesn't know before the ID is opened): the label above the name, then
/// the reason, location and date, each after its label. `reason` and `location` are as typed.
pub(super) fn detail_lines(a: &Appearance, labels: &AppearanceLabels, name: &str, reason: &str, location: &str, date: &str) -> Vec<String> {
    let label = |l: &str, v: &str| if a.labels { format!("{l}{v}") } else { v.to_string() };
    let mut lines = Vec::new();
    if a.name {
        if a.labels {
            lines.push(labels.signed_by.clone());
        }
        lines.push(name.to_string());
    }
    if a.reason && !reason.trim().is_empty() {
        lines.push(label(&labels.reason, reason.trim()));
    }
    if a.location && !location.trim().is_empty() {
        lines.push(label(&labels.location, location.trim()));
    }
    if a.date {
        lines.push(label(&labels.date, date));
    }
    lines.retain(|l| !l.is_empty());
    lines
}

/// The signature as it will look: the graphic (or the name) on the left, the details beside it
/// (`lines`, from [`detail_lines`] with the labels signing will stamp).
fn preview(
    ui: &mut egui::Ui,
    t: &Tokens,
    name: &str,
    a: &Appearance,
    lines: &[String],
    graphic: Option<&SavedSig>,
    cache: &mut Option<(SavedSig, egui::TextureHandle)>,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 96.0), egui::Sense::hover());
    let bg = if t.dark() { Color32::from_gray(0xF4) } else { Color32::WHITE };
    ui.painter().rect(rect, CornerRadius::same(4), bg, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let inner = rect.shrink(10.0);
    // As in the signature itself: a graphic with no details beside it fills the box.
    let alone = graphic.is_some() && lines.is_empty();
    let half = Rect::from_min_size(inner.min, vec2(inner.width() * 0.5 - 6.0, inner.height()));
    if let Some(g) = graphic {
        crate::fill_sign::paint_saved(ui, if alone { inner } else { half }, g, cache, "sign-graphic");
    } else if a.name {
        let galley = ui.painter().layout(name.to_string(), theme::regular(22.0), Color32::BLACK, half.width());
        ui.painter().galley(half.left_center() - vec2(0.0, galley.size().y / 2.0), galley, Color32::BLACK);
    }
    let x = if (a.name || graphic.is_some()) && !alone { inner.center().x + 6.0 } else { inner.left() };
    let galley = ui.painter().layout(lines.join("\n"), theme::regular(11.0), Color32::from_gray(0x20), inner.right() - x);
    ui.painter().galley(egui::pos2(x, inner.center().y - galley.size().y / 2.0), galley, Color32::BLACK);
}

fn sign_as(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    let ids = app.digital_ids.clone();
    // The time as the signature will show it (signing takes the session clock then).
    let date = sign::pdf::display_date(&app.session.signing_date());
    let Some(d) = app.sign_draft.as_mut() else { return true };
    let Some(entry) = d.selected.and_then(|i| ids.get(i)).cloned() else {
        d.step = SignStep::Choose;
        return false;
    };
    title(
        ui,
        &crate::i18n::fmt(
            tl!("{verb} as \"{name}\""),
            &[("verb", if d.certify.is_some() { tl!("Certify") } else { tl!("Sign") }), ("name", &entry.name)],
        ),
    );
    let in_os_key_store = entry.path.starts_with("keychain:") || entry.path.starts_with("windows:");
    let mut close = false;
    let mut go = false;
    #[cfg(not(target_arch = "wasm32"))]
    let mut pick = false;
    if d.rect.is_some() || d.field.is_some() {
        ui.label(egui::RichText::new(tl!("Appearance")).font(theme::semibold(12.5)));
        let graphic = graphic_of(app.signature.as_ref(), &d.graphic);
        let (labels, in_english) = appearance_labels(crate::i18n::current());
        let lines = detail_lines(&d.appearance, &labels, &entry.name, &d.reason, &d.location, &date);
        preview(ui, t, &entry.name, &d.appearance, &lines, graphic.as_ref(), &mut app.sign_graphic_preview);
        if in_english && d.appearance.labels {
            ui.label(
                egui::RichText::new(tl!("The labels are in English: the signature's font can't draw them in this language."))
                    .small()
                    .color(t.text_muted),
            );
        }
        ui.horizontal_wrapped(|ui| {
            let a = &mut d.appearance;
            ui.add(widgets::checkbox(&mut a.name, tl!("Name")));
            ui.add(widgets::checkbox(&mut a.date, tl!("Date")));
            ui.add(widgets::checkbox(&mut a.reason, tl!("Reason")));
            ui.add(widgets::checkbox(&mut a.location, tl!("Location")));
            ui.add(widgets::checkbox(&mut a.distinguished_name, tl!("Distinguished name")));
            ui.add(widgets::checkbox(&mut a.labels, tl!("Labels")));
        });
        // Acrobat: Configure Signature Appearance, Text, Draw or Image.
        ui.horizontal_wrapped(|ui| {
            ui.label(tl!("Graphic"));
            widgets::radio_value(ui, &mut d.graphic, SignGraphic::Name, tl!("Signer's name"));
            ui.add_enabled_ui(app.signature.is_some(), |ui| {
                if ui.add(widgets::radio(d.graphic == SignGraphic::FillSign, tl!("Fill & Sign signature"))).clicked() {
                    d.graphic = SignGraphic::FillSign;
                }
            });
            ui.add_enabled_ui(d.image.is_some(), |ui| {
                if ui.add(widgets::radio(matches!(d.graphic, SignGraphic::Image(_)), tl!("Image"))).clicked()
                    && let Some(image) = &d.image
                {
                    d.graphic = SignGraphic::Image(image.clone());
                }
            });
            #[cfg(not(target_arch = "wasm32"))]
            if widgets::pill_button(ui, tl!("Choose image…"), false).clicked() {
                pick = true;
            }
        });
        ui.add_space(6.0);
    } else {
        ui.label(egui::RichText::new(tl!("The signature will be invisible.")).color(t.text_muted));
    }
    egui::Grid::new("sign-as").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        if d.certify.is_some() {
            ui.label(tl!("Permitted actions after certifying"));
            let mut p = d.certify.unwrap_or(2);
            let label = |p: u8| match p {
                1 => tl!("No changes allowed"),
                3 => tl!("Annotations, form fill-in, and digital signatures"),
                _ => tl!("Form fill-in and digital signatures"),
            };
            craft_ui::ComboBox::from_id_salt("certify-p").selected_text(label(p)).width(300.0).show_ui(ui, |ui| {
                for v in [1u8, 2, 3] {
                    ui.selectable_value(&mut p, v, label(v));
                }
            });
            d.certify = Some(p);
            ui.end_row();
        }
        let l = ui.label(tl!("Reason"));
        ui.add(crate::widgets::line(&mut d.reason).hint_text(tl!("Optional")).desired_width(260.0)).labelled_by(l.id);
        ui.end_row();
        let l = ui.label(tl!("Location"));
        ui.add(crate::widgets::line(&mut d.location).hint_text(tl!("Optional")).desired_width(260.0)).labelled_by(l.id);
        ui.end_row();
        if in_os_key_store {
            ui.label("");
            ui.label(
                egui::RichText::new(if entry.path.starts_with("windows:") {
                    tl!("The key is in the Windows certificate store, which may ask to allow PdfCraft to use it.")
                } else {
                    tl!("The key is in the macOS Keychain, which may ask to allow PdfCraft to use it.")
                })
                .small()
                .color(t.text_muted),
            );
        } else {
            let l = ui.label(tl!("Digital ID password"));
            let r = ui.add(crate::widgets::line(&mut d.password).password(true).desired_width(200.0)).labelled_by(l.id);
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                go = true;
            }
        }
        ui.end_row();
    });
    ui.add_space(6.0);
    error(ui, &d.error);
    ui.horizontal(|ui| {
        if widgets::pill_button(ui, tl!("Back"), false).clicked() {
            d.step = SignStep::Choose;
            d.error = None;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Sign"), true).clicked() {
                go = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                close = true;
            }
        });
    });
    #[cfg(not(target_arch = "wasm32"))]
    if pick {
        app.pick_sign_graphic_image();
    }
    if go {
        match app.finish_signing() {
            Ok(()) => {
                app.sign_draft = None;
                return true;
            }
            Err(e) => {
                if let Some(d) = app.sign_draft.as_mut() {
                    d.error = Some(e);
                }
            }
        }
    }
    close
}
