use super::settings::{self, PdfCraftSettings};
use super::{PdfCraftApp, RedactPrefs, Session, actions_ui, canvas, combine_ui, fill_sign, i18n, print_paper, print_ui, stamps_ui, ui_scale};
use craft::theme::ModeChoice;
use craft::window::policy::TitleBarPref;
use pdfcraft_engine::print::names as print_names;

/// [`PdfCraftApp::date_text`] for callers that already borrow other parts of the app.
pub(crate) fn date_text(session: &Session, fmt: Option<&str>) -> Result<String, String> {
    let lang = session.date_language().unwrap_or(i18n::current().code());
    session.today_text(fmt, Some(lang))
}

/// Today in Preferences ▸ Date format for stamping into a PDF; refused when it has characters
/// Fill & Sign can't write yet (see [`pdfcraft_engine::dates::unwritable`]).
pub(crate) fn date_text_for_pdf(session: &Session) -> Result<String, String> {
    let lang = session.date_language().unwrap_or(i18n::current().code());
    session.today_text_for_pdf(None, Some(lang))
}

/// The longest author name kept (Preferences ▸ Identity, restored settings).
pub(crate) const MAX_AUTHOR_CHARS: usize = 200;

/// The saved text of a JSON-text setting, parsed; `None` when absent or not JSON.
fn parsed(text: &Option<String>) -> Option<serde_json::Value> {
    text.as_deref().and_then(|t| serde_json::from_str(t).ok())
}

/// `value` as the JSON text a setting keeps; `None` when it cannot be written.
fn json_text(value: &impl serde::Serialize) -> Option<String> {
    serde_json::to_string(value).ok()
}

/// The name a unit-variant value serialises as (a side, a sort order); `None` for any other shape.
fn name_of(value: &impl serde::Serialize) -> Option<String> {
    serde_json::to_value(value).ok()?.as_str().map(str::to_string)
}

/// The value a saved name stands for; `None` for a name that is not one of its variants.
fn from_name<T: serde::de::DeserializeOwned>(name: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(name.to_string())).ok()
}

impl PdfCraftApp {
    /// The canvas's navigation choices: the scroll speed, the middle-button direction and whether zooms and wheel
    /// notches ease. The rest keep the framework's defaults.
    pub(crate) fn navigation_options(&self) -> craft::ui::canvas::NavigationOptions {
        let mut options = craft::ui::canvas::NavigationOptions::default();
        options.wheel_line = self.scroll_speed.wheel_line();
        options.key_line = self.scroll_speed.key_line();
        options.invert_middle = self.invert_middle_drag;
        // PdfCraft's Hand stops at release; it does not coast after a drag.
        options.flick_panning = false;
        // Linux latches a middle-click auto-scroll; the other platforms drag the view.
        if cfg!(target_os = "linux") {
            options.middle_button = craft::behavior::MiddleButton::AutoScroll;
        }
        // Production's autoscroll: the speed grows with the pointer's whole distance from the mark, a release inside a
        // short hold keeps it running, and it ends when the page view loses the keys.
        options.middle_autoscroll = craft::ui::canvas::MiddleAutoscrollFeel::Reader;
        let eased = !self.animation_effects.reduced(None);
        options.animated_zoom = eased;
        options.eased_wheel = eased;
        // A ⌘/Ctrl-wheel notch zooms over about a quarter of a second, as production's egui wheel did, whatever the
        // animation setting.
        options.eased_wheel_zoom = true;
        options
    }

    /// Sets the user's interface theme preference (Light, Dark, or System); the next frame's
    /// [`Self::root_env`] carries it.
    pub fn set_theme_preference(&mut self, preference: ModeChoice) {
        self.theme_preference = preference;
    }

    /// Sets Preferences ▸ Animation effects; the next frame's [`Self::root_env`] carries it and
    /// the next [`Self::save_settings`] keeps it.
    pub fn set_motion(&mut self, motion: craft::theme::MotionChoice) {
        self.animation_effects = motion;
    }

    /// The suite as Preferences ▸ Appearance shows it: the app's own mode and motion over the
    /// in-memory suite's themes.
    pub(crate) fn suite_view(&self) -> craft::theme_store::SuiteView {
        let mut view = self.suite.view();
        view.shown.mode = Some(self.theme_preference);
        view.shown.motion = Some(self.animation_effects);
        view
    }

