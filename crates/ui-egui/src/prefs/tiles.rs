//! Picture tiles for Preferences: a row of choices, each a small drawing above its name, that
//! wraps onto more rows when the names are long. The drawings are painted shapes or Lucide icons,
//! never image assets, so they follow the theme and need no attribution.

use egui::{Align, Color32, CornerRadius, Layout, Painter, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::theme::{self, Tokens};

/// The height of a tile's drawing.
const PICTURE_H: f32 = 58.0;
/// The space around a tile's drawing and name.
const PAD: f32 = 8.0;
/// The usual least width of a tile, and the widest a long word makes it.
const MIN_W: f32 = 112.0;
const MAX_W: f32 = 196.0;
/// The narrowest a tile gets to keep its row on one line.
const SHRUNK_W: f32 = 88.0;
/// The size of a tile's name.
const LABEL_SIZE: f32 = 12.5;

/// What a tile shows above its name.
#[derive(Clone, Copy)]
pub(crate) enum Picture {
    /// A Lucide icon from `assets/icons/`.
    Icon(&'static str),
    /// A page display: two pages side by side or one, scrolling on or not.
    Pages { two_up: bool, continuous: bool },
    /// A zoom: the whole width, the whole page, or a size of your own.
    Zoom(crate::canvas::Fit),
    /// A window with its tools panel on the left or the right.
    ToolsSide { right: bool },
}

/// One choice.
pub(crate) struct Tile<'a, V> {
    pub value: V,
    /// The translated name, also the tile's accessible label.
    pub label: &'a str,
    pub picture: Picture,
}

/// The colours a drawing uses: the selected tile draws in the accent.
struct Ink {
    paper: Color32,
    line: Color32,
    frame: Color32,
    accent: Color32,
    soft: Color32,
}

/// Draw `tiles` as radio choices of `current`, wrapping in the language's reading direction.
/// Returns true when a click chose a different value.
pub(crate) fn choose<V: PartialEq + Copy>(ui: &mut egui::Ui, current: &mut V, tiles: &[Tile<'_, V>]) -> bool {
    let t = Tokens::get(ui.ctx());
    let font = theme::regular(LABEL_SIZE);
    // Every tile in the row gets the same size: as wide as the longest single word needs, and as
    // tall as the name with the most lines.
    // Each tile is as wide as its longest word needs, and at least as wide as the others; that
    // least width shrinks when it puts the whole row on one line.
    let gap = 8.0;
    let needs: Vec<f32> = tiles
        .iter()
        .map(|tile| {
            let word = tile.label.split_whitespace().map(|w| crate::bidi::layout_ui(ui, w, font.clone(), t.text).size().x).fold(0.0, f32::max);
            (word + 2.0 * PAD).min(MAX_W)
        })
        .collect();
    let row_width = |base: f32| needs.iter().map(|n| n.max(base)).sum::<f32>() + gap * needs.len().saturating_sub(1) as f32;
    let available = ui.available_width();
    let mut base = MIN_W;
    while base > SHRUNK_W && row_width(base) > available {
        base -= 4.0;
    }
    let widths: Vec<f32> = needs.iter().map(|n| n.max(base)).collect();
    let galleys: Vec<_> = tiles.iter().zip(&widths).map(|(tile, w)| label_galley(ui, tile.label, w - 2.0 * PAD, font.clone(), t.text)).collect();
    // Every tile in the row is as tall as the name with the most lines.
    let label_h = galleys.iter().map(|g| g.size().y).fold(0.0, f32::max);
    let height = PAD + PICTURE_H + 6.0 + label_h + PAD;
    let layout = if crate::i18n::current().rtl() { Layout::right_to_left(Align::TOP) } else { Layout::left_to_right(Align::TOP) };
    let mut changed = false;
    ui.with_layout(layout.with_main_wrap(true), |ui| {
        ui.spacing_mut().item_spacing = vec2(gap, gap);
        for ((tile, galley), width) in tiles.iter().zip(galleys).zip(widths) {
            let selected = *current == tile.value;
            let (rect, mut resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, ui.is_enabled(), selected, tile.label));
            if resp.clicked() && !selected {
                *current = tile.value;
                changed = true;
                resp.mark_changed();
            }
            if ui.is_rect_visible(rect) {
                let painter = ui.painter();
                let (fill, stroke) = if selected {
                    (t.accent_soft, Stroke::new(2.0, t.accent))
                } else if resp.hovered() {
                    (t.hover, Stroke::new(1.0, t.text_muted))
                } else {
                    (t.card, Stroke::new(1.0, t.border))
                };
                painter.rect(rect, CornerRadius::same(8), fill, stroke, StrokeKind::Inside);
                if resp.has_focus() {
                    painter.rect_stroke(rect.expand(2.0), CornerRadius::same(10), Stroke::new(1.5, t.accent), StrokeKind::Outside);
                }
                let picture = Rect::from_min_size(rect.min + vec2(PAD, PAD), vec2(rect.width() - 2.0 * PAD, PICTURE_H));
                let ink = Ink {
                    paper: Color32::WHITE,
                    line: Color32::from_gray(0x9a),
                    frame: if selected { t.accent } else { t.text_muted },
                    accent: t.accent,
                    soft: if selected { t.accent.gamma_multiply(0.35) } else { t.border },
                };
                draw(ui, painter, picture, tile.picture, &ink, if selected { t.accent_text } else { t.icon });
                let color = if selected { t.accent_text } else { t.text };
                painter.galley(pos2(rect.center().x, picture.bottom() + 6.0), galley, color);
            }
            theme::hand(resp);
        }
    });
    changed
}

/// A tile's name, wrapped to the tile and centred, in the interface language's direction.
fn label_galley(ui: &egui::Ui, text: &str, wrap: f32, font: egui::FontId, color: Color32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), font, color, wrap.max(1.0));
    job.halign = Align::Center;
    job.direction = crate::bidi::ui_direction();
    ui.painter().layout_job(job)
}

