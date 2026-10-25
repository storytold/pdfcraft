//! Small custom widgets built on the design tokens.

use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Stroke, vec2};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, icons};

/// Horizontal and vertical inset of a single-line field.
const FIELD_MARGIN: egui::Margin = egui::Margin::symmetric(10, 6);
/// Where text starts inside a [`line`] or [`block`] field, so plain values beside fields can line up with it.
pub(crate) const FIELD_TEXT_INSET: f32 = FIELD_MARGIN.left as f32;
/// Outer height of a single-line field, including its inset.
const FIELD_MIN_HEIGHT: f32 = 30.0;
/// Padding inside every modal.
const DIALOG_PAD_X: i8 = 20;
const DIALOG_PAD_Y: i8 = 16;

fn dialog_frame(ctx: &egui::Context) -> egui::Frame {
    let t = Tokens::get(ctx);
    egui::Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(DIALOG_PAD_X, DIALOG_PAD_Y))
        .shadow(ctx.global_style().visuals.window_shadow)
}

/// A modal with the card fill, a 10-point corner and room around its contents.
pub fn modal(ctx: &egui::Context, id: impl std::hash::Hash + std::fmt::Debug) -> egui::Modal {
    egui::Modal::new(egui::Id::new(id)).frame(dialog_frame(ctx))
}

/// Show [`modal`], outline its controls, and remember the card rect for layout checks.
pub fn show_modal<R>(ctx: &egui::Context, id: &str, content: impl FnOnce(&mut egui::Ui) -> R) -> egui::ModalResponse<R> {
    let stored = egui::Id::new(id);
    modal(ctx, id).show(ctx, |ui| {
        begin_dialog(ui);
        let inner = content(ui);
        // The frame's stroke sits outside this content rect; the stored card is the fill.
        let card = ui.min_rect().expand2(egui::vec2(f32::from(DIALOG_PAD_X), f32::from(DIALOG_PAD_Y)));
        ui.ctx().data_mut(|d| d.insert_temp(stored.with("card"), card));
        inner
    })
}

/// Outlined controls at a shared 30-point height, with a focus stroke and 4-point corners.
///
/// Dialog fills match the theme's field colour, so radio buttons, check boxes, combo boxes
/// and number fields need the border or they disappear into the card.
pub fn begin_dialog(ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let w = &mut ui.visuals_mut().widgets;
    w.inactive.bg_stroke = Stroke::new(1.0, t.border);
    w.inactive.weak_bg_fill = t.field;
    // Slider rails and check-box interiors use the plain fill.
    w.inactive.bg_fill = t.hover;
    w.hovered.bg_stroke = Stroke::new(1.0, t.text_muted);
    for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.corner_radius = CornerRadius::same(4);
    }
    ui.visuals_mut().selection.stroke = Stroke::new(1.5, t.accent);
    ui.spacing_mut().interact_size.y = FIELD_MIN_HEIGHT;
}

/// A single-line field tall enough to read and to click.
pub fn line(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).margin(FIELD_MARGIN).min_size(vec2(0.0, FIELD_MIN_HEIGHT))
}

/// A multiline field with the same horizontal inset as [`line`].
pub fn block(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::multiline(text).margin(egui::Margin::symmetric(10, 6))
}

/// One line that stays within `max_width`, cut on a character boundary with an ellipsis.
/// Only the first 160 characters are measured, so a huge name cannot lay out thousands of candidates.
pub(crate) fn fit_line(text: &str, max_width: f32, mut width_of: impl FnMut(&str) -> f32) -> String {
    if !max_width.is_finite() || max_width <= 0.0 {
        return String::new();
    }
    let mut s: String = text.chars().take(160).collect();
    if s.len() == text.len() && width_of(text) <= max_width {
        return text.to_owned();
    }
    loop {
        s.pop();
        if s.is_empty() {
            return if width_of("\u{2026}") <= max_width { "\u{2026}".into() } else { String::new() };
        }
        let candidate = format!("{s}\u{2026}");
        if width_of(&candidate) <= max_width {
            return candidate;
        }
    }
}

pub(crate) fn text_width(ui: &egui::Ui, text: &str, font: &egui::FontId) -> f32 {
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE).size().x)
}

/// Draw `text` from `pos` (left, vertically centered) so it never runs past `max_width`.
pub(crate) fn paint_left(ui: &egui::Ui, pos: egui::Pos2, text: &str, font: egui::FontId, color: Color32, max_width: f32) {
    let shown = fit_line(text, max_width, |s| text_width(ui, s, &font));
    let clip = Rect::from_min_size(egui::pos2(pos.x, pos.y - 14.0), vec2(max_width.max(0.0), 28.0));
    ui.painter().with_clip_rect(clip).text(pos, Align2::LEFT_CENTER, shown, font, color);
}