    /// Applies what the Appearance page changed to the app's own settings.
    pub(crate) fn apply_selection(&mut self, change: craft::theme_store::SelectionChange) {
        if let Some(mode) = change.mode {
            self.set_theme_preference(mode);
        }
        if let Some(motion) = change.motion {
            self.set_motion(motion);
        }
    }

    /// Hands the preferences to the settings store and writes them if they changed. A write that
    /// fails is logged: the preferences stay in the app and the next save tries again.
    pub(crate) fn save_settings(&mut self) {
        let snapshot = self.settings_snapshot();
        if let Some(store) = self.settings.as_mut() {
            store.set(snapshot);
            if let Err(e) = store.flush() {
                log::warn!("the preferences could not be written: {e}");
            }
        }
    }

    /// The app-wide reading-colours preference: sets it and applies it to every open document,
    /// not just the active one (Preferences ▸ Reading colours, Acrobat's Replace Document
    /// Colors). A document the change actually touches re-renders.
    pub fn set_reading_colors(&mut self, colors: pdfcraft_render::ReadingColors) {
        self.reading_colors = colors;
        for i in 0..self.views.len() {
            // Idempotent per document: the engine says whether anything changed.
            if self.session.set_reading_colors(self.views[i].id, colors) {
                self.views[i].invalidate_content();
            }
        }
    }

    /// Today in `fmt` (or Preferences ▸ Date format), with month and weekday names in the date
    /// language, or the interface language when it follows that.
    pub fn date_text(&self, fmt: Option<&str>) -> Result<String, String> {
        date_text(&self.session, fmt)
    }