fn draw(ui: &egui::Ui, painter: &Painter, area: Rect, picture: Picture, ink: &Ink, icon_tint: Color32) {
    match picture {
        Picture::Icon(name) => crate::icons::paint(ui, area, name, 28.0, icon_tint),
        Picture::Pages { two_up, continuous } => pages(painter, area, two_up, continuous, ink),
        Picture::Zoom(fit) => zoom(ui, painter, area, fit, ink, icon_tint),
        Picture::ToolsSide { right } => tools_side(painter, area, right, ink),
    }
}

/// A page: white, outlined, with a heading and lines of text.
fn page(painter: &Painter, r: Rect, ink: &Ink) {
    painter.rect(r, CornerRadius::same(1), ink.paper, Stroke::new(1.0, ink.frame), StrokeKind::Inside);
    let inset = (r.width() * 0.18).max(2.0);
    let rows = ((r.height() - 2.0 * inset) / 4.0).floor().clamp(0.0, 8.0) as usize;
    for i in 0..rows {
        let w = if i == 0 {
            0.5
        } else if i % 4 == 3 {
            0.4
        } else {
            1.0
        };
        let y = r.top() + inset + i as f32 * 4.0;
        let line = Rect::from_min_size(pos2(r.left() + inset, y), vec2((r.width() - 2.0 * inset) * w, 1.5));
        painter.rect_filled(line, 0.5, if i == 0 { ink.frame } else { ink.line });
    }
}

/// The view, a rounded frame the page display or zoom is drawn inside.
fn viewport(area: Rect) -> Rect {
    Rect::from_center_size(area.center(), vec2((area.height() * 1.45).min(area.width()), area.height()))
}

fn pages(painter: &Painter, area: Rect, two_up: bool, continuous: bool, ink: &Ink) {
    let view = viewport(area);
    painter.rect(view, CornerRadius::same(4), ink.soft.gamma_multiply(0.35), Stroke::new(1.0, ink.soft), StrokeKind::Inside);
    let clip = painter.with_clip_rect(view.shrink(1.0));
    let h = view.height() - 12.0;
    let w = h * 0.72;
    let gap = 4.0;
    let count = if two_up { 2.0 } else { 1.0 };
    let row_w = count * w + (count - 1.0) * gap;
    let left = view.center().x - row_w / 2.0;
    // Scrolling on: the next row of pages shows below, cut by the view, and a scroll bar.
    let top = if continuous { view.top() + 4.0 } else { view.top() + 6.0 };
    let rows = if continuous { 2 } else { 1 };
    for row in 0..rows {
        for col in 0..(count as usize) {
            let min = pos2(left + col as f32 * (w + gap), top + row as f32 * (h + gap));
            page(&clip, Rect::from_min_size(min, vec2(w, h)), ink);
        }
    }
    if continuous {
        let bar = Rect::from_min_size(pos2(view.right() - 5.0, view.top() + 5.0), vec2(2.5, view.height() * 0.4));
        painter.rect_filled(bar, 1.0, ink.accent);
    }
}

