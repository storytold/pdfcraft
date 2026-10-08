//! Small custom widgets built on the design tokens.

use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Stroke, vec2};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, icons};

/// A mode-bar tab: text with an underline when active.
pub fn mode_tab(ui: &mut egui::Ui, label: &str, active: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = if active { theme::semibold(13.5) } else { theme::medium(13.5) };
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 22.0, 48.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), active, label));
    if resp.hovered() && !active {
        ui.painter().rect_filled(rect.shrink2(vec2(2.0, 9.0)), CornerRadius::same(6), t.hover);
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, if active { t.text } else { t.text_muted });
    if active {
        let r = Rect::from_min_max(rect.left_bottom() + vec2(11.0, -3.0), rect.right_bottom() - vec2(11.0, 0.0));
        ui.painter().rect_filled(r, CornerRadius::same(1), t.text);
    }
    resp
}

/// Rounded pill button; `primary` fills with the accent.
pub fn pill_button(ui: &mut egui::Ui, label: &str, primary: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(12.5);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 26.0, 28.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    let (fill, stroke, text) = if primary {
        (if resp.hovered() { t.accent_text } else { t.accent }, Stroke::NONE, Color32::WHITE)
    } else {
        (if resp.hovered() { t.hover } else { t.card }, Stroke::new(1.2, t.text_muted), t.text)
    };
    ui.painter().rect(rect, CornerRadius::same(14), fill, stroke, egui::StrokeKind::Inside);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, text);
    resp
}

/// Icon + label, transparent until hovered.
pub fn ghost_button(ui: &mut egui::Ui, icon: &str, label: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(13.0);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 38.0, 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
    }
    icons::paint(ui, Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(18.0, 18.0)), icon, 17.0, t.icon);
    ui.painter().text(rect.left_center() + vec2(30.0, 0.0), Align2::LEFT_CENTER, label, font, t.text);
    resp
}

/// A search-field lookalike that opens the command palette.
pub fn search_box(ui: &mut egui::Ui, placeholder: &str, width: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), placeholder));
    let fill = if resp.hovered() { t.hover } else { t.field };
    ui.painter().rect(rect, CornerRadius::same(16), fill, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    icons::paint(ui, Rect::from_min_size(rect.min + vec2(10.0, 8.0), vec2(16.0, 16.0)), "search", 15.0, t.text_muted);
    ui.painter().text(rect.left_center() + vec2(34.0, 0.0), Align2::LEFT_CENTER, placeholder, theme::regular(13.0), t.text_faint);
    ui.painter().text(rect.right_center() - vec2(12.0, 0.0), Align2::RIGHT_CENTER, "⌘K", theme::regular(11.5), t.text_faint);
    resp.on_hover_cursor(egui::CursorIcon::Text)
}

pub fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> Response {
    ui.add(egui::Button::new(label).shortcut_text(shortcut))
}

pub fn section_title(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(10.0);
    // Section titles across every panel go through here, so one translation point covers them.
    ui.label(egui::RichText::new(tl!(text).to_uppercase()).font(theme::semibold(10.5)).color(t.text_faint).extra_letter_spacing(0.6));
    ui.add_space(2.0);
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
    egui::Area::new(egui::Id::new("toast"))
        .order(egui::Order::Tooltip)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(screen.center_bottom() - vec2(0.0, 28.0))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(if t.dark() { Color32::from_rgb(0xEC, 0xEC, 0xEF) } else { Color32::from_rgb(0x2A, 0x2A, 0x2F) })
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(msg).color(if t.dark() { Color32::from_rgb(0x22, 0x22, 0x26) } else { Color32::WHITE }));
                });
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
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

/// A pill button with an icon (primary = filled accent).
pub fn icon_pill(ui: &mut egui::Ui, icon: &str, label: &str, primary: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(12.5);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 46.0, 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    let (fill, stroke, text) = if primary {
        (if resp.hovered() { t.accent_text } else { t.accent }, Stroke::NONE, Color32::WHITE)
    } else {
        (if resp.hovered() { t.hover } else { t.card }, Stroke::new(1.2, t.text_muted), t.text)
    };
    ui.painter().rect(rect, CornerRadius::same(15), fill, stroke, egui::StrokeKind::Inside);
    crate::icons::paint(ui, Rect::from_min_size(rect.min + vec2(12.0, 7.0), vec2(16.0, 16.0)), icon, 15.0, text);
    ui.painter().text(rect.left_center() + vec2(34.0, 0.0), Align2::LEFT_CENTER, label, font, text);
    resp
}