    /// The user's persistent state as the typed settings the store writes. Local only.
    pub fn settings_snapshot(&self) -> PdfCraftSettings {
        let trusted: Vec<String> = self.session.trusted_certificates().iter().map(pdfcraft_engine::sign::x509::to_pem).collect();
        let d = &self.print_draft;
        let size = d.paper_size();
        let layout = settings::Layout {
            reopen_last_session: Some(self.reopen_last_session),
            restore_last_view: Some(self.restore_last_view),
            last_views: json_text(&self.last_views_to_save()),
            system_title_bar: Some(self.title_bar_choice == TitleBarPref::System),
            reload_changed_files: Some(self.reload_changed_files),
            update_check_at_start: Some(self.update_check_at_start),
            show_secured_notice: Some(self.show_secured_notice),
            text_links: Some(self.text_links),
            pinned_folders: json_text(&self.pinned.folders),
            pinned_folders_collapsed: json_text(&self.pinned.collapsed),
            pinned_tools: Some(self.pinned_tools.clone()),
            theme: Some(self.theme_preference),
            scroll_speed: Some(self.scroll_speed),
            invert_middle_drag: Some(self.invert_middle_drag),
            animation_effects: Some(self.animation_effects),
            default_mode: Some(self.default_mode),
            default_layout: Some(canvas::display_token(self.view_defaults.display, self.view_defaults.continuous).to_string()),
            combine_columns: Some(self.combine_columns.clone()),
            combine_view: Some(self.combine_view.as_str().to_string()),
            combine_zoom: Some(self.combine_zoom),
            organize_zoom: Some(self.organize_zoom),
        };
        let print = settings::Print {
            print_custom_papers: json_text(&d.custom_papers),
            // Canonical names survive changes to the order of PAPERS and do not depend on UI
            // language; a custom paper is stored by the name it was given.
            print_paper: (d.paper < d.paper_count()).then(|| d.paper_name().to_string()),
            print_paper_size: Some([size.0, size.1]),
            print_paper_overridden: Some(d.paper_overridden),
            print_printers: json_text(&crate::print_ui::setups_to_json(&d.setups)),
            duplex: Some(print_names::duplex(d.duplex).to_string()),
            grayscale: Some(d.grayscale),
            collate: Some(d.collate),
            custom_scale: Some(d.custom_scale),
            size: Some(print_names::size_mode(d.size).to_string()),
            comments_forms: Some(print_names::content(d.content).to_string()),
            binding: Some(print_names::binding(d.binding).to_string()),
            as_image: Some(d.as_image),
            dpi: Some(d.dpi),
        };
        let behavior = settings::Behavior {
            default_page_gaps: Some(self.view_defaults.page_gaps),
            full_screen_fill: Some(self.full_screen_fill),
            full_screen_escape_exits: Some(self.full_screen_escape_exits),
            units: Some(self.units),
            always_default_view: Some(self.always_default_view),
            highlight_fields: Some(self.highlight_fields),
            comments_panel_closed: Some(self.comments_panel_closed),
            right_panel_closed: Some(self.right_panel_closed),
            ocr: json_text(&self.ocr_draft.to_pref()),
            default_zoom: Some(self.view_defaults.zoom_name().to_string()),
            language: Some(self.language.clone()),
            flatten_fill_sign: Some(self.flatten_fill_sign_on_save),
            ask_to_lock_signatures: Some(self.ask_to_lock_signatures),
            fill_text_size: Some(self.fill_text_size),
            fill_text_color: Some(self.fill_text_color),
            show_page_edge_shadows: Some(self.show_page_edge_shadows),
            fill_text_border: Some(self.fill_text_default.width),
            fill_text_border_shape: Some(self.fill_text_default.shape.name().to_string()),
            // Absent is "the text's colour": TOML has no null.
            fill_text_border_color: self.fill_text_default.color,
            redact_prefs: json_text(&self.redact_defaults.to_json()),
        };
        let view = settings::View {
            reading_colors: Some(self.reading_colors),
            ui_scale: self.ui_scale.map(f64::from),
            date_format: Some(self.session.date_format().to_string()),
            date_language: self.session.date_language().map(str::to_string),
            author: Some(self.comment_prefs.author.clone()),
            hide_empty_highlight_hover: Some(self.comment_prefs.hide_empty_highlight_hover),
            comment_tool_pinned: Some(self.comment_prefs.pinned),
        };
        let styles: Vec<serde_json::Value> = self
            .comment_prefs
            .styles()
            .map(|(t, s)| {
                serde_json::json!({
                    "tool": t.command(),
                    "color": s.color,
                    "opacity": s.opacity,
                    "fill_opacity": s.fill_opacity,
                    "text_opacity": s.text_opacity,
                    "width": s.width,
                    "fill": s.fill,
                    "border_color": s.border_color,
                    "border_shape": s.border_shape.name(),
                })
            })
            .collect();
        let font_sizes: Vec<serde_json::Value> =
            self.comment_prefs.font_sizes().map(|(t, size)| serde_json::json!({ "tool": t.command(), "font_size": size })).collect();
        let comment = settings::Comment { comment_styles: json_text(&styles), comment_font_sizes: json_text(&font_sizes) };
        let signature = settings::Signature {
            signatures: Some({
                let mut slots = craft::review::ui::signature::SignatureSettings::default();
                slots.signature = self.signature.clone();
                slots.initials = self.initials.clone();
                slots
            }),
            sign_graphic: Some(self.sign_graphic.clone()),
            // macOS Keychain and Windows store identities are read from their OS key stores each time.
            digital_ids: Some(
                self.digital_ids.iter().filter(|e| !e.path.starts_with("keychain:") && !e.path.starts_with("windows:")).cloned().collect(),
            ),
            trusted: Some(trusted),
            custom_stamps: json_text(&stamps_ui::encode(&self.custom_stamps)),
            actions: json_text(&actions_ui::encode(&self.custom_actions)),
            javascript: Some(self.session.javascript()),
        };
        // PLAN: assumed app fields `dock`, `rail_side`, `recent_view`, `recent_sort` (chrome-w13, chrome-w16)
        let dock = settings::Dock { dock: json_text(&*self.dock), rail_side: name_of(&self.rail_side) };
        let recent = settings::Recent {
            recent: json_text(&self.recent.iter().take(self.recent_limit).collect::<Vec<_>>()),
            recent_limit: u32::try_from(self.recent_limit).ok(),
            recent_view: name_of(&self.recent_view),
            recent_sort: name_of(&self.recent_sort),
        };
        let hotbar = settings::Hotbar { tools: Some(self.hotbar.tools.clone()) };
        // Kept only while the preference is on.
        let session = settings::Session { last_session: self.reopen_last_session.then(|| json_text(&self.session_to_save())).flatten() };
        PdfCraftSettings { layout, print, behavior, view, comment, signature, dock, recent, hotbar, session }
    }

