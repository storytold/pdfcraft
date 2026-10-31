//! Acrobat JavaScript in the shell: what scripts ask for (alerts, printing, navigation, links),
//! the JavaScript console (⌘J), Document JavaScripts, and Preferences ▸ JavaScript.

use egui::{Align, Layout};
use pdfcraft_engine::js::{JsOutput, Message, MessageIcon, Panel, Request};
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

/// Most form script messages that wait for the toast; past it, more are left out and counted, and
/// the toast says how many once the rest have been shown. One run shows at most this many (the
/// script engine's own limit), so only runs in quick succession reach it.
const MAX_WAITING_MESSAGES: usize = 100;

/// The icon a script message's toast shows for `icon`.
pub(crate) fn message_icon(icon: MessageIcon) -> &'static str {
    match icon {
        MessageIcon::Error => "circle-x",
        MessageIcon::Warning => "triangle-alert",
        MessageIcon::Question => "circle-help",
        MessageIcon::Status => "info",
    }
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
                Request::ShowPanel(p) => {
                    let panel = match p {
                        Panel::Bookmarks => crate::RightPanel::Bookmarks,
                        Panel::Pages => crate::RightPanel::Pages,
                        Panel::Attachments => crate::RightPanel::Attachments,
                        Panel::Signatures => crate::RightPanel::Signatures,
                        Panel::Layers => crate::RightPanel::Layers,
                    };
                    // Show/Hide: a panel already showing closes.
                    self.right = (self.right != Some(panel)).then_some(panel);
                }
                Request::Focus(_) | Request::Beep | Request::Reset(_) => {}
            }
        }
        self.show_script_messages(out.alerts);
    }

    /// Show form script messages `messages` in order, each in the toast with its title and icon.
    /// The first takes the toast at once when it is free; otherwise it waits, like the rest, for
    /// what the toast shows (another message, or a notice such as the run's own submit request)
    /// to go ([`Self::next_script_message`]).
    pub fn show_script_messages(&mut self, messages: Vec<Message>) {
        let room = MAX_WAITING_MESSAGES.saturating_sub(self.script_messages.len());
        let left_out = u64::try_from(messages.len().saturating_sub(room)).unwrap_or(u64::MAX);
        self.script_messages_dropped = self.script_messages_dropped.saturating_add(left_out);
        self.script_messages.extend(messages.into_iter().take(room));
        if self.toast.is_none() {
            self.next_script_message();
        }
    }

    /// Put the next waiting script message in the toast, or, once none waits, a notice of how many
    /// the queue had no room for. False when there is neither.
    pub fn next_script_message(&mut self) -> bool {
        let Some(m) = self.script_messages.pop_front() else {
            // Every message the queue held has been shown: say how many it had no room for.
            if self.script_messages_dropped == 0 {
                return false;
            }
            let n = std::mem::take(&mut self.script_messages_dropped);
            self.notify(crate::i18n::trn(
                crate::i18n::current(),
                n,
                "{n} more message from the form's scripts was left out.",
                "{n} more messages from the form's scripts were left out.",
            ));
            // Kept with the toast until it has been shown its time (the toast's own expiry).
            self.toast_dropped = n;
            return true;
        };
        self.toast = Some((m.text.clone(), 0.0));
        self.toast_script = Some(m);
        true
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
                let out = JsOutput {
                    alerts: o.alerts,
                    console: o.console,
                    requests: o.requests,
                    errors: o.error.into_iter().collect(),
                    doc_errors: Vec::new(),
                };
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
                let out = JsOutput { alerts: o.alerts, console: o.console, requests: o.requests, errors: Vec::new(), doc_errors: Vec::new() };
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

/// The height the Preferences dialog keeps below its scrolling settings for the OK row: the
/// button, and the space above it.
pub(crate) const PREFERENCES_BUTTON_ROW: f32 = 52.0;

/// The least height the scrolling settings are given. The dialog around them takes about 80 more,
/// so this still fits the smallest window (520 pt) at the largest interface size (300%, which
/// leaves 173 pt): a taller floor would push OK off the screen again.
pub(crate) const PREFERENCES_MIN_SCROLL: f32 = 60.0;

/// Whether the interface can draw a language picker's native name. On the web the browser's glyph
/// rasterizer draws scripts no embedded face has, so the native names stay; on native, ask the
/// fonts this build installed (including an installed fallback face, unless system fonts are off).
fn picker_text_is_drawable(ui: &egui::Ui, text: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (ui, text);
        true
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        ui.fonts_mut(|fonts| fonts.has_glyphs(&egui::FontId::proportional(13.0), text))
    }
}

