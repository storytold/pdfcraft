//! Preferences dialog: card-based settings interface organized into clean, modular cards
//! with segmented pill controls and real-time search filtering.

use egui::{Color32, CornerRadius, Margin, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::PdfCraftApp;
use crate::canvas::{Fit, PageLayout};
use crate::motion::AnimationEffects;
use crate::scroll_speed::ScrollSpeed;
use crate::side_panels::ToolsSide;
use crate::theme::{self, ThemeKind, Tokens};
use crate::units::Unit;
use crate::widgets::{self, icons};

/// The height the Preferences dialog keeps below its scrolling settings for the buttons row.
pub(crate) const PREFERENCES_BUTTON_ROW: f32 = 52.0;

/// The least height the scrolling settings are given in very small windows.
pub(crate) const PREFERENCES_MIN_SCROLL: f32 = 60.0;

/// A segmented pill control for mutually exclusive choices.
///
/// Draws a rounded container with options inside. The active option is highlighted
/// with the accent colour. Emits AccessKit RadioButton widget info with the option's label.
pub fn segmented_pill_choice<T: PartialEq + Copy>(ui: &mut egui::Ui, t: &Tokens, current: &mut T, options: &[(T, &str)]) -> bool {
    let with_a11y: Vec<(T, &str, &str)> = options.iter().map(|&(v, l)| (v, l, l)).collect();
    segmented_pill_choice_a11y(ui, t, current, &with_a11y)
}

/// A segmented pill selection bar with separate accessibility labels.
pub fn segmented_pill_choice_a11y<T: PartialEq + Copy>(ui: &mut egui::Ui, t: &Tokens, current: &mut T, options: &[(T, &str, &str)]) -> bool {
    segmented_pill_choice_a11y_query(ui, t, current, options, "").0
}

/// A segmented pill selection bar with separate accessibility labels and optional query highlighting.
/// Returns whether the selection changed and the Response of the first matching option, if any.
pub fn segmented_pill_choice_a11y_query<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    t: &Tokens,
    current: &mut T,
    options: &[(T, &str, &str)],
    query: &str,
) -> (bool, Option<egui::Response>) {
    let mut changed = false;
    let mut first_match_resp: Option<egui::Response> = None;
    egui::Frame::new()
        .fill(t.field)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::symmetric(2, 2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for &(value, label, a11y_label) in options {
                    let is_active = *current == value;
                    let font = theme::medium(12.0);
                    let text_color = if is_active { Color32::WHITE } else { t.text };
                    let label_galley = layout_pill_label(ui, label, query, font, text_color);
                    let w = label_galley.size().x;
                    let (rect, resp) = ui.allocate_exact_size(vec2(w + 18.0, 24.0), Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, is_active, a11y_label));

                    if first_match_resp.is_none() && !find_match_ranges(label, query).is_empty() {
                        first_match_resp = Some(resp.clone());
                    }

                    if is_active {
                        ui.painter().rect(rect, CornerRadius::same(12), t.accent, Stroke::NONE, egui::StrokeKind::Inside);
                    } else {
                        let press = theme::Press::track(ui, &resp);
                        let fill = press.fill(Color32::TRANSPARENT, t.hover, t.pressed);
                        ui.painter().rect(rect, CornerRadius::same(12), fill, Stroke::NONE, egui::StrokeKind::Inside);
                    }
                    ui.painter().galley(rect.center() - label_galley.size() / 2.0, label_galley, text_color);

                    if resp.has_focus() {
                        ui.painter().rect_stroke(
                            rect.expand(1.0),
                            CornerRadius::same(13),
                            Stroke::new(2.0, t.accent_text),
                            egui::StrokeKind::Outside,
                        );
                    }

                    let resp = theme::hand(resp);
                    let key_select = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Space) || i.key_pressed(egui::Key::Enter));
                    if (resp.clicked() || key_select) && *current != value {
                        *current = value;
                        changed = true;
                    }
                }
            });
        });
    (changed, first_match_resp)
}

/// An independent toggle pill button with optional query highlighting.
/// Returns whether the active state changed and the control's Response.
pub fn toggle_pill_query(ui: &mut egui::Ui, t: &Tokens, active: &mut bool, label: &str, query: &str) -> (bool, egui::Response) {
    let mut changed = false;
    let is_active = *active;
    let font = theme::medium(12.0);
    let text_color = if is_active { Color32::WHITE } else { t.text };
    let label_galley = layout_pill_label(ui, label, query, font, text_color);
    let w = label_galley.size().x;
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 20.0, 26.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, is_active, label));

    let (fill, stroke) = if is_active {
        (t.accent, Stroke::NONE)
    } else {
        let press = theme::Press::track(ui, &resp);
        (press.fill(t.field, t.hover, t.pressed), Stroke::new(1.0, t.border))
    };
    ui.painter().rect(rect, CornerRadius::same(13), fill, stroke, egui::StrokeKind::Inside);
    ui.painter().galley(rect.center() - label_galley.size() / 2.0, label_galley, text_color);

    if resp.has_focus() {
        ui.painter().rect_stroke(rect.expand(1.0), CornerRadius::same(14), Stroke::new(2.0, t.accent_text), egui::StrokeKind::Outside);
    }

    let resp = theme::hand(resp);
    let key_select = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Space) || i.key_pressed(egui::Key::Enter));
    if resp.clicked() || key_select {
        *active = !*active;
        changed = true;
    }
    (changed, resp)
}

