use super::*;

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

/// A restored colour: an `[r, g, b]` array of finite numbers, each clamped to 0–1 (restored
/// settings are untrusted). Anything else (a string, a wrong length, a null or a non-finite
/// number) is refused whole, so the caller keeps its default.
fn rgb_triple(v: &serde_json::Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    let mut out = [0.0f64; 3];
    for (slot, x) in out.iter_mut().zip(a.iter()) {
        let n = x.as_f64()?;
        if !n.is_finite() {
            return None;
        }
        *slot = n.clamp(0.0, 1.0);
    }
    Some(out)
}

impl PdfCraftApp {
    pub fn set_theme_preference(&mut self, preference: ThemePreference) {
        self.theme_preference = preference;
        if let Some(ctx) = self.ctx.clone() {
            self.sync_theme(&ctx);
            theme::apply(&ctx, self.theme);
        } else {
            self.theme = preference.resolve(None, ThemeKind::Light);
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

    /// Enable the Linux desktop portal in a native window. Headless contexts keep
    /// their supplied input and never connect to the user's session bus.
    #[cfg(target_os = "linux")]
    pub fn enable_desktop_theme(&mut self) {
        self.desktop_theme_enabled = true;
    }

    /// Today in `fmt` (or Preferences ▸ Date format), with month and weekday names in the date
    /// language, or the interface language when it follows that.
    pub fn date_text(&self, fmt: Option<&str>) -> Result<String, String> {
        date_text(&self.session, fmt)
    }

    /// Serialize the user's persistent state (recent files, theme). Local only.
    pub fn persist(&self) -> String {
        let trusted: Vec<String> = self.session.trusted_certificates().iter().map(pdfcraft_engine::sign::x509::to_pem).collect();
        let mut v = serde_json::json!({
            "side_panels": self.side_panels.to_json(self.left_open),
            "docking_right": self.right,
            "recent": self.recent,
            "reopen_last_session": self.reopen_last_session,
            "restore_last_view": self.restore_last_view,
            "last_views": self.last_views_to_save(),
            // Kept only while the preference is on.
            "last_session": self.reopen_last_session.then(|| self.session_to_save()),
            "reload_changed_files": self.reload_changed_files,
            "system_title_bar": self.system_title_bar,
            "pinned_folders": self.pinned.folders,
            "theme": self.theme_preference,
            "scroll_speed": self.scroll_speed,
            "animation_effects": self.animation_effects,
            "default_mode": self.default_mode,
            "default_layout": canvas::display_token(self.view_defaults.layout, self.view_defaults.continuous),
            "default_zoom": self.view_defaults.zoom_name(),
            "always_default_view": self.always_default_view,
            "highlight_fields": self.view_defaults.highlight_fields,
            "units": self.units,
            "language": self.language,
            "reading_colors": self.reading_colors,
            // Null follows the system's text scaling.
            "ui_scale": self.ui_scale,
            "flatten_fill_sign": self.flatten_fill_sign_on_save,
            "fill_text_size": self.fill_text_size,
            "date_format": self.session.date_format(),
            // Null follows the interface language.
            "date_language": self.session.date_language(),
            "author": self.comment_prefs.author,
            // Per-tool comment defaults (colours, opacity, widths), so Make Current
            // Properties Default survives a restart (#340).
            "comment_styles": self
                .comment_prefs
                .styles()
                .map(|(t, s)| serde_json::json!({
                    "tool": t.command(),
                    "color": s.color,
                    "opacity": s.opacity,
                    "width": s.width,
                    "fill": s.fill,
                }))
                .collect::<Vec<_>>(),
            // Drawn signatures keep their original form (older settings read the same).
            "signature": match &self.signature { Some(fill_sign::SavedSig::Drawn(s)) => Some(s), _ => None },
            "signature_text": match &self.signature { Some(fill_sign::SavedSig::Typed(t)) => Some(t), _ => None },
            "signature_image": match &self.signature { Some(s @ fill_sign::SavedSig::Image(_)) => Some(s), _ => None },
            "initials": self.initials,
            "sign_graphic": self.sign_graphic,
            // macOS Keychain and Windows store identities are read from their OS key stores each time.
            "digital_ids": self.digital_ids.iter().filter(|e| !e.path.starts_with("keychain:") && !e.path.starts_with("windows:")).collect::<Vec<_>>(),
            "trusted": trusted,
            "custom_stamps": stamps_ui::encode(&self.custom_stamps),
            "javascript": self.session.javascript(),
            "actions": actions_ui::encode(&self.custom_actions),
            "combine_columns": self.combine_columns.to_json(),
            "combine_view": self.combine_view.as_str(),
            "combine_zoom": self.combine_zoom,
            "comments_panel_closed": self.comments_panel_closed,
            "right_panel_closed": self.right_panel_closed,
        });
        // Set apart: one more key inside the macro above passes serde_json's recursion limit.
        v["default_page_gaps"] = serde_json::Value::Bool(self.view_defaults.show_page_gaps);
        v["redact_prefs"] = self.redact_defaults.to_json();
        // Set outside json! above, which is at serde_json's macro recursion limit.
        v["hide_empty_highlight_hover"] = serde_json::Value::Bool(self.comment_prefs.hide_empty_highlight_hover);
        v["comment_tool_pinned"] = serde_json::Value::Bool(self.comment_prefs.pinned);
        v["show_secured_notice"] = serde_json::Value::Bool(self.show_secured_notice);
        v["organize_zoom"] = serde_json::json!(self.organize_zoom);
        // Canonical names survive changes to the order of PAPERS and do not depend on UI language.
        v["print_paper"] = serde_json::json!(pdfcraft_engine::print::PAPERS.get(self.print_draft.paper).map(|(name, _)| *name));
        v.to_string()
    }

    /// Restore state written by `persist`. Unknown or malformed data is ignored.
    pub fn restore(&mut self, json: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return };
        let panels = side_panels::restore(&v);
        self.side_panels = panels.panels;
        if let Some(open) = panels.tools_open {
            self.left_open = open;
        }
        if let Some(right) = v.get("docking_right") {
            self.right = serde_json::from_value(right.clone()).ok().flatten().filter(|_| !panels.side_closed);
        }
        self.combine_columns = combine_ui::Columns::from_json(&v["combine_columns"]);
        if let Some(view) = v["combine_view"].as_str().and_then(combine_ui::CombineView::parse) {
            self.combine_view = view;
        }
        if let Some(zoom) = v["combine_zoom"].as_f64() {
            self.set_combine_zoom(zoom as f32);
        }
        // Clamped like any grid zoom when the grid next shows; nonsense keeps the usual size.
        if let Some(zoom) = v["organize_zoom"].as_f64().map(|z| z as f32).filter(|z| z.is_finite()) {
            self.organize_zoom = zoom.clamp(*canvas::GRID_ZOOM_RANGE.start(), *canvas::GRID_ZOOM_RANGE.end());
        }
        if let Ok(r) = serde_json::from_value::<Vec<RecentFile>>(v["recent"].clone()) {
            // Only keep entries whose files still exist.
            #[cfg(not(target_arch = "wasm32"))]
            let r: Vec<RecentFile> = r.into_iter().filter(|f| std::path::Path::new(&f.path).exists()).collect();
            self.recent = r;
        }
        self.restore_layout_prefs(&v);
        self.restore_behavior_prefs(&v);
        self.restore_view_prefs(&v);
        self.restore_comment_prefs(&v);
        self.restore_signature_prefs(&v);
    }

    /// The panels, combine view, zoom and recent files.
    fn restore_layout_prefs(&mut self, v: &serde_json::Value) {
        if let Some(on) = v["reopen_last_session"].as_bool() {
            self.reopen_last_session = on;
        }
        if let Some(on) = v["restore_last_view"].as_bool() {
            self.restore_last_view = on;
        }
        self.restore_last_views(&v["last_views"]);
        self.last_session = last_session::LastSession::from_json(&v["last_session"]);
        if let Some(on) = v["system_title_bar"].as_bool() {
            self.system_title_bar = on;
        }
        if let Some(on) = v["reload_changed_files"].as_bool() {
            self.reload_changed_files = on;
        }
        if let Some(on) = v["show_secured_notice"].as_bool() {
            self.show_secured_notice = on;
        }
        self.pinned.restore(&v["pinned_folders"]);
        if let Ok(preference) = serde_json::from_value::<ThemePreference>(v["theme"].clone()) {
            self.set_theme_preference(preference);
        }
        if let Ok(speed) = serde_json::from_value::<scroll_speed::ScrollSpeed>(v["scroll_speed"].clone()) {
            self.scroll_speed = speed;
        }
        if let Ok(effects) = serde_json::from_value::<motion::AnimationEffects>(v["animation_effects"].clone()) {
            self.animation_effects = effects;
        }
        if let Ok(mode) = serde_json::from_value::<Mode>(v["default_mode"].clone()) {
            self.default_mode = mode;
        }
        if let Some((layout, continuous)) = v["default_layout"].as_str().and_then(canvas::parse_display_token) {
            (self.view_defaults.layout, self.view_defaults.continuous) = (layout, continuous);
        }
    }

    /// Switches for reopening, window chrome and file watching.
    fn restore_behavior_prefs(&mut self, v: &serde_json::Value) {
        if let Some(show) = v["default_page_gaps"].as_bool() {
            self.view_defaults.show_page_gaps = show;
        }
        if let Ok(units) = serde_json::from_value::<units::Unit>(v["units"].clone()) {
            self.units = units;
        }
        if let Some(on) = v["always_default_view"].as_bool() {
            self.always_default_view = on;
        }
        if let Some(paper) = v
            .get("print_paper")
            .and_then(serde_json::Value::as_str)
            .and_then(|name| pdfcraft_engine::print::PAPERS.iter().position(|(canonical, _)| *canonical == name))
        {
            self.print_draft.paper = paper;
        }
        if let Some(on) = v["highlight_fields"].as_bool() {
            self.view_defaults.highlight_fields = on;
        }
        if let Some(closed) = v["comments_panel_closed"].as_bool() {
            self.comments_panel_closed = closed;
        }
        if let Some(closed) = v["right_panel_closed"].as_bool() {
            self.right_panel_closed = closed;
        }
        if let Some(defaults) = v["default_zoom"].as_str().and_then(|zoom| self.view_defaults.with_zoom(zoom)) {
            self.view_defaults = defaults;
        }
        if let Some(language) = v["language"].as_str().and_then(i18n::normalize_pref) {
            self.language = language.to_string();
        }
        if let Some(on) = v["flatten_fill_sign"].as_bool() {
            self.flatten_fill_sign_on_save = on;
        }
        if let Some(size) = v["fill_text_size"].as_f64().and_then(fill_sign::valid_text_size) {
            self.fill_text_size = size;
        }
        if let Some(prefs) = RedactPrefs::from_json(&v["redact_prefs"]) {
            self.redact_prefs = prefs.clone();
            self.redact_defaults = prefs;
        }
    }

    /// Display defaults, units, language and sign-in details.
    fn restore_view_prefs(&mut self, v: &serde_json::Value) {
        // Settings are untrusted: an unreadable value (a colour that isn't three bytes, say)
        // fails to deserialize and keeps Off.
        if let Ok(colors) = serde_json::from_value::<pdfcraft_render::ReadingColors>(v["reading_colors"].clone()) {
            self.reading_colors = colors;
        }
        // Settings are untrusted: a size out of range follows the system instead.
        self.ui_scale = v["ui_scale"].as_f64().and_then(ui_scale::valid);
        // Settings are untrusted: an unusable pattern keeps the default.
        if let Some(f) = v["date_format"].as_str() {
            let _ = self.session.set_date_format(f);
        }
        if let Some(l) = v["date_language"].as_str() {
            let _ = self.session.set_date_language(Some(l));
        }
        // An empty or missing name keeps the login-name default; settings are untrusted, so the
        // name is cut to a sane length.
        if let Some(author) = v["author"].as_str().map(str::trim).filter(|a| !a.is_empty()) {
            self.comment_prefs.author = author.chars().take(MAX_AUTHOR_CHARS).collect();
        }
        if let Some(hide) = v["hide_empty_highlight_hover"].as_bool() {
            self.comment_prefs.hide_empty_highlight_hover = hide;
        }
        if let Some(pinned) = v["comment_tool_pinned"].as_bool() {
            self.comment_prefs.pinned = pinned;
        }
    }

    /// The author and per-tool comment styles.
    fn restore_comment_prefs(&mut self, v: &serde_json::Value) {
        // Per-tool comment styles (#340): each field goes through its clamping setter, so a
        // malformed or hostile entry is clamped or ignored rather than trusted.
        if let Some(styles) = v["comment_styles"].as_array() {
            for e in styles {
                let Some(tool) = e["tool"].as_str().and_then(crate::comments::CommentTool::from_command) else { continue };
                if let Some(c) = rgb_triple(&e["color"]) {
                    self.comment_prefs.set_color(tool, c);
                }
                if let Some(o) = e["opacity"].as_f64() {
                    self.comment_prefs.set_opacity(tool, o);
                }
                if let Some(w) = e["width"].as_f64() {
                    self.comment_prefs.set_width(tool, w);
                }
                // `"fill": null` is a saved "no fill", which clears a tool's default fill; a
                // missing or malformed fill keeps it.
                let fill = match e.get("fill") {
                    Some(serde_json::Value::Null) => Some(None),
                    Some(v) => rgb_triple(v).map(Some),
                    None => None,
                };
                if let Some(fill) = fill {
                    let mut s = self.comment_prefs.style(tool);
                    s.fill = fill;
                    self.comment_prefs.set_style(tool, s);
                }
            }
        }
    }

    /// Saved signatures, stamps, actions and trusted certificates.
    fn restore_signature_prefs(&mut self, v: &serde_json::Value) {
        if let Ok(s) = serde_json::from_value::<Vec<Vec<[f32; 2]>>>(v["signature"].clone())
            && s.iter().all(|st| st.iter().all(|p| p.iter().all(|x| x.is_finite())))
        {
            self.signature = Some(fill_sign::SavedSig::Drawn(s));
        }
        if let Some(t) = v["signature_text"].as_str().filter(|t| !t.trim().is_empty()) {
            self.signature = Some(fill_sign::SavedSig::Typed(t.to_string()));
        }
        if let Ok(s @ fill_sign::SavedSig::Image(_)) = serde_json::from_value::<fill_sign::SavedSig>(v["signature_image"].clone()) {
            self.signature = Some(s);
        }
        if let Ok(i) = serde_json::from_value::<fill_sign::SavedSig>(v["initials"].clone()) {
            self.initials = Some(i);
        }
        // An unreadable choice (a damaged image) falls back to the name.
        self.sign_graphic = serde_json::from_value(v["sign_graphic"].clone()).unwrap_or_default();
        if let Ok(ids) = serde_json::from_value::<Vec<DigitalIdEntry>>(v["digital_ids"].clone()) {
            self.digital_ids = ids;
        }
        self.custom_stamps = stamps_ui::decode(&v["custom_stamps"]);
        self.custom_actions = actions_ui::decode(&v["actions"]);
        if let Some(on) = v["javascript"].as_bool() {
            self.session.set_javascript(on);
        }
        if let Ok(pems) = serde_json::from_value::<Vec<String>>(v["trusted"].clone()) {
            let certs = pems.iter().filter_map(|p| pdfcraft_engine::sign::x509::load_certificates(p.as_bytes()).ok()).flatten().collect();
            self.apply_trusted_certificates(certs);
        }
    }
}