/// The date picker's displayed name. Its languages are the interface languages, so it uses the
/// same drawable-name fallback: a native name whose script this build has no face for would
/// otherwise abort under egui 0.37's `MissingGlyphPolicy::Panic` (for example Hindi without a
/// Devanagari face and installed system fonts turned off).
fn date_language_picker_name(l: &pdfcraft_engine::dates::DateLanguage, drawable: impl FnOnce(&str) -> bool) -> &'static str {
    match crate::i18n::Lang::from_code(l.code) {
        Some(language) => language.picker_name(drawable),
        None => l.name,
    }
}

/// Preferences: interface language and size, appearance, identity and JavaScript. Drawn in a scroll
/// area; [`preferences_buttons`] goes below it, outside the scroll, so OK is always in reach.
pub(crate) fn preferences_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    ui.label(egui::RichText::new(tl!("Preferences")).font(theme::semibold(18.0)));
    ui.horizontal(|ui| {
        let label = ui.label(tl!("Interface language"));
        // A native name is only drawn when these fonts can draw it; otherwise the English name
        // keeps the list readable instead of aborting on a missing glyph (for example Hindi
        // without a Devanagari face, and installed system fonts turned off).
        let (selected, picker) = {
            let drawable = |text: &str| picker_text_is_drawable(ui, text);
            let selected = crate::i18n::Lang::from_code(&app.language)
                .map_or_else(|| tl!("Auto").to_owned(), |language| language.picker_name(drawable).to_owned());
            let picker = crate::i18n::Lang::for_picker_by(|language| language.picker_name(drawable))
                .into_iter()
                .map(|language| (language, language.picker_name(drawable)))
                .collect::<Vec<_>>();
            (selected, picker)
        };
        let before = app.language.clone();
        craft_ui::ComboBox::from_id_salt("interface-language")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                if ui.selectable_value(&mut app.language, crate::i18n::AUTO.to_string(), tl!("Auto")).clicked() {
                    ui.close();
                }
                for (language, name) in &picker {
                    if ui.selectable_value(&mut app.language, language.code().to_string(), *name).clicked() {
                        ui.close();
                    }
                }
            })
            .response
            .labelled_by(label.id);
        // Relabel the rest of this dialog in the new language right away, not next frame.
        if app.language != before {
            crate::i18n::set_current(crate::i18n::Lang::from_pref(&app.language));
        }
    });
    ui.horizontal(|ui| {
        use crate::ui_scale;
        let label = ui.label(tl!("Interface size"));
        // Auto shows the size it draws at: the display's scale times the system's text scale.
        let native = ui.ctx().native_pixels_per_point().unwrap_or(1.0);
        let auto_percent = ui_scale::percent(ui_scale::drawn(None, app.system_text_scale, native));
        let auto = crate::i18n::fmt(tl!("Auto ({percent}%)"), &[("percent", &auto_percent.to_string())]);
        let selected = app.ui_scale.map_or_else(|| auto.clone(), |f| format!("{}%", ui_scale::percent(f)));
        craft_ui::ComboBox::from_id_salt("interface-size")
            .selected_text(selected)
            .show_ui(ui, |ui| {
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
            })
            .response
            .labelled_by(label.id);
    });
    ui.label(
        egui::RichText::new(tl!("Auto follows the system's text size. Pages always render at the display's full resolution."))
            .small()
            .color(t.text_muted),
    );
    ui.horizontal(|ui| {
        ui.label(tl!("Tools panel side"));
        let right = app.side_panels.tools_on_right();
        if ui.selectable_label(!right, tl!("Left")).clicked() {
            app.side_panels.tools_side = crate::side_panels::ToolsSide::Left;
        }
        if ui.selectable_label(right, tl!("Right")).clicked() {
            app.side_panels.tools_side = crate::side_panels::ToolsSide::Right;
        }
    });
    ui.add_space(8.0);
    crate::appearance::preferences_section(ui, app, t);
    ui.add_space(8.0);
    reading_colours_section(ui, app, t);
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
    ui.add(widgets::checkbox(&mut defaults.show_page_gaps, tl!("Show gaps between pages")));
    ui.label(egui::RichText::new(tl!("Hide gaps to join page edges. Margins inside the PDF remain unchanged.")).small().color(t.text_muted));
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
    ui.add(widgets::checkbox(&mut app.always_default_view, tl!("Always use my default zoom and page layout")));
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
    // Acrobat's "Restore last view settings when reopening documents" (#1767).
    #[cfg(not(target_arch = "wasm32"))]
    ui.add(widgets::checkbox(&mut app.restore_last_view, tl!("Restore the last view (page, zoom and page display) when a document is opened again")));
    // The web build has no files on disk to watch.
    #[cfg(not(target_arch = "wasm32"))]
    ui.add(widgets::checkbox(&mut app.reload_changed_files, tl!("Reload documents when their files change on disk")));
    // The notice's "Don't show again" turns this off (#4035); the restrictions stay either way.
    ui.add(widgets::checkbox(&mut app.show_secured_notice, tl!("Show a notice when a document is secured")));
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
}