    /// Hands the app the settings store it saves the preferences to from now on (the launch opens it).
    pub fn attach_settings(&mut self, store: settings::Store) {
        self.settings = Some(store);
    }

    /// Applies settings the store or an import produced. A value that is absent keeps what the app
    /// has; a value out of range is clamped or refused as the dialogs would.
    pub fn apply_settings(&mut self, s: &PdfCraftSettings) {
        self.apply_hotbar(&s.hotbar);
        self.apply_dock(&s.dock);
        self.apply_recent(&s.recent);
        self.apply_layout(&s.layout);
        self.apply_behavior(&s.behavior);
        self.apply_print(&s.print);
        self.apply_view(&s.view);
        self.apply_comment(&s.comment);
        self.apply_signature(&s.signature);
        self.apply_session(&s.session);
    }

    fn apply_hotbar(&mut self, s: &settings::Hotbar) {
        if let Some(tools) = &s.tools {
            self.hotbar.tools = tools.clone();
        }
    }

    fn apply_dock(&mut self, s: &settings::Dock) {
        if let Some(workspace) = parsed(&s.dock).and_then(|v| serde_json::from_value(v).ok()) {
            std::sync::Arc::make_mut(&mut self.dock).replace(workspace);
        }
        if let Some(side) = s.rail_side.as_deref().and_then(from_name) {
            self.rail_side = side;
        }
    }

    /// The recent files, how Home lists them and how many are kept.
    fn apply_recent(&mut self, s: &settings::Recent) {
        if let Some(entries) = parsed(&s.recent).as_ref().and_then(serde_json::Value::as_array) {
            let files: Vec<crate::RecentFile> = entries.iter().filter_map(|e| serde_json::from_value(e.clone()).ok()).collect();
            // Entries whose files are gone leave once the window is up: asking here could block
            // startup on a mount that has stopped answering (#5682).
            self.recent_check.restored(files.iter().map(|f| f.path.clone()).collect());
            self.recent = files;
        }
        // Nonsense keeps the limit it had; settings from before the limit have none.
        if let Some(limit) = s.recent_limit {
            self.recent_limit = usize::try_from(limit).map_or(crate::RECENT_FILES_MAX, |n| n.min(crate::RECENT_FILES_MAX));
        }
        self.recent.truncate(self.recent_limit);
        if let Some(view) = s.recent_view.as_deref().and_then(from_name) {
            self.recent_view = view;
        }
        if let Some(sort) = s.recent_sort.as_deref().and_then(from_name) {
            self.recent_sort = sort;
        }
    }

    /// The Last-session choice, window chrome, theme and the grids' zoom.
    fn apply_layout(&mut self, s: &settings::Layout) {
        if let Some(columns) = &s.combine_columns {
            self.combine_columns = columns.clone();
        }
        if let Some(view) = s.combine_view.as_deref().and_then(combine_ui::CombineView::parse) {
            self.combine_view = view;
        }
        if let Some(zoom) = s.combine_zoom {
            self.set_combine_zoom(zoom);
        }
        // Clamped like any grid zoom; nonsense keeps the usual size.
        if let Some(zoom) = s.organize_zoom.filter(|z| z.is_finite()) {
            let range = craft::pages::ui::ORGANIZE_ZOOM.range();
            self.organize_zoom = zoom.clamp(*range.start(), *range.end());
        }
        if let Some(on) = s.reopen_last_session {
            self.reopen_last_session = on;
        }
        if let Some(on) = s.restore_last_view {
            self.restore_last_view = on;
        }
        self.restore_last_views(&parsed(&s.last_views).unwrap_or(serde_json::Value::Null));
        if let Some(on) = s.system_title_bar {
            // The window follows in the next frame (`prepare_raw_input`).
            self.title_bar_choice = if on { TitleBarPref::System } else { TitleBarPref::Integrated };
        }
        if let Some(on) = s.reload_changed_files {
            self.reload_changed_files = on;
        }
        if let Some(on) = s.update_check_at_start {
            self.update_check_at_start = on;
        }
        if let Some(on) = s.show_secured_notice {
            self.show_secured_notice = on;
        }
        if let Some(on) = s.text_links {
            self.text_links = on;
        }
        self.pinned.restore(&parsed(&s.pinned_folders).unwrap_or(serde_json::Value::Null));
        self.pinned.restore_collapsed(&parsed(&s.pinned_folders_collapsed).unwrap_or(serde_json::Value::Null));
        if let Some(items) = &s.pinned_tools {
            let mut tools: Vec<String> = Vec::new();
            for id in items {
                if pdfcraft_engine::catalog::group(id).is_some() && !tools.iter().any(|t| t == id) {
                    tools.push(id.clone());
                }
            }
            self.pinned_tools = tools;
        }
        if let Some(preference) = s.theme {
            self.set_theme_preference(preference);
        }
        if let Some(speed) = s.scroll_speed {
            self.scroll_speed = speed;
        }
        if let Some(invert) = s.invert_middle_drag {
            self.invert_middle_drag = invert;
        }
        if let Some(effects) = s.animation_effects {
            self.animation_effects = effects;
        }
        if let Some(mode) = s.default_mode {
            self.default_mode = mode;
        }
        if let Some((layout, continuous)) = s.default_layout.as_deref().and_then(canvas::parse_display_token) {
            (self.view_defaults.display, self.view_defaults.continuous) = (layout, continuous);
        }
    }