/// A mode-bar tab: text with an underline when active.
pub fn mode_tab(ui: &mut egui::Ui, label: &str, active: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = if active { theme::semibold(13.5) } else { theme::medium(13.5) };
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 22.0, 48.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), active, label));
    let press = theme::Press::track(ui, &resp);
    let well = rect.shrink2(vec2(2.0, 9.0));
    if active {
        if press.down > 0.01 {
            ui.painter().rect_filled(well, CornerRadius::same(6), t.pressed.gamma_multiply(press.down));
        }
    } else {
        press.wash(ui, well, 6, false);
    }
    let body = rect.translate(press.offset());
    ui.painter().text(body.center(), Align2::CENTER_CENTER, label, font, if active { t.text } else { t.text_muted });
    if active {
        let r = Rect::from_min_max(rect.left_bottom() + vec2(10.0, -2.0), rect.right_bottom() - vec2(10.0, 0.0));
        ui.painter().rect_filled(r, CornerRadius { nw: 1, ne: 1, sw: 0, se: 0 }, t.accent);
    }
    theme::hand(resp)
}

/// Rounded pill button; `primary` fills with the accent.
pub fn pill_button(ui: &mut egui::Ui, label: &str, primary: bool) -> Response {
    pill_button_marked(ui, label, primary, false)
}

/// [`pill_button`] that, when `marked`, is the keyboard's choice: it takes egui focus (so
/// assistive tech follows it) and shows a ring in the accent colour around the pill.
pub fn pill_button_marked(ui: &mut egui::Ui, label: &str, primary: bool, marked: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(12.5);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 26.0, 28.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    let press = theme::Press::track(ui, &resp);
    let (fill, stroke, text) = if primary {
        (press.fill(t.accent, t.accent_text, t.accent.lerp_to_gamma(Color32::BLACK, 0.22)), Stroke::NONE, Color32::WHITE)
    } else {
        (press.fill(t.card, t.hover, t.pressed), Stroke::new(1.0, t.border), t.text)
    };
    ui.painter().rect(rect, CornerRadius::same(14), fill, stroke, egui::StrokeKind::Inside);
    if marked {
        resp.request_focus();
        ui.painter().rect_stroke(rect.expand(2.0), CornerRadius::same(16), Stroke::new(2.0, t.accent), egui::StrokeKind::Outside);
    }
    ui.painter().text(rect.translate(press.offset()).center(), Align2::CENTER_CENTER, label, font, text);
    theme::hand(resp)
}

/// Icon + label, transparent until hovered.
pub fn ghost_button(ui: &mut egui::Ui, icon: &str, label: &str) -> Response {
    ghost_button_sized(ui, icon, label, 30.0)
}

/// [`ghost_button`] at a given height (the macOS title bar is shorter than the tab strip elsewhere).
pub fn ghost_button_sized(ui: &mut egui::Ui, icon: &str, label: &str, height: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(13.0);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 38.0, height), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    let press = theme::Press::track(ui, &resp);
    press.wash(ui, rect, 6, false);
    let body = rect.translate(press.offset());
    icons::paint(ui, Rect::from_min_size(body.min + vec2(6.0, (height - 18.0) / 2.0), vec2(18.0, 18.0)), icon, 17.0, t.icon);
    ui.painter().text(body.left_center() + vec2(30.0, 0.0), Align2::LEFT_CENTER, label, font, t.text);
    theme::hand(resp)
}

/// A search-field lookalike that opens the command palette.
pub fn search_box(ui: &mut egui::Ui, placeholder: &str, width: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), placeholder));
    let press = theme::Press::track(ui, &resp);
    let fill = press.fill(t.field, t.hover, t.pressed);
    let edge = t.border.lerp_to_gamma(t.text_muted, press.hover * 0.35);
    ui.painter().rect(rect, CornerRadius::same(16), fill, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    icons::paint(ui, Rect::from_min_size(rect.min + vec2(10.0, 8.0), vec2(16.0, 16.0)), "search", 15.0, t.text_muted);
    let shortcut = crate::commands::command_shortcut_label(ui.ctx(), "view.palette");
    let shortcut = ui.painter().layout_no_wrap(shortcut, theme::regular(11.5), t.text_faint);
    let shortcut_pos = rect.right_center() - vec2(12.0 + shortcut.size().x, shortcut.size().y * 0.5);
    // Reserve the measured shortcut width in every language; long translations are elided
    // within the remaining space rather than painted underneath the shortcut.
    let text_right = (shortcut_pos.x - 10.0).max(rect.left());
    let text_left = (rect.left() + 34.0).min(text_right);
    let text_rect = Rect::from_min_max(egui::pos2(text_left, rect.top()), egui::pos2(text_right, rect.bottom()));
    let mut job = egui::text::LayoutJob::simple_singleline(placeholder.to_owned(), theme::regular(13.0), t.text_faint);
    job.wrap = egui::text::TextWrapping { max_width: text_rect.width(), max_rows: 1, break_anywhere: true, ..Default::default() };
    let text = ui.fonts_mut(|f| f.layout_job(job));
    let elided = text.elided;
    ui.painter().with_clip_rect(text_rect).galley(egui::pos2(text_left, rect.center().y - text.size().y * 0.5), text, t.text_faint);
    ui.painter().with_clip_rect(rect).galley(shortcut_pos, shortcut, t.text_faint);
    let resp = resp.on_hover_cursor(egui::CursorIcon::Text);
    if elided { resp.on_hover_text(placeholder) } else { resp }
}