fn layout_pill_label(ui: &egui::Ui, label: &str, query: &str, font: egui::FontId, normal_color: Color32) -> std::sync::Arc<egui::Galley> {
    let ranges = find_match_ranges(label, query);
    if ranges.is_empty() {
        crate::bidi::layout_ui(ui, label.to_owned(), font, normal_color)
    } else {
        let (job, _) = create_highlight_job(label, &ranges, font, normal_color);
        ui.painter().layout_job(job)
    }
}

/// A standard card container for a Preferences section.
/// Groups related settings with clean margins, subtle border and rounded corners.
pub fn preference_card<R>(
    ui: &mut egui::Ui,
    t: &Tokens,
    title: &str,
    icon: Option<&str>,
    ctx: &mut PrefSearchContext,
    content: impl FnOnce(&mut egui::Ui, &mut PrefSearchContext) -> R,
) -> R {
    let card_fill = if t.kind == ThemeKind::Light { Color32::from_rgb(0xF7, 0xF8, 0xFA) } else { Color32::from_rgb(0x24, 0x24, 0x28) };
    egui::Frame::new()
        .fill(card_fill)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if let Some(ic) = icon {
                    ui.add(icons::image(ic, 16.0, t.accent));
                }
                ctx.card_title(ui, title, t);
            });
            ui.add_space(8.0);
            content(ui, ctx)
        })
        .inner
}

/// Folds a character for case-insensitive search, folding dotted/dotless i variants
/// ('I', 'i', 'İ', 'ı') to 'i', Greek sigmas to 'σ', and otherwise following `char::to_lowercase`.
fn fold_search_char(c: char, mut f: impl FnMut(char)) {
    match c {
        'I' | 'i' | '\u{130}' | '\u{131}' => f('i'),
        'Σ' | 'σ' | 'ς' => f('σ'),
        _ => {
            for lc in c.to_lowercase() {
                f(lc);
            }
        }
    }
}

/// Finds non-overlapping (start_byte, end_byte) slices in `text` matching `query` case-insensitively.
/// All byte indices are guaranteed to align to valid char boundaries.
pub fn find_match_ranges(text: &str, query: &str) -> Vec<(usize, usize)> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let mut q_chars = Vec::new();
    for c in q.chars() {
        fold_search_char(c, |ch| q_chars.push(ch));
    }
    if q_chars.is_empty() {
        return Vec::new();
    }
    let char_indices: Vec<(usize, char)> = text.char_indices().collect();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < char_indices.len() {
        let mut matched = true;
        let mut text_char_count = 0;
        let mut q_idx = 0;

        while q_idx < q_chars.len() {
            if i + text_char_count >= char_indices.len() {
                matched = false;
                break;
            }
            let c = char_indices.get(i + text_char_count).map(|(_, ch)| *ch).unwrap_or('\0');
            let mut c_folded = ['\0'; 4];
            let mut len = 0;
            fold_search_char(c, |lc| {
                if len < c_folded.len() {
                    c_folded[len] = lc;
                    len += 1;
                }
            });
            if len == 0 || q_idx + len > q_chars.len() || q_chars.get(q_idx..q_idx + len) != Some(&c_folded[..len]) {
                matched = false;
                break;
            }
            q_idx += len;
            text_char_count += 1;
        }

        if matched
            && q_idx == q_chars.len()
            && text_char_count > 0
            && let Some(start_byte) = char_indices.get(i).map(|(pos, _)| *pos)
        {
            let end_byte = if let Some((pos, _)) = char_indices.get(i + text_char_count) { *pos } else { text.len() };
            ranges.push((start_byte, end_byte));
            i += text_char_count;
            continue;
        }
        i += 1;
    }
    ranges
}

/// Builds an `egui::text::LayoutJob` with yellow background for matching spans.
pub fn create_highlight_job(
    text: &str,
    ranges: &[(usize, usize)],
    normal_font: egui::FontId,
    normal_color: Color32,
) -> (egui::text::LayoutJob, usize) {
    let mut job = egui::text::LayoutJob::default();
    let normal_format = egui::text::TextFormat { font_id: normal_font.clone(), color: normal_color, ..Default::default() };
    let highlight_format = egui::text::TextFormat {
        font_id: egui::FontId::new(normal_font.size, egui::FontFamily::Proportional),
        color: Color32::BLACK,
        background: Color32::from_rgb(255, 220, 60),
        ..Default::default()
    };

    let mut cursor = 0;
    for &(start, end) in ranges {
        if start > cursor
            && let Some(sub) = text.get(cursor..start)
        {
            job.append(sub, 0.0, normal_format.clone());
        }
        if let Some(matched) = text.get(start..end) {
            job.append(matched, 0.0, highlight_format.clone());
        }
        cursor = end;
    }
    if cursor < text.len()
        && let Some(sub) = text.get(cursor..)
    {
        job.append(sub, 0.0, normal_format);
    }
    let count = ranges.len();
    (job, count)
}