    /// The windows to reopen. Absent clears them, as the choice is saved only while reopening is on.
    fn apply_session(&mut self, s: &settings::Session) {
        let value = parsed(&s.last_session).unwrap_or(serde_json::Value::Null);
        self.last_session = craft::platform::session::Session::from_json(&value);
    }

    /// The Print dialog's papers, each printer's setup and the choices it keeps (#2001).
    fn apply_print(&mut self, p: &settings::Print) {
        // The Print dialog's own paper sizes; settings are untrusted, so each custom paper is
        // checked as if it were typed.
        if let Some(papers) = parsed(&p.print_custom_papers) {
            self.print_draft.custom_papers = print_paper::read_custom(&papers);
        }
        // After the custom papers, so a custom paper chosen last time is found by its name.
        if let Some(name) = p.print_paper.as_deref() {
            let saved_size =
                p.print_paper_size.and_then(|[w, h]| if w >= 72.0 && h >= 72.0 && w.is_finite() && h.is_finite() { Some((w, h)) } else { None });
            let size_ok = |s: &(f64, f64)| match saved_size {
                Some(sz) => (s.0 - sz.0).abs() <= 1.0 && (s.1 - sz.1).abs() <= 1.0,
                None => true,
            };
            let matched_pos = self
                .print_draft
                .papers
                .iter()
                .position(|(n, s)| n == name && size_ok(s))
                .or_else(|| pdfcraft_engine::print::PAPERS.iter().position(|(canonical, s)| *canonical == name && size_ok(s)))
                .or_else(|| self.print_draft.paper_index(name));
            let overridden = p.print_paper_overridden.unwrap_or(false);
            if let Some(paper) = matched_pos {
                self.print_draft.paper = paper;
                self.print_draft.paper_overridden = overridden;
            } else if let Some(size) = saved_size {
                self.print_draft.papers.push((name.to_string(), size));
                self.print_draft.paper = self.print_draft.papers.len() - 1;
                self.print_draft.paper_overridden = overridden;
            }
        }
        if let Some(setups) = parsed(&p.print_printers).and_then(|v| crate::print_ui::setups_from_json(&v)) {
            self.print_draft.setups = setups;
        }
        // The Print dialog's remembered choices (#2001). Settings are untrusted: an unknown name
        // or a scale outside the dialog's range keeps the default. The scale is applied before the
        // size mode, which carries it.
        let draft = &mut self.print_draft;
        if let Some(duplex) = p.duplex.as_deref().and_then(print_names::parse_duplex) {
            draft.duplex = duplex;
        }
        if let Some(on) = p.grayscale {
            draft.grayscale = on;
        }
        if let Some(on) = p.collate {
            draft.collate = on;
        }
        if let Some(scale) = p.custom_scale.filter(|s| print_ui::CUSTOM_SCALE.contains(s)) {
            draft.custom_scale = scale;
        }
        if let Some(size) = p.size.as_deref().and_then(|name| print_names::parse_size_mode(name, draft.custom_scale)) {
            draft.size = size;
        }
        if let Some(content) = p.comments_forms.as_deref().and_then(print_names::parse_content) {
            draft.content = content;
        }
        // A booklet's binding side, as Acrobat keeps it (#2707).
        if let Some(binding) = p.binding.as_deref().and_then(print_names::parse_binding) {
            draft.binding = binding;
        }
        // Print as image and its resolution (#6392), so a printer that needs pictures keeps getting
        // them; a resolution the dialog doesn't offer keeps the default.
        if let Some(on) = p.as_image {
            draft.as_image = on;
        }
        if let Some(dpi) = p.dpi.filter(|d| print_ui::IMAGE_DPI.contains(d)) {
            draft.dpi = dpi;
        }
    }