pub fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> Response {
    ui.add(egui::Button::new(label).shortcut_text(shortcut))
}

/// Corner radius of a checkbox's box. With the widgets' radius (`Tokens::radius`, 6) egui's 14 px
/// box is a circle, the shape of a radio button, and View ▸ Page display has both.
const CHECKBOX_RADIUS: u8 = 3;

/// A checkbox whose box is an outlined rounded square, never mistaken for a radio button. Every
/// checkbox goes through here: clippy.toml disallows `ui.checkbox` and `egui::Checkbox`.
pub fn checkbox<'a>(checked: &'a mut bool, atoms: impl egui::IntoAtoms<'a>) -> impl egui::Widget + 'a {
    #[expect(clippy::disallowed_methods, reason = "the one place that styles the box")]
    let widget = egui::Checkbox::new(checked, atoms);
    toggle(widget, Some(CHECKBOX_RADIUS))
}

/// [`checkbox`] without a label: the selection box on Organize Pages thumbnails.
pub fn checkbox_without_text(checked: &mut bool) -> impl egui::Widget + '_ {
    #[expect(clippy::disallowed_methods, reason = "the one place that styles the box")]
    let widget = egui::Checkbox::without_text(checked);
    toggle(widget, Some(CHECKBOX_RADIUS))
}

/// A radio button with an outline and a fill, like [`checkbox`]. Every radio button goes through
/// here or [`radio_value`]: clippy.toml disallows `ui.radio`, `ui.radio_value` and
/// `egui::RadioButton`.
pub fn radio<'a>(selected: bool, atoms: impl egui::IntoAtoms<'a>) -> impl egui::Widget + 'a {
    #[expect(clippy::disallowed_methods, reason = "the one place that styles the circle")]
    let widget = egui::RadioButton::new(selected, atoms);
    toggle(widget, None)
}

/// [`radio`] selected while `*current == alternative`; a click selects it (`Ui::radio_value`).
pub fn radio_value<'a, V: PartialEq>(ui: &mut egui::Ui, current: &mut V, alternative: V, atoms: impl egui::IntoAtoms<'a>) -> Response {
    let mut response = ui.add(radio(*current == alternative, atoms));
    if response.clicked() && *current != alternative {
        *current = alternative;
        response.mark_changed();
    }
    response
}

/// The box of a checkbox or the circle of a radio button. egui draws it with the widgets' visuals
/// and has no style of its own for it, so it is set in a scope around the widget, keeping what
/// the caller set (dialogs and the search panel already outline theirs):
/// - an outline, darker on hover;
/// - on the light theme, the plain fill (`Tokens::hover`), as dialogs have: the field colour is
///   the panels' white, so the box would vanish;
/// - for a checkbox, a corner radius small enough to keep the box square.
fn toggle<'a>(widget: impl egui::Widget + 'a, radius: Option<u8>) -> impl egui::Widget + 'a {
    move |ui: &mut egui::Ui| {
        ui.scope(|ui| {
            let t = Tokens::get(ui.ctx());
            let w = &mut ui.visuals_mut().widgets;
            if let Some(radius) = radius {
                for v in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
                    v.corner_radius = CornerRadius::same(radius);
                }
            }
            if !t.dark() && w.inactive.bg_fill == t.field {
                w.inactive.bg_fill = t.hover;
            }
            if w.inactive.bg_stroke.is_empty() {
                w.inactive.bg_stroke = Stroke::new(1.0, t.border);
            }
            if w.hovered.bg_stroke.is_empty() {
                w.hovered.bg_stroke = Stroke::new(1.0, t.text_muted);
            }
            ui.add(widget)
        })
        .inner
    }
}

pub fn section_title(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(10.0);
    // Section titles across every panel go through here, so one translation point covers them.
    ui.label(egui::RichText::new(crate::i18n::upper_label(text)).font(theme::semibold(10.5)).color(t.text_faint).extra_letter_spacing(0.6));
    ui.add_space(2.0);
}

/// Paint one line of `text` left-aligned and vertically centred on `pos`, cut with "…" so it is
/// at most `max_width` wide: a long file name or path stays inside its row (#425).
pub fn row_text(ui: &egui::Ui, pos: egui::Pos2, text: impl Into<String>, font: egui::FontId, color: Color32, max_width: f32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.into(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(0.0));
    let galley = ui.painter().layout_job(job);
    let rect = Align2::LEFT_CENTER.anchor_size(pos, galley.size());
    ui.painter().galley(rect.min, galley, color);
}