/// Creates a `WidgetText` highlighting `query` in `text` and returning match count.
pub fn highlight_widget_text(text: &str, query: &str, normal_font: egui::FontId, normal_color: Color32) -> (egui::WidgetText, usize) {
    let ranges = find_match_ranges(text, query);
    if ranges.is_empty() {
        (egui::WidgetText::from(egui::RichText::new(text).font(normal_font).color(normal_color)), 0)
    } else {
        let (job, count) = create_highlight_job(text, &ranges, normal_font, normal_color);
        (egui::WidgetText::from(job), count)
    }
}

/// Context for browser-style in-situ text scanning, highlighting, and auto-scrolling in Preferences.
pub(crate) struct PrefSearchContext<'a> {
    pub query: &'a str,
    pub total_matches: usize,
    pub first_match_scrolled: bool,
    pub should_scroll: bool,
}

impl<'a> PrefSearchContext<'a> {
    pub fn new(query: &'a str, should_scroll: bool) -> Self {
        Self { query, total_matches: 0, first_match_scrolled: false, should_scroll }
    }

    pub fn scroll_if_first_match(&mut self, resp: &egui::Response, count: usize) {
        if count > 0 && self.should_scroll && !self.first_match_scrolled {
            resp.scroll_to_me(Some(egui::Align::Center));
            self.first_match_scrolled = true;
        }
    }

    pub fn checkbox(&mut self, ui: &mut egui::Ui, checked: &mut bool, label: &str, t: &Tokens) -> egui::Response {
        let (widget_text, count) = highlight_widget_text(label, self.query, theme::regular(13.0), t.text);
        self.total_matches += count;
        let resp = ui.add(widgets::checkbox(checked, widget_text));
        self.scroll_if_first_match(&resp, count);
        resp
    }

    pub fn label(&mut self, ui: &mut egui::Ui, text: &str, font: egui::FontId, color: Color32) -> egui::Response {
        let (widget_text, count) = highlight_widget_text(text, self.query, font, color);
        self.total_matches += count;
        let resp = ui.label(widget_text);
        self.scroll_if_first_match(&resp, count);
        resp
    }

    pub fn label_primary(&mut self, ui: &mut egui::Ui, text: &str, t: &Tokens) -> egui::Response {
        self.label(ui, text, theme::regular(13.0), t.text)
    }

    pub fn label_secondary(&mut self, ui: &mut egui::Ui, text: &str, t: &Tokens) -> egui::Response {
        self.label(ui, text, theme::regular(11.5), t.text_muted)
    }

    pub fn card_title(&mut self, ui: &mut egui::Ui, title: &str, t: &Tokens) -> egui::Response {
        self.label(ui, title, theme::semibold(13.5), t.text)
    }

    pub fn pill_choice<T: PartialEq + Copy>(&mut self, ui: &mut egui::Ui, t: &Tokens, current: &mut T, options: &[(T, &str)]) -> bool {
        let mut matches = 0;
        for &(_, label) in options {
            matches += find_match_ranges(label, self.query).len();
        }
        self.total_matches += matches;
        let with_a11y: Vec<(T, &str, &str)> = options.iter().map(|&(v, l)| (v, l, l)).collect();
        let (changed, match_resp) = segmented_pill_choice_a11y_query(ui, t, current, &with_a11y, self.query);
        if let Some(resp) = match_resp {
            self.scroll_if_first_match(&resp, matches);
        }
        changed
    }

    pub fn pill_choice_a11y<T: PartialEq + Copy>(&mut self, ui: &mut egui::Ui, t: &Tokens, current: &mut T, options: &[(T, &str, &str)]) -> bool {
        let mut matches = 0;
        for &(_, label, _) in options {
            matches += find_match_ranges(label, self.query).len();
        }
        self.total_matches += matches;
        let (changed, match_resp) = segmented_pill_choice_a11y_query(ui, t, current, options, self.query);
        if let Some(resp) = match_resp {
            self.scroll_if_first_match(&resp, matches);
        }
        changed
    }

    pub fn toggle_pill(&mut self, ui: &mut egui::Ui, t: &Tokens, active: &mut bool, label: &str) -> bool {
        let matches = find_match_ranges(label, self.query).len();
        self.total_matches += matches;
        let (changed, resp) = toggle_pill_query(ui, t, active, label, self.query);
        self.scroll_if_first_match(&resp, matches);
        changed
    }
}

/// Whether the interface can draw a language picker's native name.
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

/// The date picker's displayed name.
pub(crate) fn date_language_picker_name(l: &pdfcraft_engine::dates::DateLanguage, drawable: impl FnOnce(&str) -> bool) -> &'static str {
    match crate::i18n::Lang::from_code(l.code) {
        Some(language) => language.picker_name(drawable),
        None => l.name,
    }
}