    /// Switches for reopening, window chrome and file watching.
    fn apply_behavior(&mut self, b: &settings::Behavior) {
        if let Some(show) = b.default_page_gaps {
            self.view_defaults.page_gaps = show;
        }
        if let Some(fill) = b.full_screen_fill {
            self.full_screen_fill = fill;
        }
        if let Some(exits) = b.full_screen_escape_exits {
            self.full_screen_escape_exits = exits;
        }
        if let Some(units) = b.units {
            self.units = units;
        }
        if let Some(on) = b.always_default_view {
            self.always_default_view = on;
        }
        if let Some(on) = b.highlight_fields {
            self.highlight_fields = on;
        }
        if let Some(closed) = b.comments_panel_closed {
            self.comments_panel_closed = closed;
        }
        if let Some(closed) = b.right_panel_closed {
            self.right_panel_closed = closed;
        }
        // Scan & OCR choices (language, engine, strategy, output mode, preprocessing and skip
        // flags); unknown or malformed values keep the defaults.
        self.ocr_draft.apply_pref(&parsed(&b.ocr).unwrap_or(serde_json::Value::Null));
        if let Some(defaults) = b.default_zoom.as_deref().and_then(|zoom| self.view_defaults.with_zoom(zoom)) {
            self.view_defaults = defaults;
        }
        if let Some(language) = b.language.as_deref().and_then(i18n::normalize_pref) {
            self.language = language.to_string();
        }
        if let Some(on) = b.flatten_fill_sign {
            self.flatten_fill_sign_on_save = on;
        }
        if let Some(on) = b.ask_to_lock_signatures {
            self.ask_to_lock_signatures = on;
        }
        if let Some(size) = b.fill_text_size.and_then(fill_sign::valid_text_size) {
            self.fill_text_size = size;
        }
        if let Some(color) = b.fill_text_color.and_then(fill_sign::valid_text_color) {
            self.fill_text_color = color;
        }
        if let Some(show) = b.show_page_edge_shadows {
            self.show_page_edge_shadows = show;
        }
        if let Some(width) = b.fill_text_border.and_then(fill_sign::valid_text_border) {
            self.fill_text_default.width = width;
        }
        if let Some(shape) = b.fill_text_border_shape.as_deref().and_then(pdfcraft_engine::BorderShape::parse) {
            self.fill_text_default.shape = shape;
        }
        // Absent is a saved "the text's colour".
        self.fill_text_default.color = b.fill_text_border_color.map(|c| c.map(|x| x.clamp(0.0, 1.0)));
        // Fill & Sign text starts from the saved border.
        self.fill_text_border = self.fill_text_default.width;
        self.fill_text_border_color = self.fill_text_default.color;
        self.fill_text_border_shape = self.fill_text_default.shape;
        if let Some(prefs) = parsed(&b.redact_prefs).and_then(|v| RedactPrefs::from_json(&v)) {
            self.redact_prefs = prefs.clone();
            self.redact_defaults = prefs;
        }
    }