/// The Preferences dialog's OK row. Returns `true` to close.
pub(crate) fn preferences_buttons(ui: &mut egui::Ui) -> bool {
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
        craft_ui::ComboBox::from_id_salt("date-format")
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
        // Build every label before the combo box mutably borrows `ui`, with the same drawable
        // fallback the interface language picker uses.
        let (follow, selected, current, names) = {
            let drawable = |text: &str| picker_text_is_drawable(ui, text);
            let interface = crate::i18n::current().picker_name(drawable);
            let follow = crate::i18n::fmt(tl!("Same as interface ({language})"), &[("language", interface)]);
            let current = app.session.date_language().map(str::to_string);
            let selected = DATE_LANGUAGES
                .iter()
                .find(|l| current.as_deref() == Some(l.code))
                .map_or_else(|| follow.clone(), |l| date_language_picker_name(l, drawable).to_string());
            let names = DATE_LANGUAGES.iter().map(|l| (l, date_language_picker_name(l, drawable))).collect::<Vec<_>>();
            (follow, selected, current, names)
        };
        craft_ui::ComboBox::from_id_salt("date-language")
            .selected_text(selected)
            .width(240.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), follow).clicked() && app.session.set_date_language(None).is_ok() {
                    ui.close();
                }
                for (l, name) in &names {
                    if ui.selectable_label(current.as_deref() == Some(l.code), *name).clicked() && app.session.set_date_language(Some(l.code)).is_ok()
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

fn reading_colours_section(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    // Reading colours (Acrobat: Replace Document Colors): solid text and line-art paints are
    // remapped while rendering, and the page clears to the background colour. The preference
    // is app-wide: every open document follows it, and documents open with it.
    ui.label(egui::RichText::new(tl!("Reading colours")).font(theme::semibold(13.0)));
    let mode = app.reading_colors.mode_name();
    ui.horizontal_wrapped(|ui| {
        for (name, label, colors) in [
            ("off", "Off", pdfcraft_render::ReadingColors::default()),
            ("black-on-white", "Black on white", pdfcraft_render::ReadingColors::BLACK_ON_WHITE),
            ("white-on-black", "White on black", pdfcraft_render::ReadingColors::WHITE_ON_BLACK),
            ("yellow-on-black", "Yellow on black", pdfcraft_render::ReadingColors::YELLOW_ON_BLACK),
            ("green-on-black", "Green on black", pdfcraft_render::ReadingColors::GREEN_ON_BLACK),
        ] {
            let params = colors.params.unwrap_or(pdfcraft_render::ReadingParams {
                text: [0, 0, 0],
                background: [255, 255, 255],
                only_black: false,
                line_art: true,
            });
            ui.vertical(|ui| {
                ui.set_width(104.0);
                let sample = egui::RichText::new("Aa").size(22.0).color(egui::Color32::from_rgb(params.text[0], params.text[1], params.text[2]));
                let button = egui::Button::new(sample)
                    .fill(egui::Color32::from_rgb(params.background[0], params.background[1], params.background[2]))
                    .min_size(egui::vec2(94.0, 44.0))
                    .stroke(egui::Stroke::new(if mode == name { 2.0 } else { 1.0 }, if mode == name { t.accent } else { t.border }));
                let response = ui.add(button);
                response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, mode == name, tl!(label)));
                if response.clicked() {
                    app.set_reading_colors(colors);
                }
                ui.label(tl!(label));
            });
        }
    });
    ui.add_space(8.0);
    if let Some(params) = app.reading_colors.params {
        let text = egui::Color32::from_rgb(params.text[0], params.text[1], params.text[2]);
        let background = egui::Color32::from_rgb(params.background[0], params.background[1], params.background[2]);
        egui::Frame::new().fill(background).inner_margin(16.0).show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 32.0).max(0.0));
            ui.label(egui::RichText::new(tl!("Reading colours")).size(18.0).color(text));
            ui.label(egui::RichText::new(tl!("Colours are remapped while rendering; the document itself never changes.")).color(text));
        });
    } else {
        ui.label(tl!("Original"));
    }
    egui::CollapsingHeader::new(tl!("Custom")).default_open(mode == "custom").show(ui, |ui| {
        let mut colors = if app.reading_colors.params.is_some() { app.reading_colors } else { pdfcraft_render::ReadingColors::BLACK_ON_WHITE };
        let mut changed = false;
        if let Some(params) = colors.params.as_mut() {
            ui.horizontal(|ui| {
                let text = ui.label(tl!("Text"));
                changed |= ui.color_edit_button_srgb(&mut params.text).labelled_by(text.id).changed();
                let background = ui.label(tl!("Page background"));
                changed |= ui.color_edit_button_srgb(&mut params.background).labelled_by(background.id).changed();
            });
            changed |= ui.add(crate::widgets::checkbox(&mut params.only_black, tl!("Only change the colour of black text or line art"))).changed();
            changed |= ui.add(crate::widgets::checkbox(&mut params.line_art, tl!("Change the colour of line art as well as text"))).changed();
        }
        if changed {
            app.set_reading_colors(colors);
        }
    });
    ui.label(egui::RichText::new(tl!("Colours are remapped while rendering; the document itself never changes.")).small().color(t.text_muted));
    ui.add_space(8.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_language_names_fall_back_when_the_native_name_cannot_be_drawn() {
        let date_language = |code: &str| pdfcraft_engine::dates::DATE_LANGUAGES.iter().find(|l| l.code == code).expect("date language");
        let hindi = date_language("hi");
        assert_eq!(date_language_picker_name(hindi, |_| true), hindi.name, "the native name is kept when it can be drawn");
        assert_eq!(date_language_picker_name(hindi, |_| false), "Hindi", "no Devanagari face: show the English name");
        let chinese = date_language("zh-hans");
        assert_eq!(date_language_picker_name(chinese, |_| false), "Simplified Chinese");
        for language in &pdfcraft_engine::dates::DATE_LANGUAGES {
            assert!(!date_language_picker_name(language, |_| false).is_empty(), "{}", language.code);
        }
    }
}
