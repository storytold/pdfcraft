//! Acrobat JavaScript in the shell: what scripts ask for (alerts, printing, navigation, links),
//! the JavaScript console (⌘J), Document JavaScripts, and Preferences ▸ JavaScript.

use egui::{Align, Layout};
use pdfcraft_engine::js::{JsOutput, Request};
use pdfcraft_engine::{DocId, Edit};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// The JavaScript console: the input and the output so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsConsole {
    pub input: String,
    pub log: Vec<String>,
}

/// Document JavaScripts: the script being edited.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocJsDraft {
    pub name: String,
    pub script: String,
}

/// What the user is told when a form's scripts were turned off: in a toast as it happens, then
/// on the notice bar for as long as the document is open.
pub(crate) fn scripts_off_notice() -> String {
    tl!("This form's scripts are turned off because one ran too long. Reopen the document to run them again.").to_string()
}

impl PdfCraftApp {
    /// Act on what scripts produced in document `id`: alerts are shown, console output goes to
    /// the console, print and page requests are carried out, links wait for the user's permission
    /// (form submissions are reported, never sent).
    pub fn handle_js(&mut self, id: DocId, out: JsOutput) {
        let view = self.views.iter().position(|v| v.id == id);
        // A script that ran away turned the form's scripts off: calculations and buttons stop
        // working, and the error is only in the console. Say so once, and bring the notice bar
        // (which keeps saying so) back if it was closed. A message of the form's own, below,
        // takes the toast's place.
        if let Some(v) = view.and_then(|i| self.views.get_mut(i))
            && !v.scripts_off_noted
            && self.session.get(id).is_some_and(|d| d.xfa_scripts_off())
        {
            v.scripts_off_noted = true;
            v.notice_dismissed = false;
            self.notify(scripts_off_notice());
        }
        if out.is_empty() {
            return;
        }
        self.js_console.log.extend(out.console.iter().cloned());
        for e in &out.errors {
            self.js_console.log.push(format!("Error: {e}"));
        }
        for r in out.requests {
            match r {
                Request::Print => self.open_print(),
                Request::GoToPage(p) => {
                    if let Some(i) = view {
                        self.views[i].go_to_page(p);
                    }
                }
                Request::LaunchUrl(u) => self.request_document_url(&u, crate::LinkOrigin::Script),
                Request::Submit(u) => self.notify_fmt(
                    "The form asks to be submitted to {u}; PdfCraft doesn't send form data. Save the document to keep your entries.",
                    &[("u", &u)],
                ),
                Request::SaveAs => self.run_command("file.save_as"),
                Request::Focus(_) | Request::Beep | Request::Reset(_) => {}
            }
        }
        if let Some(a) = out.alerts.last() {
            self.notify(a.clone());
        }
    }

    /// Run a push button's JavaScript (its Mouse Up action).
    pub fn run_button_script(&mut self, id: DocId, field: &str, script: &str) {
        // A script reads the fields: include what's still being typed in one (#166).
        if !self.commit_form_typing() {
            return;
        }
        match self.session.run_javascript(id, script, Some(field)) {
            Ok(o) => {
                if let Some(i) = self.views.iter().position(|v| v.id == id)
                    && let Some(info) = self.session.get(id).map(|d| d.info.clone())
                {
                    self.views[i].document_changed(&info);
                }
                let out = JsOutput { alerts: o.alerts, console: o.console, requests: o.requests, errors: o.error.into_iter().collect() };
                self.handle_js(id, out);
            }
            Err(e) => self.notify_fmt("{field}: {e}", &[("field", field), ("e", &e.to_string())]),
        }
    }

    /// Prepare a form ▸ detect fields from the page's blanks, lines and boxes.
    pub fn detect_fields(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        match self.session.auto_detect_fields(id, &[]) {
            Ok(names) if names.is_empty() => self.notify_tr("No form fields were detected"),
            Ok(names) => {
                if let Some(info) = self.session.get(id).map(|d| d.info.clone()) {
                    self.views[i].document_changed(&info);
                }
                if names.len() == 1 {
                    self.notify_tr("Detected 1 form field");
                } else {
                    self.notify_fmt("Detected {n} form fields", &[("n", &names.len().to_string())]);
                }
            }
            Err(e) => self.notify_error(e),
        }
    }