    /// Display defaults, units, language and the author.
    fn apply_view(&mut self, v: &settings::View) {
        if let Some(colors) = v.reading_colors {
            self.reading_colors = colors;
        }
        // A size out of range follows the system, and so does none: TOML has no null to save it as.
        self.ui_scale = v.ui_scale.and_then(ui_scale::valid);
        // Settings are untrusted: an unusable pattern keeps the default.
        if let Some(f) = v.date_format.as_deref() {
            let _ = self.session.set_date_format(f);
        }
        if let Some(l) = v.date_language.as_deref() {
            let _ = self.session.set_date_language(Some(l));
        }
        // An empty or missing name keeps the login-name default; settings are untrusted, so the
        // name is cut to a sane length.
        if let Some(author) = v.author.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
            self.comment_prefs.author = author.chars().take(MAX_AUTHOR_CHARS).collect();
        }
        if let Some(hide) = v.hide_empty_highlight_hover {
            self.comment_prefs.hide_empty_highlight_hover = hide;
        }
        if let Some(pinned) = v.comment_tool_pinned {
            self.comment_prefs.pinned = pinned;
        }
    }

    /// The per-tool comment styles and font sizes.
    fn apply_comment(&mut self, c: &settings::Comment) {
        // Per-tool comment styles (#340): each field goes through its clamping setter, so a
        // malformed or hostile entry is clamped or ignored rather than trusted.
        if let Some(styles) = parsed(&c.comment_styles).as_ref().and_then(serde_json::Value::as_array) {
            for e in styles {
                let Some(tool) = e["tool"].as_str().and_then(crate::comments::CommentTool::from_command) else { continue };
                if let Some(c) = settings::rgb_triple(&e["color"]) {
                    self.comment_prefs.set_color(tool, c);
                }
                if let Some(o) = e["opacity"].as_f64() {
                    self.comment_prefs.set_opacity(tool, o);
                }
                // The fill's and text's own opacities, where saved (older settings have none).
                self.comment_prefs.set_part_opacity(tool, e["fill_opacity"].as_f64(), e["text_opacity"].as_f64());
                if let Some(w) = e["width"].as_f64() {
                    self.comment_prefs.set_width(tool, w);
                }
                // `"fill": null` is a saved "no fill", which clears a tool's default fill; a
                // missing or malformed fill keeps it.
                let fill = match e.get("fill") {
                    Some(serde_json::Value::Null) => Some(None),
                    Some(v) => settings::rgb_triple(v).map(Some),
                    None => None,
                };
                if let Some(fill) = fill {
                    let mut s = self.comment_prefs.style(tool);
                    s.fill = fill;
                    self.comment_prefs.set_style(tool, s);
                }
                // `"border_color": null` is a saved "Same as text", which clears a tool's own border
                // colour; a missing or malformed one keeps it.
                match e.get("border_color") {
                    Some(serde_json::Value::Null) => self.comment_prefs.set_border_color(tool, None),
                    Some(c) => {
                        if let Some(c) = settings::rgb_triple(c) {
                            self.comment_prefs.set_border_color(tool, Some(c));
                        }
                    }
                    None => {}
                }
                if let Some(shape) = e["border_shape"].as_str().and_then(pdfcraft_engine::BorderShape::parse) {
                    self.comment_prefs.set_border_shape(tool, shape);
                }
            }
        }
        // Default font sizes (#5767), clamped by the setter; a missing or malformed size keeps
        // the tool's own.
        if let Some(sizes) = parsed(&c.comment_font_sizes).as_ref().and_then(serde_json::Value::as_array) {
            for e in sizes {
                let Some(tool) = e["tool"].as_str().and_then(crate::comments::CommentTool::from_command) else { continue };
                if let Some(size) = e["font_size"].as_f64() {
                    self.comment_prefs.set_font_size(tool, size);
                }
            }
        }
    }

    /// Saved signatures, stamps, actions and trusted certificates.
    fn apply_signature(&mut self, s: &settings::Signature) {
        if let Some(slots) = &s.signatures {
            self.signature = slots.signature.clone();
            self.initials = slots.initials.clone();
        }
        // An unreadable choice (a damaged image) falls back to the name.
        self.sign_graphic = s.sign_graphic.clone().unwrap_or_default();
        if let Some(ids) = &s.digital_ids {
            self.digital_ids = ids.clone();
        }
        if let Some(stamps) = parsed(&s.custom_stamps) {
            self.custom_stamps = stamps_ui::decode(&stamps);
        }
        if let Some(actions) = parsed(&s.actions) {
            self.custom_actions = actions_ui::decode(&actions);
        }
        if let Some(on) = s.javascript {
            self.session.set_javascript(on);
        }
        if let Some(pems) = &s.trusted {
            let certs = pems.iter().filter_map(|p| pdfcraft_engine::sign::x509::load_certificates(p.as_bytes()).ok()).flatten().collect();
            self.apply_trusted_certificates(certs);
        }
    }
}