/// Text wrapped to the available width, for text people write (comments, replies). A paragraph
/// in a right-to-left script (Persian, Arabic, Hebrew) reads right to left and sits against the
/// right edge ([`crate::bidi::wrap`]); text without one is an ordinary wrapping label.
/// `truncate` keeps the first line only, with an ellipsis where the text goes on.
pub fn wrapped_text(ui: &mut egui::Ui, text: &str, font: egui::FontId, color: Color32, truncate: bool) -> Response {
    if !crate::bidi::has_rtl(text) {
        let mut label = egui::Label::new(egui::RichText::new(text).font(font).color(color)).wrap();
        if truncate {
            label = label.truncate();
        }
        return ui.add(label);
    }
    let width = ui.available_width().max(1.0);
    let lines = wrapped_lines(ui, text, &font, width, truncate);
    let galleys: Vec<_> = lines.iter().map(|line| (line.rtl, ui.painter().layout_no_wrap(line.text.clone(), font.clone(), color))).collect();
    let height = galleys.iter().map(|(_, g)| g.size().y).sum();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    // Painted text only: the accessibility name keeps the logical order.
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    let mut y = rect.top();
    for (rtl, galley) in galleys {
        let x = if rtl { rect.right() - galley.size().x } else { rect.left() };
        let h = galley.size().y;
        ui.painter().galley(egui::pos2(x, y), galley, color);
        y += h;
    }
    resp
}

const ELLIPSIS: &str = "\u{2026}";

/// Wrapped right-to-left text kept between frames. Wrapping measures many candidate lines, so
/// re-wrapping every comment on every frame would make a panel of long comments slow; the
/// result depends only on the text, font, width, scale and collapsed state, so it is looked up.
#[derive(Clone, Default)]
struct WrapCache {
    lines: std::collections::HashMap<u64, std::sync::Arc<[crate::bidi::Line]>>,
}

/// How many wrapped texts the cache holds before it starts over (each costs its text's size).
const WRAP_CACHE_MAX: usize = 256;

/// A collapsed comment shows one line, so no more than this many characters are wrapped for it:
/// a long comment doesn't cost a full wrap just to show its start.
const COLLAPSED_WRAP_CHARS: usize = 1024;

/// The lines `wrapped_text` paints for a right-to-left `text`, from the cache when the same text
/// was wrapped the same way before.
fn wrapped_lines(ui: &egui::Ui, text: &str, font: &egui::FontId, width: f32, truncate: bool) -> std::sync::Arc<[crate::bidi::Line]> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (text, font, width.to_bits(), ui.ctx().pixels_per_point().to_bits(), truncate).hash(&mut hasher);
    let key = hasher.finish();
    let cache_id = egui::Id::new("pdfcraft-rtl-wrap-cache");
    if let Some(hit) = ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<WrapCache>(cache_id).lines.get(&key).cloned()) {
        return hit;
    }
    // Collapsed: only the start is wrapped (cut at a char boundary, so the slice can't fail).
    let (measured, cut) = match text.char_indices().nth(COLLAPSED_WRAP_CHARS) {
        Some((at, _)) if truncate => (text.get(..at).unwrap_or(text), true),
        _ => (text, false),
    };
    let measure = |f: &mut egui::epaint::text::FontsView<'_>, s: &str| f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x;
    let mut lines = ui.fonts_mut(|f| {
        let room = if truncate { width - measure(f, ELLIPSIS) } else { width };
        crate::bidi::wrap(measured, |s| measure(f, s) <= room)
    });
    if truncate && (lines.len() > 1 || cut) {
        lines.truncate(1);
        if let Some(line) = lines.first_mut() {
            // The ellipsis goes where the line's reading ends: the left of a right-to-left line.
            if line.rtl {
                line.text.insert_str(0, ELLIPSIS);
            } else {
                line.text.push_str(ELLIPSIS);
            }
        }
    }
    let lines: std::sync::Arc<[crate::bidi::Line]> = lines.into();
    ui.ctx().data_mut(|d| {
        let cache = &mut d.get_temp_mut_or_default::<WrapCache>(cache_id).lines;
        if cache.len() >= WRAP_CACHE_MAX {
            cache.clear();
        }
        cache.insert(key, lines.clone());
    });
    lines
}

/// Transient message at the bottom centre.
pub fn toast(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some((msg, start)) = app.toast.clone() else { return };
    let now = ctx.input(|i| i.time);
    let start = if start == 0.0 { now } else { start };
    app.toast = Some((msg.clone(), start));
    if now - start > 3.5 {
        app.toast = None;
        return;
    }
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    // Above the progress notice while a job runs.
    let card = if app.progress_notice.is_some() { ctx.data(|d| d.get_temp::<f32>(egui::Id::new(PROGRESS_HEIGHT))).unwrap_or(0.0) } else { 0.0 };
    let lift = if card > 0.0 { 28.0 + card + 10.0 } else { 28.0 };
    egui::Area::new(egui::Id::new("toast"))
        .order(egui::Order::Tooltip)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(screen.center_bottom() - vec2(0.0, lift))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(if t.dark() { Color32::from_rgb(0xEC, 0xEC, 0xEF) } else { Color32::from_rgb(0x2A, 0x2A, 0x2F) })
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    // An Area remembers its previous content width. A short notice
                    // must not force the next validation message into a narrow column.
                    // Allow natural short labels; wrap longer ones within the viewport,
                    // including the frame's 32 points and 16-point outside margins.
                    ui.set_max_width((screen.width() - 64.0).clamp(1.0, 560.0));
                    ui.add(
                        egui::Label::new(egui::RichText::new(msg).color(if t.dark() { Color32::from_rgb(0x22, 0x22, 0x26) } else { Color32::WHITE }))
                            .wrap(),
                    );
                });
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
}