/// Interactive search box widget at the top of Preferences.
fn preference_search_box_at(ui: &mut egui::Ui, t: &Tokens, query: &mut String, matches_count: Option<usize>, rect: Rect) {
    let stroke = Stroke::new(1.0, t.border);
    ui.painter().rect(rect, CornerRadius::same(15), t.field, stroke, egui::StrokeKind::Inside);

    // Magnifying glass icon: 14pt icon centered vertically at (rect.min.x + 16, rect.center().y)
    let icon_rect = Rect::from_center_size(pos2(rect.min.x + 16.0, rect.center().y), vec2(14.0, 14.0));
    icons::paint(ui, icon_rect, "search", 14.0, t.text_muted);

    let badge_text = if !query.trim().is_empty() {
        let count = matches_count.unwrap_or(0);
        Some(crate::i18n::trn(crate::i18n::current(), count as u64, "{n} match", "{n} matches"))
    } else {
        None
    };

    let mut right_cursor = rect.max.x;
    let edit_id = ui.auto_id_with("search_input");

    if !query.is_empty() {
        let clear_rect = Rect::from_center_size(pos2(right_cursor - 14.0, rect.center().y), vec2(16.0, 16.0));
        let clear_resp = ui.interact(clear_rect, ui.auto_id_with("clear_search"), Sense::click());
        clear_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Clear search")));
        icons::paint(ui, clear_rect, "x", 12.0, if clear_resp.hovered() { t.text } else { t.text_muted });
        if clear_resp.clicked() {
            query.clear();
            let state = egui::text_edit::TextEditState::default();
            state.store(ui.ctx(), edit_id);
        }
        right_cursor -= 28.0;
    }

    if let Some(ref text) = badge_text {
        let font = theme::medium(11.0);
        let count = matches_count.unwrap_or(0);
        let badge_color = if count > 0 { t.accent_text } else { t.text_muted };
        let galley = crate::bidi::layout_ui(ui, text.clone(), font, badge_color);
        let bw = galley.size().x;
        let badge_pos = pos2(right_cursor - bw - 4.0, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(badge_pos, galley, badge_color);
        let badge_rect = Rect::from_min_size(badge_pos, vec2(bw, 16.0));
        let badge_resp = ui.interact(badge_rect, ui.auto_id_with("search_match_count"), Sense::hover());
        badge_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text.clone()));
        right_cursor -= bw + 8.0;
    }

    let edit_rect = Rect::from_min_max(pos2(rect.min.x + 28.0, rect.min.y), pos2(right_cursor.max(rect.min.x + 30.0), rect.max.y));
    let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(edit_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let edit = child_ui.add(
        egui::TextEdit::singleline(query)
            .id(edit_id)
            .hint_text(tl!("Search settings…"))
            .frame(egui::Frame::NONE)
            .align(egui::Align2::LEFT_CENTER)
            .desired_width(edit_rect.width()),
    );
    edit.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, tl!("Search settings…")));

    if edit.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        query.clear();
        let state = egui::text_edit::TextEditState::default();
        state.store(ui.ctx(), edit_id);
    }
}