fn zoom(ui: &egui::Ui, painter: &Painter, area: Rect, fit: crate::canvas::Fit, ink: &Ink, tint: Color32) {
    use crate::canvas::Fit;
    let view = viewport(area);
    painter.rect(view, CornerRadius::same(4), ink.soft.gamma_multiply(0.35), Stroke::new(1.0, ink.soft), StrokeKind::Inside);
    let clip = painter.with_clip_rect(view.shrink(1.0));
    let arrow = Stroke::new(1.5, ink.accent);
    match fit {
        Fit::Width => {
            // The page fills the width and runs past the bottom.
            let r = Rect::from_min_max(pos2(view.left() + 6.0, view.top() + 6.0), pos2(view.right() - 6.0, view.bottom() + 30.0));
            page(&clip, r, ink);
            let y = view.top() + view.height() * 0.62;
            double_arrow(painter, pos2(view.left() + 9.0, y), pos2(view.right() - 9.0, y), arrow);
        }
        Fit::Page | Fit::Height => {
            // The whole page, top to bottom.
            let h = view.height() - 10.0;
            let r = Rect::from_center_size(view.center(), vec2(h * 0.72, h));
            page(&clip, r, ink);
            let x = r.right() + 7.0;
            double_arrow(painter, pos2(x, r.top() + 1.0), pos2(x, r.bottom() - 1.0), arrow);
        }
        Fit::None => {
            // A page at a size of your own, and a magnifier.
            let h = view.height() * 0.6;
            let r = Rect::from_center_size(view.center() - vec2(8.0, 0.0), vec2(h * 0.72, h));
            page(&clip, r, ink);
            let lens = Rect::from_center_size(pos2(view.right() - 16.0, view.bottom() - 14.0), vec2(20.0, 20.0));
            crate::icons::paint(ui, lens, "zoom-in", 18.0, tint);
        }
    }
}

/// A line with an arrow head at each end.
fn double_arrow(painter: &Painter, a: egui::Pos2, b: egui::Pos2, stroke: Stroke) {
    painter.line_segment([a, b], stroke);
    let along = (b - a).normalized();
    let across = along.rot90();
    for (tip, dir) in [(a, along), (b, -along)] {
        painter.line_segment([tip, tip + dir * 4.0 + across * 3.0], stroke);
        painter.line_segment([tip, tip + dir * 4.0 - across * 3.0], stroke);
    }
}

fn tools_side(painter: &Painter, area: Rect, right: bool, ink: &Ink) {
    let view = viewport(area);
    painter.rect(view, CornerRadius::same(4), ink.soft.gamma_multiply(0.35), Stroke::new(1.0, ink.frame), StrokeKind::Inside);
    // The title bar.
    let bar = Rect::from_min_size(view.min, vec2(view.width(), 8.0));
    painter.rect_filled(bar.shrink(1.0), CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 }, ink.soft);
    // The tools panel, in the accent, on its side.
    let panel_w = (view.width() * 0.3).round();
    let body = Rect::from_min_max(pos2(view.left(), bar.bottom()), view.max).shrink(1.0);
    let panel = if right {
        Rect::from_min_max(pos2(body.right() - panel_w, body.top()), body.max)
    } else {
        Rect::from_min_max(body.min, pos2(body.left() + panel_w, body.bottom()))
    };
    painter.rect_filled(panel, 2.0, ink.accent.gamma_multiply(0.3));
    for i in 0..4 {
        let row = Rect::from_min_size(panel.min + vec2(4.0, 5.0 + i as f32 * 7.0), vec2(panel.width() - 8.0, 3.0));
        painter.rect_filled(row, 1.0, ink.accent);
    }
    // The page, in what is left.
    let rest = if right {
        Rect::from_min_max(body.min, pos2(panel.left(), body.bottom()))
    } else {
        Rect::from_min_max(pos2(panel.right(), body.top()), body.max)
    };
    let h = rest.height() - 8.0;
    page(painter, Rect::from_center_size(rest.center(), vec2(h * 0.72, h)), ink);
}