/// A background job's progress, shown at the bottom centre until the job is done.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressNotice {
    /// What is happening ("Optimizing… image 3 of 40").
    pub label: String,
    /// From 0 to 1.
    pub fraction: f32,
    /// Show a Cancel button.
    pub cancellable: bool,
}

/// Where the progress notice keeps its height this frame, for placing the toast above it.
const PROGRESS_HEIGHT: &str = "progress-notice-height";

/// Draw [`PdfCraftApp::progress_notice`]: the label, a bar that eases towards the fraction with
/// a light sweeping across it, the percentage and Cancel. Returns true when Cancel was clicked.
/// With animation effects off ([`crate::motion`]) the bar steps to the fraction and has no sweep.
pub fn progress_notice(app: &PdfCraftApp, ctx: &egui::Context) -> bool {
    let Some(p) = &app.progress_notice else { return false };
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    // The same inverted card as the toast.
    let (fill, text) = if t.dark() {
        (Color32::from_rgb(0xEC, 0xEC, 0xEF), Color32::from_rgb(0x22, 0x22, 0x26))
    } else {
        (Color32::from_rgb(0x2A, 0x2A, 0x2F), Color32::WHITE)
    };
    let fraction = if p.fraction.is_finite() { p.fraction.clamp(0.0, 1.0) } else { 0.0 };
    let animated = crate::motion::animated(ctx);
    let shown = ctx.animate_value_with_time(egui::Id::new("progress-notice-bar"), fraction, if animated { 0.3 } else { 0.0 });
    let now = ctx.input(|i| i.time);
    let mut cancel = false;
    let area = egui::Area::new(egui::Id::new("progress-notice"))
        .order(egui::Order::Tooltip)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(screen.center_bottom() - vec2(0.0, 28.0))
        .show(ctx, |ui| {
            egui::Frame::NONE.fill(fill).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::symmetric(16, 12)).show(ui, |ui| {
                let width = 340.0_f32.min(screen.width() - 64.0).max(160.0);
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&p.label).color(text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(format!("{:.0}%", shown * 100.0)).font(theme::medium(12.0)).color(text.gamma_multiply(0.7)));
                    });
                });
                ui.add_space(6.0);
                let (track, resp) = ui.allocate_exact_size(vec2(width, 6.0), Sense::hover());
                // The label above names the job; the bar carries the percentage.
                resp.widget_info(|| {
                    let mut info = egui::WidgetInfo::new(egui::WidgetType::ProgressIndicator);
                    info.value = Some(f64::from(fraction) * 100.0);
                    info
                });
                let painter = ui.painter();
                painter.rect_filled(track, CornerRadius::same(3), text.gamma_multiply(0.18));
                let done = Rect::from_min_size(track.min, vec2(track.width() * shown, track.height()));
                if done.width() > 0.5 {
                    painter.rect_filled(done, CornerRadius::same(3), t.accent);
                    // A soft light sweeping along the filled part says the job is alive between
                    // updates.
                    if animated {
                        let band = 48.0;
                        let x = done.left() - band + ((now * 160.0) % f64::from(done.width() + band)) as f32;
                        let clip = painter.with_clip_rect(done);
                        for (i, a) in [0.10_f32, 0.22, 0.10].into_iter().enumerate() {
                            let r = Rect::from_min_size(egui::pos2(x + i as f32 * band / 3.0, done.top()), vec2(band / 3.0, done.height()));
                            clip.rect_filled(r, CornerRadius::ZERO, Color32::WHITE.gamma_multiply(a));
                        }
                    }
                }
                if p.cancellable {
                    ui.add_space(4.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let b = egui::Button::new(egui::RichText::new(tl!("Cancel")).font(theme::medium(12.5)).color(text)).frame(false);
                        cancel = ui.add(b).clicked();
                    });
                }
            });
        });
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(PROGRESS_HEIGHT), area.response.rect.height()));
    ctx.request_repaint_after(std::time::Duration::from_millis(33));
    cancel
}

/// The ArtCraft wordmark (Storyteller's brand, docs/brand/; not open source), sized to `height`.
pub fn artcraft_logo(ui: &mut egui::Ui, height: f32) -> Response {
    let dark = ui.visuals().dark_mode;
    let (uri, bytes): (&str, &'static [u8]) = if dark {
        ("bytes://artcraft-logo-white.svg", include_bytes!("../../../docs/brand/artcraft-logo-white.svg"))
    } else {
        ("bytes://artcraft-logo.svg", include_bytes!("../../../docs/brand/artcraft-logo.svg"))
    };
    ui.add(egui::Image::from_bytes(uri, bytes).max_height(height).alt_text("ArtCraft"))
}

/// The ArtCraft mark (brand blue, works on light and dark), `size` points square.
pub fn artcraft_mark(ui: &mut egui::Ui, size: f32) -> Response {
    ui.add(
        egui::Image::from_bytes("bytes://artcraft-mark.svg", include_bytes!("../../../docs/brand/artcraft-mark.svg"))
            .fit_to_exact_size(vec2(size, size))
            .alt_text("ArtCraft"),
    )
}