/// Top header for Preferences: Title on the left, search box and match count badge on the right.
pub(crate) fn preferences_header(ui: &mut egui::Ui, t: &Tokens) {
    let search_id = egui::Id::unique("pdfcraft_preferences_search_query");
    let mut query: String = ui.data(|d| d.get_temp(search_id)).unwrap_or_default();
    let matches_id = egui::Id::unique("pdfcraft_preferences_search_matches");
    let last_matches: usize = ui.data(|d| d.get_temp(matches_id)).unwrap_or(0);

    // Top Header: Title on the left, Search bar on the right.
    // Allocate the full available width so the search bar aligns flush with the card boxes below.
    let full_w = ui.available_width();
    let header_h = 30.0;
    let (header_rect, _) = ui.allocate_exact_size(vec2(full_w, header_h), Sense::hover());

    // Title on the left, vertically centered in the header row:
    let title_rect = Rect::from_min_max(header_rect.min, pos2(header_rect.min.x + 240.0, header_rect.max.y));
    let mut title_ui = ui.new_child(egui::UiBuilder::new().max_rect(title_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    title_ui.label(RichText::new(tl!("Preferences")).font(theme::semibold(18.0)).color(t.text));

    // Search bar on the right, right edge flush with the card box boundary:
    let search_w = 230.0;
    let search_rect = Rect::from_min_max(pos2(header_rect.max.x - search_w, header_rect.min.y), header_rect.max);
    let match_count = (!query.trim().is_empty()).then_some(last_matches);
    let before = query.clone();
    preference_search_box_at(ui, t, &mut query, match_count, search_rect);
    if query != before {
        ui.data_mut(|d| d.insert_temp(search_id, query));
        ui.ctx().request_repaint();
    }
}

/// The body of the Preferences dialog with card layout and browser-style in-situ text scanning.
pub(crate) fn preferences_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    let search_id = egui::Id::unique("pdfcraft_preferences_search_query");
    let query: String = ui.data(|d| d.get_temp(search_id)).unwrap_or_default();
    let matches_id = egui::Id::unique("pdfcraft_preferences_search_matches");
    let last_matches: usize = ui.data(|d| d.get_temp(matches_id)).unwrap_or(0);
    let last_scrolled_id = egui::Id::unique("pdfcraft_pref_last_scrolled_query");
    let last_scrolled_query: String = ui.data(|d| d.get_temp(last_scrolled_id)).unwrap_or_default();

    let should_scroll = !query.trim().is_empty() && query != last_scrolled_query;
    let mut ctx = PrefSearchContext::new(&query, should_scroll);

    // Card 1: General & Interface
    let title1 = tl!("General & Interface");
    preference_card(ui, t, title1, Some("settings"), &mut ctx, |ui, ctx| {
        card_general(ui, app, t, ctx);
    });
    ui.add_space(12.0);

    // Card 2: Appearance & Theme
    let title2 = tl_ctx!("theme", "Appearance");
    preference_card(ui, t, title2, Some("sun"), &mut ctx, |ui, ctx| {
        card_appearance(ui, app, t, ctx);
    });
    ui.add_space(12.0);

    // Card 3: Documents and View
    let title3 = tl!("Documents and view");
    preference_card(ui, t, title3, Some("file-text"), &mut ctx, |ui, ctx| {
        card_documents_and_view(ui, app, t, ctx);
    });
    ui.add_space(12.0);

    // Card 4: Comment
    let title4 = tl!("Comment");
    preference_card(ui, t, title4, Some("message-square-text"), &mut ctx, |ui, ctx| {
        card_identity(ui, app, t, ctx);
    });
    ui.add_space(12.0);

    // Card 5: Fill & Sign
    let title5 = tl!("Fill & Sign");
    preference_card(ui, t, title5, Some("pen-tool"), &mut ctx, |ui, ctx| {
        card_fill_and_sign(ui, app, t, ctx);
    });
    ui.add_space(12.0);

    // Card 6: JavaScript
    let title6 = "JavaScript";
    preference_card(ui, t, title6, Some("code-xml"), &mut ctx, |ui, ctx| {
        card_javascript(ui, app, t, ctx);
    });
    ui.add_space(12.0);

    if ctx.first_match_scrolled {
        ui.data_mut(|d| d.insert_temp(last_scrolled_id, query.clone()));
    } else if query.trim().is_empty() {
        ui.data_mut(|d| d.insert_temp(last_scrolled_id, String::new()));
    }

    if ctx.total_matches != last_matches {
        ui.data_mut(|d| d.insert_temp(matches_id, ctx.total_matches));
        ui.ctx().request_repaint();
    }
}

/// Content for Card 1: General & Interface
fn card_general(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, ctx: &mut PrefSearchContext) {
    ui.horizontal(|ui| {
        let label = ctx.label_primary(ui, tl!("Interface language"), t);
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
            .width(160.0)
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
        if app.language != before {
            crate::i18n::set_current(crate::i18n::Lang::from_pref(&app.language));
        }

        ui.add_space(16.0);
        use crate::ui_scale;
        let label_size = ctx.label_primary(ui, tl!("Interface size"), t);
        let native = ui.ctx().native_pixels_per_point().unwrap_or(1.0);
        let auto_percent = ui_scale::percent(ui_scale::drawn(None, app.system_text_scale, native));
        let auto = crate::i18n::fmt(tl!("Auto ({percent}%)"), &[("percent", &auto_percent.to_string())]);
        let selected = app.ui_scale.map_or_else(|| auto.clone(), |f| format!("{}%", ui_scale::percent(f)));
        craft_ui::ComboBox::from_id_salt("interface-size")
            .selected_text(selected)
            .width(130.0)
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
            .labelled_by(label_size.id);
    });
    ctx.label_secondary(ui, tl!("Auto follows the system's text size. Pages always render at the display's full resolution."), t);
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ctx.label_primary(ui, tl!("Tools panel side"), t);
        let right = app.side_panels.tools_on_right();
        let mut side = if right { ToolsSide::Right } else { ToolsSide::Left };
        if ctx.pill_choice(ui, t, &mut side, &[(ToolsSide::Left, tl!("Left")), (ToolsSide::Right, tl!("Right"))]) {
            app.side_panels.tools_side = side;
        }

        ui.add_space(16.0);
        ctx.label_primary(ui, tl!("Navigation rail side"), t);
        let rail_right = app.side_panels.rail_on_right();
        let mut rside = if rail_right { ToolsSide::Right } else { ToolsSide::Left };
        let left_a11y = format!("{}: {}", tl!("Navigation rail side"), tl!("Left"));
        let right_a11y = format!("{}: {}", tl!("Navigation rail side"), tl!("Right"));
        if ctx.pill_choice_a11y(ui, t, &mut rside, &[(ToolsSide::Left, tl!("Left"), &left_a11y), (ToolsSide::Right, tl!("Right"), &right_a11y)]) {
            app.side_panels.rail_side = rside;
        }
    });
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ctx.label_primary(ui, tl!("Animation effects"), t);
        ctx.pill_choice(
            ui,
            t,
            &mut app.animation_effects,
            &[
                (AnimationEffects::System, tl_ctx!("animation effects", AnimationEffects::System.label())),
                (AnimationEffects::On, tl_ctx!("animation effects", AnimationEffects::On.label())),
                (AnimationEffects::Off, tl_ctx!("animation effects", AnimationEffects::Off.label())),
            ],
        );
    });
    ui.add_space(6.0);

    // The browser draws the web build. Chosen before the window exists, so it applies at the
    // next start (crate::renderer explains the flicker it works around).
    #[cfg(not(target_arch = "wasm32"))]
    {
        ui.horizontal(|ui| {
            ctx.label_primary(ui, tl!("Graphics"), t);
            // "OpenGL" is a name, not translated.
            ctx.pill_choice(
                ui,
                t,
                &mut app.renderer,
                &[(crate::renderer::RendererPreference::Auto, tl!("Auto")), (crate::renderer::RendererPreference::Gl, crate::renderer::RendererPreference::Gl.label())],
            );
        });
        ui.label(
            egui::RichText::new(tl!(
                "Applies the next time PdfCraft starts. Choose OpenGL if the window flickers on a display with G-SYNC, FreeSync or another variable refresh rate."
            ))
            .small()
            .color(t.text_muted),
        );
        ui.add_space(6.0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    ctx.checkbox(ui, &mut app.reopen_last_session, tl!("Reopen the files that were open when PdfCraft last closed"), t);
    #[cfg(not(target_arch = "wasm32"))]
    ctx.checkbox(ui, &mut app.restore_last_view, tl!("Restore the last view (page, zoom and page display) when a document is opened again"), t);
    #[cfg(not(target_arch = "wasm32"))]
    ctx.checkbox(ui, &mut app.reload_changed_files, tl!("Reload documents when their files change on disk"), t);
    ctx.checkbox(ui, &mut app.show_secured_notice, tl!("Show a notice when a document is secured"), t);

    #[cfg(not(target_arch = "wasm32"))]
    {
        ui.horizontal(|ui| {
            let label = ctx.label_primary(ui, tl!("Recent files to keep"), t);
            ui.add(egui::DragValue::new(&mut app.recent_limit).range(0..=crate::RECENT_FILES_MAX)).labelled_by(label.id);
        });
        ctx.label_secondary(ui, tl!("Lowering this forgets the oldest files. At 0 there is no list."), t);
    }

    if app.window_controls.is_some() {
        ctx.checkbox(ui, &mut app.system_title_bar, tl!("Use system title bar"), t).on_hover_text(tl!(
            "Show a basic title bar for the window buttons instead of the tab strip. GNOME doesn't draw title bars for apps, so this one doesn't follow its theme."
        ));
    }
}

/// Content for Card 2: Appearance & Theme
fn card_appearance(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, ctx: &mut PrefSearchContext) {
    if ctx.checkbox(ui, &mut app.show_page_edge_shadows, tl!("Show Page Edge Shadows"), t).changed() {
        ui.ctx().request_repaint();
    }
    ui.add_space(4.0);
    crate::appearance::preferences_section_with_context(ui, app, t, ctx);
    ui.add_space(8.0);
    reading_colours_section(ui, app, t, ctx);
}

/// Content for Card 3: Documents and View
fn card_documents_and_view(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, ctx: &mut PrefSearchContext) {
    ui.horizontal(|ui| {
        ctx.label_primary(ui, tl!("Default workspace mode"), t);
        ctx.pill_choice(
            ui,
            t,
            &mut app.default_mode,
            &[
                (crate::Mode::AllTools, tl!("All tools")),
                (crate::Mode::Read, tl!("Read")),
                (crate::Mode::Edit, tl!("Edit")),
                (crate::Mode::Convert, tl!("Convert")),
                (crate::Mode::Sign, tl!("E-Sign")),
            ],
        );
    });
    ctx.label_secondary(ui, tl!("Used when opening PDFs. An explicit launch or control mode takes precedence for the session."), t);
    ui.add_space(6.0);

    let defaults = &mut app.view_defaults;
    ui.horizontal(|ui| {
        ctx.label_primary(ui, tl!("Default page display"), t);
        let mut layout_choice = defaults.layout;
        if ctx.pill_choice(ui, t, &mut layout_choice, &[(PageLayout::Single, tl!("Single page")), (PageLayout::TwoUp, tl!("Two-page view"))]) {
            defaults.layout = layout_choice;
        }
        let mut continuous = defaults.continuous;
        if ctx.toggle_pill(ui, t, &mut continuous, tl!("Continuous scrolling")) {
            defaults.continuous = continuous;
        }
    });
    ctx.checkbox(ui, &mut defaults.show_page_gaps, tl!("Show gaps between pages"), t);
    ctx.label_secondary(ui, tl!("Hide gaps to join page edges. Margins inside the PDF remain unchanged."), t);
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ctx.label_primary(ui, tl!("Default zoom"), t);
        ctx.pill_choice(
            ui,
            t,
            &mut defaults.fit,
            &[(Fit::Width, tl!("Fit to width")), (Fit::Page, tl!("Zoom to page level")), (Fit::None, tl!("Custom"))],
        );
        let mut percent = defaults.zoom * 100.0;
        if ui.add(egui::DragValue::new(&mut percent).range(8.0..=6400.0).max_decimals(0).suffix("%")).changed() {
            (defaults.fit, defaults.zoom) = (Fit::None, percent / 100.0);
        }
    });
    ctx.label_secondary(ui, tl!("Used when a PDF doesn't ask for a layout or zoom. Continuous scrolling never snaps between pages."), t);
    ctx.checkbox(ui, &mut app.always_default_view, tl!("Always use my default zoom and page layout"), t);
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ctx.label_primary(ui, tl!("Units"), t);
        ctx.pill_choice(
            ui,
            t,
            &mut app.units,
            &[
                (Unit::Inches, Unit::Inches.label()),
                (Unit::Millimetres, Unit::Millimetres.label()),
                (Unit::Centimetres, Unit::Centimetres.label()),
                (Unit::Points, Unit::Points.label()),
            ],
        );
    });
    ctx.label_secondary(ui, tl!("For page sizes and margins."), t);
    ui.add_space(6.0);
    scrolling(ui, app, t, ctx);
}

