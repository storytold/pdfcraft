//! In-app text selection translation pop-up.

pub mod service;

use egui::{Align, Color32, CornerRadius, Id, Layout, Margin, Order, Pos2, Rect, RichText, ScrollArea, Ui, pos2};
use std::sync::mpsc::Receiver;

/// Supported translation languages (code, native name).
pub const POPULAR_LANGUAGES: &[(&str, &str)] = &[
    ("tr", "Türkçe"),
    ("en", "English"),
    ("de", "Deutsch"),
    ("fr", "Français"),
    ("es", "Español"),
    ("it", "Italiano"),
    ("pt", "Português"),
    ("ru", "Русский"),
    ("ar", "العربية"),
    ("zh-CN", "中文 (简体)"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("uk", "Українська"),
    ("nl", "Nederlands"),
    ("pl", "Polski"),
    ("el", "Ελληνικά"),
    ("hi", "हिन्दी"),
];

/// Current state of the translation operation.
pub enum Status {
    Loading,
    Success(String),
    Error(String),
}

/// Floating translation popup attached to a text selection.
pub struct TranslationPopup {
    pub original_text: String,
    pub anchor: Pos2,
    pub source_lang: String,
    pub target_lang: String,
    pub detected_lang: Option<String>,
    pub status: Status,
    pub copied: bool,
    pub copied_at: f64,
    rx: Option<Receiver<service::TranslationResult>>,
}

impl TranslationPopup {
    /// Create and automatically start translation for `text` anchored at `anchor`.
    pub fn new(text: String, anchor: Pos2) -> Self {
        let target_lang = default_target_lang();
        let mut popup = Self {
            original_text: text,
            anchor,
            source_lang: "auto".to_string(),
            target_lang,
            detected_lang: None,
            status: Status::Loading,
            copied: false,
            copied_at: 0.0,
            rx: None,
        };
        popup.start_translation();
        popup
    }

    /// Trigger an asynchronous translation request on a background thread.
    pub fn start_translation(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let query = self.original_text.clone();
        let sl = self.source_lang.clone();
        let tl = self.target_lang.clone();

        self.status = Status::Loading;
        self.copied = false;
        self.rx = Some(rx);

        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            let res = service::fetch_translation(&query, &sl, &tl);
            let _ = tx.send(res);
        });

        #[cfg(target_arch = "wasm32")]
        {
            let res = service::fetch_translation(&query, &sl, &tl);
            let _ = tx.send(res);
        }
    }

    /// Render the floating pop-up. Returns `false` when the user closes or dismisses the popup.
    pub fn show(&mut self, ui: &mut Ui, viewport: Rect) -> bool {
        // Poll for asynchronous result from background worker
        if let Some(rx) = &self.rx
            && let Ok(res) = rx.try_recv()
        {
            match res {
                Ok((translated, detected)) => {
                    self.status = Status::Success(translated);
                    self.detected_lang = Some(detected);
                }
                Err(err) => {
                    self.status = Status::Error(err);
                }
            }
            self.rx = None;
        }

        // Close on Escape
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            return false;
        }

        // Reset copy feedback after 2 seconds
        let now = ui.input(|i| i.time);
        if self.copied && now - self.copied_at > 2.0 {
            self.copied = false;
        }

        let popup_width = 380.0_f32;
        let x = self.anchor.x.clamp(viewport.left() + 10.0, (viewport.right() - popup_width - 10.0).max(viewport.left() + 10.0));
        let y = if self.anchor.y + 260.0 < viewport.bottom() { self.anchor.y + 8.0 } else { (self.anchor.y - 280.0).max(viewport.top() + 8.0) };

        let mut keep_open = true;

        let area_res = egui::Area::new(Id::new("doc-translation-popup")).order(Order::Foreground).fixed_pos(pos2(x, y)).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).inner_margin(Margin::same(12)).corner_radius(CornerRadius::same(10)).show(ui, |ui| {
                ui.set_width(popup_width);

                // --- Header: Title + Language Selectors + Close Button ---
                ui.horizontal(|ui| {
                    ui.label(RichText::new("🌐").size(14.0));
                    ui.strong(tl!("Translate"));

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("✕").clicked() {
                            keep_open = false;
                        }

                        // Target Language Selector
                        let target_display = lang_name(&self.target_lang);
                        egui::ComboBox::from_id_salt("target_lang_combo").selected_text(target_display).width(90.0).show_ui(ui, |ui| {
                            for &(code, name) in POPULAR_LANGUAGES {
                                if ui.selectable_label(self.target_lang == code, name).clicked() && self.target_lang != code {
                                    self.target_lang = code.to_string();
                                    self.start_translation();
                                }
                            }
                        });

                        ui.label("➔");

                        // Source Language Selector
                        let source_label = if self.source_lang == "auto" {
                            if let Some(detected) = &self.detected_lang {
                                format!("{} ({})", tl!("Auto"), lang_name(detected))
                            } else {
                                tl!("Auto").to_string()
                            }
                        } else {
                            lang_name(&self.source_lang).to_string()
                        };

                        egui::ComboBox::from_id_salt("source_lang_combo").selected_text(source_label).width(105.0).show_ui(ui, |ui| {
                            if ui.selectable_label(self.source_lang == "auto", tl!("Auto Detect")).clicked() && self.source_lang != "auto" {
                                self.source_lang = "auto".to_string();
                                self.start_translation();
                            }
                            ui.separator();
                            for &(code, name) in POPULAR_LANGUAGES {
                                if ui.selectable_label(self.source_lang == code, name).clicked() && self.source_lang != code {
                                    self.source_lang = code.to_string();
                                    self.start_translation();
                                }
                            }
                        });
                    });
                });

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // --- Original Text Section ---
                ScrollArea::vertical().id_salt("orig_scroll").max_height(60.0).show(ui, |ui| {
                    ui.label(RichText::new(&self.original_text).size(12.0).weak());
                });

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // --- Translation Result Section ---
                match &self.status {
                    Status::Loading => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.add_space(4.0);
                            ui.label(RichText::new(tl!("Translating...")).size(12.5).weak());
                        });
                    }
                    Status::Success(result) => {
                        ScrollArea::vertical().id_salt("result_scroll").max_height(140.0).show(ui, |ui| {
                            ui.label(RichText::new(result).size(13.0));
                        });

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let copy_label = if self.copied {
                                RichText::new(format!("✓ {}", tl!("Copied!"))).color(Color32::from_rgb(46, 184, 92))
                            } else {
                                RichText::new(format!("📋 {}", tl!("Copy")))
                            };

                            if ui.button(copy_label).clicked() {
                                ui.ctx().copy_text(result.clone());
                                self.copied = true;
                                self.copied_at = ui.input(|i| i.time);
                                ui.ctx().request_repaint_after(std::time::Duration::from_secs(2));
                            }

                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(RichText::new("Google Translate").weak().size(11.0));
                            });
                        });
                    }
                    Status::Error(err) => {
                        ui.label(RichText::new(err).color(ui.visuals().error_fg_color).size(12.0));
                        ui.add_space(4.0);
                        if ui.button(tl!("Retry")).clicked() {
                            self.start_translation();
                        }
                    }
                }
            });
        });

        // Dismiss on clicks outside the popup
        if area_res.response.clicked_elsewhere() {
            keep_open = false;
        }

        if ui.input(|i| i.pointer.any_pressed())
            && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
        {
            let rect = area_res.response.rect;
            if !rect.contains(pos) {
                let on_subpopup =
                    ui.ctx().layer_id_at(pos).is_some_and(|lid| lid.order == egui::Order::Foreground || lid.order == egui::Order::Tooltip);
                if !on_subpopup {
                    keep_open = false;
                }
            }
        }

        keep_open
    }
}