/// Buttons for every community link (`pdfcraft_engine::links`), Discord first and prominent.
/// Returns the registry command of the one clicked.
pub fn community_links(ui: &mut egui::Ui) -> Option<&'static str> {
    let mut clicked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for (i, l) in pdfcraft_engine::links::LINKS.iter().enumerate() {
            let resp = if i == 0 { icon_pill(ui, l.icon, tl!("Join our Discord"), true) } else { icon_pill(ui, l.icon, tl!(l.label), false) };
            if resp.on_hover_text(l.url).clicked() {
                clicked = Some(l.command);
            }
        }
    });
    clicked
}

/// [`icon_pill`] with a ▾ part at its end that opens a menu of related choices. Returns the
/// main button's response and the ▾'s (give it to `egui::Popup::menu`).
pub fn split_pill(ui: &mut egui::Ui, icon: &str, label: &str, more: &str) -> (Response, Response) {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(12.5);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, _) = ui.allocate_exact_size(vec2(w + 46.0 + 28.0, 30.0), Sense::hover());
    let (main_rect, more_rect) = rect.split_left_right_at_x(rect.right() - 28.0);
    let main = ui.interact(main_rect, ui.id().with(("split-pill", label)), Sense::click());
    let arrow = ui.interact(more_rect, ui.id().with(("split-pill-more", label)), Sense::click());
    main.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    arrow.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), more));
    let radius = CornerRadius::same(15);
    ui.painter().rect(rect, radius, t.card, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let left = CornerRadius { nw: 15, sw: 15, ne: 0, se: 0 };
    let right = CornerRadius { nw: 0, sw: 0, ne: 15, se: 15 };
    let main_press = theme::Press::track(ui, &main);
    let arrow_press = theme::Press::track(ui, &arrow);
    for (r, press, corners) in [(main_rect, main_press, left), (more_rect, arrow_press, right)] {
        let fill = press.fill(Color32::TRANSPARENT, t.hover, t.pressed);
        if fill.a() > 0 {
            ui.painter().rect_filled(r.shrink(1.2), corners, fill);
        }
    }
    let body = rect.translate(main_press.offset());
    let more_body = more_rect.translate(arrow_press.offset());
    ui.painter().vline(more_rect.left(), rect.y_range().shrink(7.0), Stroke::new(1.0, t.text_muted));
    crate::icons::paint(ui, Rect::from_min_size(body.min + vec2(12.0, 7.0), vec2(16.0, 16.0)), icon, 15.0, t.text);
    ui.painter().text(body.left_center() + vec2(34.0, 0.0), Align2::LEFT_CENTER, label, font, t.text);
    crate::icons::paint(ui, Rect::from_center_size(more_body.center(), vec2(14.0, 14.0)), "chevron-down", 13.0, t.text);
    (theme::hand(main).on_hover_text(label), theme::hand(arrow).on_hover_text(more))
}