/// The scroll speed and the middle button's direction, in Card 3.
fn scrolling(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, ctx: &mut PrefSearchContext) {
    ui.horizontal(|ui| {
        ctx.label_primary(ui, tl!("Scroll speed (mouse wheel and arrow keys)"), t);
        ctx.pill_choice(
            ui,
            t,
            &mut app.scroll_speed,
            &[
                (ScrollSpeed::Slow, tl_ctx!("scroll speed", ScrollSpeed::Slow.label())),
                (ScrollSpeed::Normal, tl_ctx!("scroll speed", ScrollSpeed::Normal.label())),
                (ScrollSpeed::Fast, tl_ctx!("scroll speed", ScrollSpeed::Fast.label())),
            ],
        );
    });
    ui.add_space(6.0);
    ctx.checkbox(ui, &mut app.invert_middle_drag, tl!("Invert middle-button drag"), t);
    ctx.label_secondary(ui, tl!("Panning or auto-scrolling with the middle (wheel) button moves the page the other way."), t);
}

/// Content for Card 4: Identity
fn card_identity(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, ctx: &mut PrefSearchContext) {
    ui.horizontal(|ui| {
        let label = ctx.label_primary(ui, tl!("Name on new comments"), t);
        ui.add(crate::widgets::line(&mut app.comment_prefs.author).desired_width(240.0).char_limit(crate::MAX_AUTHOR_CHARS)).labelled_by(label.id);
    });
    ctx.checkbox(ui, &mut app.comment_prefs.hide_empty_highlight_hover, tl!("Hide hover previews for highlights without comment text"), t);
}