    /// The console's Run: evaluate the input in the active document.
    pub fn run_console(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        let script = self.js_console.input.clone();
        if script.trim().is_empty() {
            return;
        }
        self.js_console.log.push(format!("> {}", script.trim()));
        match self.session.run_javascript(id, &script, None) {
            Ok(o) => {
                if let Some(info) = self.session.get(id).map(|d| d.info.clone()) {
                    self.views[i].document_changed(&info);
                }
                let (error, result) = (o.error.clone(), o.result.clone());
                let out = JsOutput { alerts: o.alerts, console: o.console, requests: o.requests, errors: Vec::new() };
                self.handle_js(id, out);
                match error {
                    Some(e) => self.js_console.log.push(format!("Error: {e}")),
                    None => self.js_console.log.extend(result),
                }
            }
            Err(e) => self.js_console.log.push(format!("Error: {e}")),
        }
    }
}

fn buttons(ui: &mut egui::Ui, primary: &str, others: &[&str]) -> Option<String> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Stable ids: the console's output above changes how many widgets come first.
            // Labels are translated for display; the returned id stays English.
            if ui.push_id(primary, |ui| widgets::pill_button(ui, tl!(primary), true)).inner.clicked() {
                clicked = Some(primary.to_string());
            }
            for o in others {
                if ui.push_id(o, |ui| widgets::pill_button(ui, tl!(o), false)).inner.clicked() {
                    clicked = Some(o.to_string());
                }
            }
        })
    });
    clicked
}

/// The JavaScript console. Returns `true` to close.
pub(crate) fn console_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    ui.label(egui::RichText::new(tl!("JavaScript Console")).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    if !app.session.javascript() {
        ui.label(egui::RichText::new(tl!("JavaScript is turned off (Preferences ▸ JavaScript).")).small().color(t.text_muted));
    }
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(220.0).stick_to_bottom(true).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(160.0);
            if app.js_console.log.is_empty() {
                ui.label(egui::RichText::new(tl!("Output appears here.")).color(t.text_muted));
            }
            for line in &app.js_console.log {
                ui.label(egui::RichText::new(line).monospace());
            }
        });
    });
    ui.add_space(6.0);
    let input = ui.add(
        crate::widgets::block(&mut app.js_console.input)
            .code_editor()
            .desired_rows(4)
            .desired_width(f32::INFINITY)
            .hint_text(tl!("JavaScript, e.g. getField(\"total\").value"))
            .id_salt("js-console-input"),
    );
    let run_key = input.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
    ui.add_space(8.0);
    match buttons(ui, "Run", &["Close", "Clear"]).as_deref() {
        Some("Run") => app.run_console(),
        Some("Clear") => app.js_console.log.clear(),
        Some("Close") => return true,
        _ if run_key => app.run_console(),
        _ => {}
    }
    false
}

/// Document JavaScripts: list, edit, add and delete. Returns `true` to close.
pub(crate) fn document_js_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    ui.label(egui::RichText::new(tl!("Document JavaScripts")).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    let scripts = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|d| d.document_scripts()).unwrap_or_default();
    let mut edit: Option<Edit> = None;
    ui.horizontal(|ui| {
        ui.label(tl!("Script Name:"));
        ui.add(crate::widgets::line(&mut app.doc_js.name).desired_width(240.0).id_salt("doc-js-name"));
    });
    ui.add_space(4.0);
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(120.0).id_salt("doc-js-list").show(ui, |ui| {
            ui.set_width(ui.available_width());
            if scripts.is_empty() {
                ui.label(egui::RichText::new(tl!("This document has no document-level scripts.")).color(t.text_muted));
            }
            for (name, js) in &scripts {
                if ui.selectable_label(app.doc_js.name == *name, name).clicked() {
                    app.doc_js = DocJsDraft { name: name.clone(), script: js.clone() };
                }
            }
        });
    });
    ui.add_space(6.0);
    ui.add(crate::widgets::block(&mut app.doc_js.script).code_editor().desired_rows(8).desired_width(f32::INFINITY).id_salt("doc-js-script"));
    ui.add_space(8.0);
    let name = app.doc_js.name.trim().to_string();
    let close = match buttons(ui, "Save", &["Close", "Delete"]).as_deref() {
        Some("Save") if !name.is_empty() => {
            edit = Some(Edit::SetDocumentScript { name, script: Some(app.doc_js.script.clone()) });
            false
        }
        Some("Delete") if scripts.iter().any(|(n, _)| *n == name) => {
            edit = Some(Edit::SetDocumentScript { name, script: None });
            app.doc_js = DocJsDraft::default();
            false
        }
        Some("Close") => true,
        _ => false,
    };
    if let Some(e) = edit {
        app.apply_edit(e);
    }
    close
}