/// A pill button with an icon (primary = filled accent).
pub fn icon_pill(ui: &mut egui::Ui, icon: &str, label: &str, primary: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(12.5);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 46.0, 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    let press = theme::Press::track(ui, &resp);
    let (fill, stroke, text) = if primary {
        (press.fill(t.accent, t.accent_text, t.accent.lerp_to_gamma(Color32::BLACK, 0.22)), Stroke::NONE, Color32::WHITE)
    } else {
        (press.fill(t.card, t.hover, t.pressed), Stroke::new(1.0, t.border), t.text)
    };
    ui.painter().rect(rect, CornerRadius::same(15), fill, stroke, egui::StrokeKind::Inside);
    let body = rect.translate(press.offset());
    crate::icons::paint(ui, Rect::from_min_size(body.min + vec2(12.0, 7.0), vec2(16.0, 16.0)), icon, 15.0, text);
    ui.painter().text(body.left_center() + vec2(34.0, 0.0), Align2::LEFT_CENTER, label, font, text);
    theme::hand(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Pos2;

    #[test]
    fn search_placeholder_never_overlaps_the_shortcut() {
        let previous = crate::i18n::current();
        for (code, expected) in [("en", "Ctrl+K"), ("de", "Strg+K")] {
            crate::i18n::set_current(crate::i18n::Lang::from_code(code).unwrap());
            for width in [260.0, 160.0, 90.0] {
                let ctx = egui::Context::default();
                ctx.set_os(egui::os::OperatingSystem::Nix);
                ctx.set_fonts(theme::font_definitions());
                let placeholder = "Werkzeuge und Befehle suchen";
                let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    search_box(ui, placeholder, width);
                });
                let text_shapes: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| if let egui::Shape::Text(text) = &shape.shape { Some((shape.clip_rect, text)) } else { None })
                    .collect();
                let (clip, hint) = text_shapes.iter().find(|(_, text)| text.galley.job.text == placeholder).unwrap();
                let (_, shortcut) = text_shapes.iter().find(|(_, text)| text.galley.job.text.contains('K')).unwrap();
                assert_eq!(shortcut.galley.job.text, expected);
                assert!(clip.right() <= shortcut.pos.x - 9.0, "hint must leave a gap before the shortcut at width {width}");
                assert!(hint.galley.elided, "the long original German hint must be elided at width {width}");
                assert_eq!(hint.galley.rows.len(), 1);
                output.drop_without_applying_deltas();
            }
        }
        crate::i18n::set_current(previous);
    }

    /// View ▸ Page display mixes radio buttons and checkboxes. With the theme's 6 px widget radius
    /// egui's 14 px checkbox box drew as a circle, and on the light theme an unticked box or an
    /// unselected radio button was white on white with no outline, so it couldn't be seen.
    #[test]
    fn checkboxes_and_radio_buttons_are_outlined_filled_and_distinct() {
        for kind in [theme::ThemeKind::Light, theme::ThemeKind::Dark] {
            let ctx = egui::Context::default();
            theme::apply(&ctx, kind);
            let t = Tokens::for_kind(kind);
            let side = ctx.global_style().spacing.icon_width;
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.add(checkbox(&mut false, "Show cover page in two-page view"));
                ui.add(radio(false, "Single page"));
            });
            let boxes: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Rect(r) if (r.rect.width() - side).abs() < 0.5 && (r.rect.height() - side).abs() < 0.5 => Some(r),
                    _ => None,
                })
                .collect();
            let circles: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Circle(c) if (c.radius * 2.0 - side).abs() < 1.0 => Some(c),
                    _ => None,
                })
                .collect();
            assert_eq!((boxes.len(), circles.len()), (1, 1), "{kind:?}: one checkbox box and one radio circle");
            let r = boxes[0].corner_radius;
            let radius = r.nw.max(r.ne).max(r.sw).max(r.se);
            assert!(f32::from(radius) <= side / 4.0, "{kind:?}: a {side} px box with corner radius {radius} looks like a radio button");
            for (what, fill, edge) in [("checkbox", boxes[0].fill, boxes[0].stroke), ("radio button", circles[0].fill, circles[0].stroke)] {
                assert!(fill != t.panel && fill != t.card, "{kind:?}: an unselected {what} is filled like the panel behind it");
                assert!(
                    edge.width >= 1.0 && edge.color != t.panel && edge.color != t.card,
                    "{kind:?}: an unselected {what} needs an outline, got {edge:?}"
                );
            }
            output.drop_without_applying_deltas();
        }
        // An outline the caller set (dialogs) is kept.
        let ctx = egui::Context::default();
        let own = Stroke::new(1.5, Color32::RED);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.visuals_mut().widgets.inactive.bg_stroke = own;
            ui.add(checkbox(&mut true, "Collate"));
            ui.add(radio(false, "All"));
        });
        let kept = output
            .shapes
            .iter()
            .filter(|s| match &s.shape {
                egui::Shape::Rect(r) => r.stroke == own,
                egui::Shape::Circle(c) => c.stroke == own,
                _ => false,
            })
            .count();
        assert_eq!(kept, 2, "the checkbox and the radio button keep the caller's outline");
        output.drop_without_applying_deltas();
    }

    fn toast_rect(app: &mut PdfCraftApp, ctx: &egui::Context, width: f32, now: &mut f64) -> Rect {
        // Let the Area settle after a notice/viewport change, without advancing to
        // expiry. No native rendering or renderer worker is needed for this layout.
        for _ in 0..3 {
            *now += 0.016;
            let output = ctx.run_ui(
                egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 900.0))), time: Some(*now), ..Default::default() },
                |ui| toast(app, ui.ctx()),
            );
            output.drop_without_applying_deltas();
        }
        ctx.memory(|memory| memory.area_rect(egui::Id::new("toast")).unwrap())
    }

    #[test]
    fn toast_width_adapts_to_long_notices_and_narrow_viewports() {
        let ctx = egui::Context::default();
        let mut app = PdfCraftApp::new();
        let mut now = 1.0;
        app.notify("Saved");
        let short = toast_rect(&mut app, &ctx, 1280.0, &mut now);
        assert!(short.width() < 160.0, "short notices retain their natural width: {short:?}");
        app.notify("Fill in name failed: The value entered is not valid for the field [ name ]");
        let long = toast_rect(&mut app, &ctx, 1280.0, &mut now);
        assert!(long.width() > 300.0, "a previous short Area must not squeeze validation feedback: {long:?}");
        assert!(long.height() < 85.0, "ordinary feedback must not become a seven-line column: {long:?}");
        for width in [320.0, 180.0] {
            let rect = toast_rect(&mut app, &ctx, width, &mut now);
            assert!(rect.width() <= width - 32.0 + 1.0, "frame and viewport margins must fit: {rect:?}");
            assert!(rect.left() >= 15.0 && rect.right() <= width - 15.0, "notice remains horizontally on screen: {rect:?}");
        }
        app.notify("Saved");
        let again = toast_rect(&mut app, &ctx, 1280.0, &mut now);
        assert!((again.width() - short.width()).abs() < 1.0, "long notices must not impose a permanent minimum width");
    }

    /// The text shapes `wrapped_text` paints in a 120 pt wide column at x = 0.
    fn wrapped_shapes(text: &str, truncate: bool) -> Vec<(Pos2, Rect, String)> {
        let ctx = egui::Context::default();
        ctx.set_fonts(theme::font_definitions());
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let column = Rect::from_min_size(Pos2::ZERO, vec2(120.0, 400.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
                wrapped_text(ui, text, theme::regular(13.0), Color32::WHITE, truncate);
            });
        });
        let shapes = output
            .shapes
            .iter()
            .filter_map(|shape| if let egui::Shape::Text(t) = &shape.shape { Some((t.pos, t.galley.rect, t.galley.job.text.clone())) } else { None })
            .collect();
        output.drop_without_applying_deltas();
        shapes
    }

    /// Right-to-left text is wrapped once and looked up on later frames; the cache stays bounded
    /// however many comments go by, and a collapsed comment of any length still shows one line.
    #[test]
    fn rtl_wrapping_is_cached_and_bounded() {
        let ctx = egui::Context::default();
        ctx.set_fonts(theme::font_definitions());
        let cache_len =
            |ctx: &egui::Context| ctx.data_mut(|d| d.get_temp_mut_or_default::<WrapCache>(egui::Id::new("pdfcraft-rtl-wrap-cache")).lines.len());
        let persian = "این یک نظر طولانی به زبان فارسی است که باید در چند خط شکسته شود";
        let long = persian.repeat(2_000);
        let mut first = None;
        for _ in 0..3 {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                let lines = wrapped_lines(ui, persian, &theme::regular(13.0), 120.0, false);
                assert!(lines.len() > 1);
                if let Some(prev) = &first {
                    assert!(std::sync::Arc::ptr_eq(prev, &lines), "the second frame reuses the first frame's wrap");
                }
                first = Some(lines);
                let collapsed = wrapped_lines(ui, &long, &theme::regular(13.0), 120.0, true);
                assert_eq!(collapsed.len(), 1);
                assert!(collapsed[0].text.starts_with(ELLIPSIS), "{:?}", collapsed[0].text);
            })
            .drop_without_applying_deltas();
        }
        assert_eq!(cache_len(&ctx), 2, "one entry per text and layout");
        ctx.run_ui(egui::RawInput::default(), |ui| {
            for i in 0..WRAP_CACHE_MAX + 50 {
                wrapped_lines(ui, &format!("{persian} {i}"), &theme::regular(13.0), 120.0, false);
            }
        })
        .drop_without_applying_deltas();
        assert!(cache_len(&ctx) <= WRAP_CACHE_MAX, "bounded: {}", cache_len(&ctx));
    }

    #[test]
    fn rtl_comment_wraps_against_the_right_edge() {
        let persian = "این یک نظر طولانی به زبان فارسی است که باید در چند خط شکسته شود";
        let lines = wrapped_shapes(persian, false);
        assert!(lines.len() > 1, "{lines:?}");
        let right = lines.iter().map(|(pos, rect, _)| pos.x + rect.width()).fold(0.0, f32::max);
        for (pos, rect, text) in &lines {
            assert!(pos.x >= -0.5 && (pos.x + rect.width() - right).abs() < 0.5, "{text:?} at {pos:?} must be right-aligned");
        }
        assert!(right <= 120.5, "lines stay inside the column: {right}");
        // The first words read on the first line, at the top.
        let top = lines.iter().min_by(|a, b| a.0.y.total_cmp(&b.0.y)).unwrap();
        assert!(top.2.contains("این"), "{top:?}");
        // Collapsed: one line, its ellipsis on the left where right-to-left reading ends.
        let collapsed = wrapped_shapes(persian, true);
        assert_eq!(collapsed.len(), 1);
        assert!(collapsed[0].2.starts_with('\u{2026}'), "{collapsed:?}");
        // Text without right-to-left script is a plain label, as before.
        let latin = wrapped_shapes("A short note", false);
        assert_eq!(latin.len(), 1);
        assert_eq!(latin[0].2, "A short note");
        assert!(latin[0].0.x.abs() < 0.5);
    }

    #[test]
    fn line_field_is_at_least_thirty_points_tall() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        let mut height = 0.0;
        ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0))), ..Default::default() }, |ui| {
            begin_dialog(ui);
            height = ui.add(line(&mut text)).rect.height();
        })
        .drop_without_applying_deltas();
        assert!(height >= FIELD_MIN_HEIGHT, "field height {height}");
    }

    #[test]
    fn fit_line_keeps_short_text_and_ellipsizes_the_rest() {
        let width = |s: &str| s.chars().count() as f32;
        assert_eq!(fit_line("localhost", 20.0, width), "localhost");
        let long = "Apple Worldwide Developer Relations Certification Authority";
        let fitted = fit_line(long, 24.0, width);
        assert!(fitted.ends_with('\u{2026}'), "{fitted}");
        assert!(fitted.chars().count() <= 24, "{fitted}");
        assert!(long.starts_with(fitted.trim_end_matches('\u{2026}')));
        assert_eq!(fit_line(long, 0.0, width), "");
        assert_eq!(fit_line("ééé", 2.0, width), "é\u{2026}");
    }
}