/// Content for Card 5: Fill & Sign
fn card_fill_and_sign(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, ctx: &mut PrefSearchContext) {
    ctx.checkbox(ui, &mut app.flatten_fill_sign_on_save, tl!("Flatten Fill & Sign when saving"), t);
    ctx.label_secondary(ui, tl!("Text, marks and signatures become part of the page. Other comments stay editable."), t);
    ui.add_space(8.0);
    date_format(ui, app, t, ctx);
}

/// Content for Card 6: JavaScript
fn card_javascript(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, ctx: &mut PrefSearchContext) {
    let mut on = app.session.javascript();
    if ctx.checkbox(ui, &mut on, tl!("Enable Acrobat JavaScript"), t).changed() {
        app.session.set_javascript(on);
    }
    ctx.label_secondary(
        ui,
        tl!(
            "Scripts run in a sandbox without file or network access. A script that asks to open a web page needs your permission first. With JavaScript off, Acrobat's standard format, validate and calculate functions still work."
        ),
        t,
    );
}

/// Preferences dialog buttons: OK on the right. Returns true to close.
pub(crate) fn preferences_buttons(ui: &mut egui::Ui) -> bool {
    ui.add_space(10.0);
    let mut close = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("OK"), true).clicked() {
                close = true;
            }
        });
    });
    close
}