/// The label for a [`pdfcraft_render::ReadingColors`] mode name (`mode_name`).
fn reading_mode_label(mode: &str) -> &'static str {
    match mode {
        "black-on-white" => "Black on white",
        "white-on-black" => "White on black",
        "yellow-on-black" => "Yellow on black",
        "green-on-black" => "Green on black",
        "custom" => "Custom",
        _ => "Off",
    }
}

/// Preferences: interface language and size, appearance, identity and JavaScript. Returns `true` to close.
pub(crate) fn preferences_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> bool {
    ui.label(egui::RichText::new(tl!("Preferences")).font(theme::semibold(18.0)));
    ui.horizontal(|ui| {
        ui.label(tl!("Interface language"));
        let selected = crate::i18n::Lang::from_code(&app.language).map_or(tl!("Auto"), crate::i18n::Lang::name);
        let before = app.language.clone();
        egui::ComboBox::from_id_salt("interface-language").selected_text(selected).show_ui(ui, |ui| {
            if ui.selectable_value(&mut app.language, crate::i18n::AUTO.to_string(), tl!("Auto")).clicked() {
                ui.close();
            }
            for language in crate::i18n::Lang::all() {
                if ui.selectable_value(&mut app.language, language.code().to_string(), language.name()).clicked() {
                    ui.close();
                }
            }
        });
        // Relabel the rest of this dialog in the new language right away, not next frame.
        if app.language != before {
            crate::i18n::set_current(crate::i18n::Lang::from_pref(&app.language));
        }
    });
    ui.horizontal(|ui| {
        use crate::ui_scale;
        ui.label(tl!("Interface size"));
        // Auto shows the size it draws at: the display's scale times the system's text scale.
        let native = ui.ctx().native_pixels_per_point().unwrap_or(1.0);
        let auto_percent = ui_scale::percent(ui_scale::drawn(None, app.system_text_scale, native));
        let auto = crate::i18n::fmt(tl!("Auto ({percent}%)"), &[("percent", &auto_percent.to_string())]);
        let selected = app.ui_scale.map_or_else(|| auto.clone(), |f| format!("{}%", ui_scale::percent(f)));
        egui::ComboBox::from_id_salt("interface-size").selected_text(selected).show_ui(ui, |ui| {
            if ui.selectable_value(&mut app.ui_scale, None, auto).clicked() {
                ui.close();
            }
            for p in ui_scale::PRESETS {
                let factor = f32::from(p) / 100.0;
                if ui.selectable_label(app.ui_scale.is_some_and(|f| ui_scale::percent(f) == u32::from(p)), format!("{p}%")).clicked() {
                    app.ui_scale = Some(factor);
                    ui.close();
                }
            }
        });
    });
    ui.label(
        egui::RichText::new(tl!("Auto follows the system's text size. Pages always render at the display's full resolution."))
            .small()
            .color(t.text_muted),
    );
    ui.horizontal(|ui| {
        ui.label(tl!("Tools panel side"));
        let right = app.docking.tools_on_right();
        if ui.selectable_label(!right, tl!("Left")).clicked()
            && let Err(error) = app.docking.set_tools_on_right(false)
        {
            app.notify(error);
        }
        if ui.selectable_label(right, tl!("Right")).clicked()
            && let Err(error) = app.docking.set_tools_on_right(true)
        {
            app.notify(error);
        }
    });
    ui.add_space(8.0);
    crate::appearance::preferences_section(ui, app, t);
    ui.add_space(8.0);
    ui.label(egui::RichText::new(tl!("Documents and view")).font(theme::semibold(13.0)));
    ui.horizontal(|ui| {
        ui.label(tl!("Default workspace mode"));
        for (mode, label) in [
            (crate::Mode::AllTools, "All tools"),
            (crate::Mode::Read, "Read"),
            (crate::Mode::Edit, "Edit"),
            (crate::Mode::Convert, "Convert"),
            (crate::Mode::Sign, "E-Sign"),
        ] {
            widgets::radio_value(ui, &mut app.default_mode, mode, tl!(label));
        }
    });
    ui.label(
        egui::RichText::new(tl!("Used when opening PDFs. An explicit launch or control mode takes precedence for the session."))
            .small()
            .color(t.text_muted),
    );
    let defaults = &mut app.view_defaults;
    ui.horizontal(|ui| {
        ui.label(tl!("Default page display"));
        for l in crate::canvas::PageLayout::ORDER {
            widgets::radio_value(ui, &mut defaults.layout, l, tl!(l.label()));
        }
        ui.add(widgets::checkbox(&mut defaults.continuous, tl!("Continuous scrolling")));
    });
    ui.horizontal(|ui| {
        use crate::canvas::Fit;
        ui.label(tl!("Default zoom"));
        widgets::radio_value(ui, &mut defaults.fit, Fit::Width, tl!("Fit to width"));
        widgets::radio_value(ui, &mut defaults.fit, Fit::Page, tl!("Zoom to page level"));
        widgets::radio_value(ui, &mut defaults.fit, Fit::None, tl!("Custom"));
        // Editing the percentage selects Custom.
        let mut percent = defaults.zoom * 100.0;
        if ui.add(egui::DragValue::new(&mut percent).range(8.0..=6400.0).max_decimals(0).suffix("%")).changed() {
            (defaults.fit, defaults.zoom) = (Fit::None, percent / 100.0);
        }
    });
    ui.label(
        egui::RichText::new(tl!("Used when a PDF doesn't ask for a layout or zoom. Continuous scrolling never snaps between pages."))
            .small()
            .color(t.text_muted),
    );
    ui.horizontal(|ui| {
        ui.label(tl!("Units"));
        for u in crate::units::Unit::ALL {
            widgets::radio_value(ui, &mut app.units, u, u.label());
        }
    });
    ui.label(egui::RichText::new(tl!("For page sizes and margins.")).small().color(t.text_muted));
    // The web build has no file paths to reopen.
    #[cfg(not(target_arch = "wasm32"))]
    ui.add(widgets::checkbox(&mut app.reopen_last_session, tl!("Reopen the files that were open when PdfCraft last closed")));
    // The web build has no files on disk to watch.
    #[cfg(not(target_arch = "wasm32"))]
    ui.add(widgets::checkbox(&mut app.reload_changed_files, tl!("Reload documents when their files change on disk")));
    ui.horizontal(|ui| {
        ui.label(tl!("Scroll speed (mouse wheel and arrow keys)"));
        for speed in crate::scroll_speed::ScrollSpeed::ALL {
            widgets::radio_value(ui, &mut app.scroll_speed, speed, tl_ctx!("scroll speed", speed.label()));
        }
    });
    // Whether anything animates, the wheel's scrolling and zooming included; left alone it
    // follows the system's animation setting.
    ui.horizontal(|ui| {
        ui.label(tl!("Animation effects"));
        for choice in crate::motion::AnimationEffects::ALL {
            widgets::radio_value(ui, &mut app.animation_effects, choice, tl_ctx!("animation effects", choice.label()));
        }
    });
    // Only where the app draws the title bar (GNOME on Wayland); elsewhere the desktop draws it.
    if app.window_controls.is_some() {
        ui.add(widgets::checkbox(&mut app.system_title_bar, tl!("Use system title bar")))
            .on_hover_text(tl!("Show a basic title bar for the window buttons instead of the tab strip. GNOME doesn't draw title bars for apps, so this one doesn't follow its theme."));
    }
    ui.add_space(8.0);
    // Reading colours (Acrobat: Replace Document Colors): solid text and line-art paints are
    // remapped while rendering, and the page clears to the background colour. The preference
    // is app-wide: every open document follows it, and documents open with it.
    ui.label(egui::RichText::new(tl!("Reading colours")).font(theme::semibold(13.0)));
    ui.horizontal(|ui| {
        ui.label(tl!("Replace document colours"));
        let mode = app.reading_colors.mode_name();
        egui::ComboBox::from_id_salt("reading-colors").selected_text(tl!(reading_mode_label(mode))).show_ui(ui, |ui| {
            for (m, label) in [
                ("off", "Off"),
                ("black-on-white", "Black on white"),
                ("white-on-black", "White on black"),
                ("yellow-on-black", "Yellow on black"),
                ("green-on-black", "Green on black"),
                ("custom", "Custom"),
            ] {
                if ui.selectable_label(mode == m, tl!(label)).clicked() {
                    match m {
                        "off" => app.set_reading_colors(pdfcraft_render::ReadingColors::default()),
                        "custom" => {
                            // Custom starts from the colours already chosen, or black on white.
                            let colors = app.reading_colors.params.map_or(pdfcraft_render::ReadingColors::BLACK_ON_WHITE, |params| {
                                pdfcraft_render::ReadingColors { params: Some(params) }
                            });
                            app.set_reading_colors(colors);
                        }
                        // `parse` knows these names; a preset comes with its own flags.
                        preset => {
                            if let Ok(colors) = pdfcraft_render::ReadingColors::parse(preset, None, None, None, None) {
                                app.set_reading_colors(colors);
                            }
                        }
                    }
                    ui.close();
                }
            }
        });
    });
    if app.reading_colors.params.is_some() {
        let mut changed = false;
        if let Some(params) = app.reading_colors.params.as_mut() {
            ui.horizontal(|ui| {
                let text = ui.label(tl!("Text"));
                changed |= ui.color_edit_button_srgb(&mut params.text).labelled_by(text.id).changed();
                let background = ui.label(tl!("Page background"));
                changed |= ui.color_edit_button_srgb(&mut params.background).labelled_by(background.id).changed();
            });
            if ui.add(crate::widgets::checkbox(&mut params.only_black, tl!("Only change the colour of black text or line art"))).changed() {
                changed = true;
            }
            if ui.add(crate::widgets::checkbox(&mut params.line_art, tl!("Change the colour of line art as well as text"))).changed() {
                changed = true;
            }
        }
        if changed {
            app.set_reading_colors(app.reading_colors);
        }
    }
    ui.label(egui::RichText::new(tl!("Colours are remapped while rendering; the document itself never changes.")).small().color(t.text_muted));
    ui.add_space(8.0);
    // Identity: the author of new comments (Acrobat: Preferences ▸ Identity).
    ui.label(egui::RichText::new(tl!("Identity")).font(theme::semibold(13.0)));
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Name on new comments"));
        ui.add(crate::widgets::line(&mut app.comment_prefs.author).desired_width(220.0).char_limit(crate::MAX_AUTHOR_CHARS)).labelled_by(label.id);
    });
    ui.add(widgets::checkbox(&mut app.comment_prefs.hide_empty_highlight_hover, tl!("Hide hover previews for highlights without comment text")));
    ui.add_space(8.0);
    ui.label(egui::RichText::new(tl!("Fill & Sign")).font(theme::semibold(13.0)));
    ui.add(widgets::checkbox(&mut app.flatten_fill_sign_on_save, tl!("Flatten Fill & Sign when saving")));
    ui.label(
        egui::RichText::new(tl!("Text, marks and signatures become part of the page. Other comments stay editable.")).small().color(t.text_muted),
    );
    // Signatures ▸ Time server (RFC 3161). The browser build has no time servers.
    #[cfg(not(target_arch = "wasm32"))]
    {
        ui.add_space(8.0);
        crate::sign_ui::time_server_preferences(ui, app, t);
    }
    ui.add_space(8.0);
    date_format(ui, app, t);
    ui.add_space(8.0);
    ui.label(egui::RichText::new("JavaScript").font(theme::semibold(13.0)));
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let mut on = app.session.javascript();
        if ui.add(widgets::checkbox(&mut on, tl!("Enable Acrobat JavaScript"))).changed() {
            app.session.set_javascript(on);
        }
        ui.label(
            egui::RichText::new(tl!("Scripts run in a sandbox without file or network access. A script that asks to open a web page needs your permission first. With JavaScript off, Acrobat's standard format, validate and calculate functions still work."))
                .small()
                .color(t.text_muted),
        );
    });
    ui.add_space(10.0);
    buttons(ui, tl!("OK"), &[]).is_some()
}