/// Helper to get native display name of a language code.
fn lang_name(code: &str) -> &'static str {
    let base = code.split(['-', '_']).next().unwrap_or(code);
    if base.eq_ignore_ascii_case("zh") {
        return "中文 (简体)";
    }
    for &(c, name) in POPULAR_LANGUAGES {
        if c.eq_ignore_ascii_case(code) || c.eq_ignore_ascii_case(base) {
            return name;
        }
    }
    "Other"
}

/// Detect default target language based on system locale or app language.
fn default_target_lang() -> String {
    // 1. Check system locale
    #[cfg(target_os = "windows")]
    let sys_opt = sys_locale::get_locale();
    #[cfg(not(target_os = "windows"))]
    let sys_opt: Option<String> = None;

    if let Some(locale) = sys_opt {
        let base = locale.split(['-', '_']).next().unwrap_or("").to_lowercase();
        if base == "zh" {
            return "zh-CN".to_string();
        }
        if !base.is_empty() {
            for &(code, _) in POPULAR_LANGUAGES {
                if code.eq_ignore_ascii_case(&base) || code.eq_ignore_ascii_case(&locale) {
                    return code.to_string();
                }
            }
        }
    }

    // 2. Check current app language
    let app_lang = crate::i18n::current().code();
    let base = app_lang.split(['-', '_']).next().unwrap_or("").to_lowercase();
    if base == "zh" {
        return "zh-CN".to_string();
    }
    if !base.is_empty() {
        for &(code, _) in POPULAR_LANGUAGES {
            if code.eq_ignore_ascii_case(&base) || code.eq_ignore_ascii_case(app_lang) {
                return code.to_string();
            }
        }
    }

    // Default fallback
    "tr".to_string()
}