/// Fill & Sign date pattern and date language picker.
fn date_format(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, ctx: &mut PrefSearchContext) {
    use pdfcraft_engine::dates::{DATE_FORMATS, DATE_LANGUAGES, MAX_DATE_FORMAT_CHARS};
    ctx.label(ui, tl!("Date format"), theme::semibold(12.5), t.text);
    let sample = |app: &PdfCraftApp, f: &str| format!("{}   {f}", app.date_text(Some(f)).unwrap_or_default());
    ui.horizontal(|ui| {
        let label = ctx.label_primary(ui, tl!("Fill & Sign dates"), t);
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
        let label = ctx.label_primary(ui, tl!("Pattern"), t);
        let mut text = app.date_format_draft.clone().unwrap_or_else(|| app.session.date_format().to_string());
        let edit = ui.add(egui::TextEdit::singleline(&mut text).desired_width(160.0).char_limit(MAX_DATE_FORMAT_CHARS)).labelled_by(label.id);
        if edit.changed() {
            let _ = app.session.set_date_format(&text);
            app.date_format_draft = Some(text.clone());
        }
        if !edit.has_focus() && pdfcraft_engine::dates::check_date_format(&text).is_ok() {
            app.date_format_draft = None;
        }
        match app.date_text(Some(&text)) {
            Ok(today) => {
                let today_str = crate::i18n::fmt(tl!("Today: {date}"), &[("date", &today)]);
                ctx.label_secondary(ui, &today_str, t);
            }
            Err(e) => {
                ctx.label(ui, &e, theme::regular(13.0), Color32::from_rgb(0xD1, 0x3B, 0x3B));
            }
        };
    });
    let bad = app.date_text(None).map(|today| pdfcraft_engine::dates::unwritable(&today)).unwrap_or_default();
    if !bad.is_empty() {
        let bad_msg = crate::i18n::fmt(
            tl!("{chars} can't be written into the PDF yet: Fill & Sign text is Western European only. Pick a numeric format such as dd.mm.yyyy."),
            &[("chars", &bad)],
        );
        ctx.label(ui, &bad_msg, theme::regular(13.0), Color32::from_rgb(0xD1, 0x3B, 0x3B));
    }
    ui.horizontal(|ui| {
        let label = ctx.label_primary(ui, tl!("Language"), t);
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
    let date_help = crate::i18n::fmt(
        tl!(
            "yyyy or yy year · m or mm month · mmm or mmmm month name · d or dd day · ddd or dddd weekday. H, h, M, s and t are time letters and can't be used on their own: put {escape} before a letter to show it as is."
        ),
        &[("escape", "\\")],
    );
    ctx.label_secondary(ui, &date_help, t);
}

/// Reading colours settings: presets, preview and custom replacement options.
fn reading_colours_section(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens, ctx: &mut PrefSearchContext) {
    ctx.label(ui, tl!("Reading colours"), theme::semibold(13.0), t.text);
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
                ui.set_width(100.0);
                let sample = RichText::new("Aa").size(20.0).color(Color32::from_rgb(params.text[0], params.text[1], params.text[2]));
                let button = egui::Button::new(sample)
                    .fill(Color32::from_rgb(params.background[0], params.background[1], params.background[2]))
                    .min_size(vec2(90.0, 40.0))
                    .stroke(Stroke::new(if mode == name { 2.0 } else { 1.0 }, if mode == name { t.accent } else { t.border }));
                let response = ui.add(button);
                response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, mode == name, tl!(label)));
                if response.clicked() {
                    app.set_reading_colors(colors);
                }
                ctx.label(ui, tl!(label), theme::regular(12.0), t.text);
            });
        }
    });
    ui.add_space(4.0);
    if let Some(params) = app.reading_colors.params {
        let text = Color32::from_rgb(params.text[0], params.text[1], params.text[2]);
        let background = Color32::from_rgb(params.background[0], params.background[1], params.background[2]);
        egui::Frame::new().fill(background).inner_margin(12.0).corner_radius(CornerRadius::same(6)).show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 24.0).max(0.0));
            ctx.label(ui, tl!("Reading colours"), theme::semibold(16.0), text);
            ctx.label(ui, tl!("Colours are remapped while rendering; the document itself never changes."), theme::regular(13.0), text);
        });
    } else {
        ctx.label_primary(ui, tl!("Original"), t);
    }
    egui::CollapsingHeader::new(tl!("Custom")).default_open(mode == "custom").show(ui, |ui| {
        let mut colors = if app.reading_colors.params.is_some() { app.reading_colors } else { pdfcraft_render::ReadingColors::BLACK_ON_WHITE };
        let mut changed = false;
        if let Some(params) = colors.params.as_mut() {
            ui.horizontal(|ui| {
                let text = ctx.label_primary(ui, tl!("Text"), t);
                changed |= ui.color_edit_button_srgb(&mut params.text).labelled_by(text.id).changed();
                let background = ctx.label_primary(ui, tl!("Page background"), t);
                changed |= ui.color_edit_button_srgb(&mut params.background).labelled_by(background.id).changed();
            });
            changed |= ctx.checkbox(ui, &mut params.only_black, tl!("Only change the colour of black text or line art"), t).changed();
            changed |= ctx.checkbox(ui, &mut params.line_art, tl!("Change the colour of line art as well as text"), t).changed();
        }
        if changed {
            app.set_reading_colors(colors);
        }
    });
    ctx.label_secondary(ui, tl!("Colours are remapped while rendering; the document itself never changes."), t);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_match_ranges_case_insensitive() {
        let text = "Hide hover previews for highlights without comment text";
        let ranges = find_match_ranges(text, "hide hover");
        assert_eq!(ranges, vec![(0, 10)]);
        assert_eq!(text.get(ranges[0].0..ranges[0].1), Some("Hide hover"));

        let text2 = "Highlight fields and highlight text";
        let ranges2 = find_match_ranges(text2, "highlight");
        assert_eq!(ranges2.len(), 2);
        assert_eq!(text2.get(ranges2[0].0..ranges2[0].1), Some("Highlight"));
        assert_eq!(text2.get(ranges2[1].0..ranges2[1].1), Some("highlight"));

        let ranges3 = find_match_ranges(text, "not found");
        assert!(ranges3.is_empty());

        let ranges4 = find_match_ranges(text, "");
        assert!(ranges4.is_empty());
    }

    #[test]
    fn test_find_match_ranges_turkish_i() {
        let text = "Doldur ve İmzala";
        let ranges = find_match_ranges(text, "imzala");
        assert_eq!(ranges.len(), 1);
        assert_eq!(text.get(ranges[0].0..ranges[0].1), Some("İmzala"));

        let ranges2 = find_match_ranges(text, "İMZALA");
        assert_eq!(ranges2.len(), 1);
        assert_eq!(text.get(ranges2[0].0..ranges2[0].1), Some("İmzala"));

        let text2 = "Doldur ve imzala";
        let ranges3 = find_match_ranges(text2, "İmzala");
        assert_eq!(ranges3.len(), 1);
        assert_eq!(text2.get(ranges3[0].0..ranges3[0].1), Some("imzala"));

        let ranges4 = find_match_ranges(text, "ımzala");
        assert_eq!(ranges4.len(), 1);
        assert_eq!(text.get(ranges4[0].0..ranges4[0].1), Some("İmzala"));
    }
}