/// Preferences ▸ Date format: a preset, or any pattern typed in, for Fill & Sign dates, and the
/// language of its month and weekday names. A valid pattern applies as it's typed; an invalid
/// one stays in the box with the reason, and the last valid one stays in use.
fn date_format(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    use pdfcraft_engine::dates::{DATE_FORMATS, DATE_LANGUAGES, MAX_DATE_FORMAT_CHARS};
    ui.label(egui::RichText::new(tl!("Date format")).font(theme::semibold(13.0)));
    let sample = |app: &PdfCraftApp, f: &str| format!("{}   {f}", app.date_text(Some(f)).unwrap_or_default());
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Fill & Sign dates"));
        let current = app.session.date_format().to_string();
        let selected = if DATE_FORMATS.contains(&current.as_str()) { sample(app, &current) } else { tl!("Custom").to_string() };
        egui::ComboBox::from_id_salt("date-format")
            .selected_text(selected)
            .width(240.0)
            .show_ui(ui, |ui| {
                for f in DATE_FORMATS {
                    if ui.selectable_label(current == f, sample(app, f)).clicked() && app.session.set_date_format(f).is_ok() {
                        app.date_format_draft = None;
                        ui.close();
                    }
                }
            })
            .response
            .labelled_by(label.id);
    });
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Pattern"));
        let mut text = app.date_format_draft.clone().unwrap_or_else(|| app.session.date_format().to_string());
        let edit = ui.add(egui::TextEdit::singleline(&mut text).desired_width(160.0).char_limit(MAX_DATE_FORMAT_CHARS)).labelled_by(label.id);
        // The box keeps the text as typed (a trailing space included) while it has focus; the
        // setting takes the trimmed pattern whenever it's valid.
        if edit.changed() {
            let _ = app.session.set_date_format(&text);
            app.date_format_draft = Some(text.clone());
        }
        if !edit.has_focus() && pdfcraft_engine::dates::check_date_format(&text).is_ok() {
            app.date_format_draft = None;
        }
        match app.date_text(Some(&text)) {
            Ok(today) => ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Today: {date}"), &[("date", &today)])).color(t.text_muted)),
            // The engine's reason, in English.
            Err(e) => ui.label(egui::RichText::new(e).color(egui::Color32::from_rgb(0xD1, 0x3B, 0x3B))),
        };
    });
    // Fill & Sign refuses a date it would save as "?"; say so before anyone tries.
    let bad = app.date_text(None).map(|today| pdfcraft_engine::dates::unwritable(&today)).unwrap_or_default();
    if !bad.is_empty() {
        ui.label(
            egui::RichText::new(crate::i18n::fmt(
                tl!(
                    "{chars} can't be written into the PDF yet: Fill & Sign text is Western European only. Pick a numeric format such as dd.mm.yyyy."
                ),
                &[("chars", &bad)],
            ))
            .color(egui::Color32::from_rgb(0xD1, 0x3B, 0x3B)),
        );
    }
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Language"));
        let interface = crate::i18n::current().name();
        let follow = crate::i18n::fmt(tl!("Same as interface ({language})"), &[("language", interface)]);
        let current = app.session.date_language().map(str::to_string);
        let selected = DATE_LANGUAGES.iter().find(|l| current.as_deref() == Some(l.code)).map_or(follow.clone(), |l| l.name.to_string());
        egui::ComboBox::from_id_salt("date-language")
            .selected_text(selected)
            .width(240.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), follow).clicked() && app.session.set_date_language(None).is_ok() {
                    ui.close();
                }
                for l in &DATE_LANGUAGES {
                    if ui.selectable_label(current.as_deref() == Some(l.code), l.name).clicked()
                        && app.session.set_date_language(Some(l.code)).is_ok()
                    {
                        ui.close();
                    }
                }
            })
            .response
            .labelled_by(label.id);
    });
    ui.label(
        egui::RichText::new(crate::i18n::fmt(
            tl!("yyyy or yy year · m or mm month · mmm or mmmm month name · d or dd day · ddd or dddd weekday. H, h, M, s and t are time letters and can't be used on their own: put {escape} before a letter to show it as is."),
            &[("escape", "\\")],
        ))
        .small()
        .color(t.text_muted),
    );
}
