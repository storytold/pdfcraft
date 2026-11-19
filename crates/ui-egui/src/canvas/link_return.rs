//! The way back from a link: after a link takes the reader to another place in the document,
//! "Back to page N" shows at the pages' bottom left. A click on it returns to the place and zoom
//! the link was followed from. It fades out after a few seconds and shows again while the
//! pointer is near that corner, until the reader goes back (with it, or Previous / Next view) or
//! follows another link.

use egui::{Pos2, Rect, Vec2};

use super::{DocInfo, DocView, Fit, SIDE, vec2};
use crate::icons;
use crate::theme::{self, Tokens};

/// How long the way back shows after the jump, in seconds.
const SHOWN_FOR: f64 = 3.0;
/// How long it takes to fade in or out, in seconds, when the window animates.
const FADE: f32 = 0.25;
/// How close the pointer comes to it, in points, to bring it back.
const NEAR: f32 = 80.0;
/// Its distance from the pages' bottom-left corner, in points.
const INSET: f32 = 16.0;
/// Its size before it has been drawn once.
const SIZE_GUESS: Vec2 = Vec2::new(170.0, 40.0);

/// Where a link was followed from: the page, a point on it and where that point was in the
/// window, with the zoom, as [`DocView::return_from_link`] restores them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LinkReturn {
    pub(crate) page: usize,
    /// The point of the page nearest the window's top-left (beside the quick-action bar's
    /// gutter), in user space: `[left, top]`, `None` before pages were laid out.
    place: [Option<f32>; 2],
    /// Where that point was, from the window's top-left. Above the first page it is in the
    /// margin, which a point at the window's very edge would lose.
    rel: Vec2,
    zoom: f32,
    fit: Fit,
    /// egui time of the jump.
    since: f64,
}

impl DocView {
    /// Where the reader is now, to come back to after following a link from here at `now`.
    pub(super) fn link_origin(&self, info: &DocInfo, now: f64) -> LinkReturn {
        let area = self.viewport_screen;
        let corner = area.min + vec2(SIDE, 0.0);
        let at = self
            .screen_xforms
            .iter()
            .filter(|(_, xf)| area.is_positive() && xf.rect.is_positive())
            .min_by(|a, b| a.1.rect.distance_sq_to_pos(corner).total_cmp(&b.1.rect.distance_sq_to_pos(corner)).then(a.0.cmp(&b.0)))
            .and_then(|(page, xf)| {
                let p = info.pages.get(*page)?;
                let point = corner.clamp(xf.rect.min, xf.rect.max);
                let (x, y) = xf.screen_to_view(point);
                // `max` drops a NaN size, so the bounds stay ordered and `clamp` can't panic.
                let (x, y) = (x.clamp(0.0, p.width.max(0.0)), y.clamp(0.0, p.height.max(0.0)));
                Some((*page, p.xyz_for_view(Some(x), Some(y)), point - area.min))
            });
        // Before the first frame has laid pages out, the current page's top.
        let (page, place, rel) = at.unwrap_or((self.current, [None, None], vec2(SIDE, 0.0)));
        LinkReturn { page, place, rel, zoom: self.zoom, fit: self.fit, since: now }
    }

    /// Back to where the last link was followed from, at the zoom it had. When that page is the
    /// last in the view history it goes back through it, as Previous view does, so Next view
    /// returns to where the link went. `false` when there is no way back.
    pub(crate) fn return_from_link(&mut self, info: &DocInfo) -> bool {
        let Some(from) = self.link_return.take() else { return false };
        if from.page != self.current && self.back.last() == Some(&from.page) {
            self.view_history(false);
        } else {
            self.go_to_page(from.page);
        }
        self.fit = from.fit;
        if from.fit == Fit::None {
            self.zoom = from.zoom;
        }
        self.zoom_anchor = None;
        self.goto_point = None;
        if let Some(p) = info.pages.get(self.current) {
            let [fx, fy] = p.dest_fraction(from.place[0], from.place[1], self.rotation);
            if fx.is_some() || fy.is_some() {
                self.goto_point = Some((self.current, fx, fy, from.rel));
            }
        }
        true
    }
}

/// Draw the way back at the pages' bottom left while it shows: for [`SHOWN_FOR`] after the jump,
/// then while `pointer` (the pages' pointer) is near it or over it. `enabled` is false while
/// something covers the pages, which hides it. Not shown in the page grid or full screen.
pub(super) fn overlay(ui: &egui::Ui, view: &mut DocView, info: &DocInfo, pointer: Option<Pos2>, now: f64, animates: bool, enabled: bool) {
    let id = area_id(view);
    ui.ctx().data_mut(|d| d.remove::<Rect>(id.with("drawn")));
    let Some(from) = view.link_return else { return };
    if view.organize || view.presentation.is_some() {
        return;
    }
    let ctx = ui.ctx();
    let viewport = view.viewport_screen;
    let corner = viewport.left_bottom() + vec2(INSET, -INSET);
    let size = egui::AreaState::load(ctx, id).and_then(|a| a.size).unwrap_or(SIZE_GUESS);
    let button = Rect::from_min_size(corner - vec2(0.0, size.y), size);
    // The pages' pointer is `None` over the button itself, which is drawn over them.
    let over = ctx.rect_contains_pointer(egui::LayerId::new(egui::Order::Foreground, id), button);
    let near = over || pointer.is_some_and(|p| button.expand(NEAR).contains(p));
    let left = SHOWN_FOR - (now - from.since);
    if left > 0.0 {
        // Fade out on time, without waiting for input.
        ctx.request_repaint_after_secs(left as f32);
    }
    let shown = enabled && (left > 0.0 || near);
    let opacity = ctx.animate_bool_with_time(id.with("shown"), shown, if animates { FADE } else { 0.0 });
    // Faded out it is not drawn at all, so it takes no clicks from the page under it.
    if opacity <= 0.0 {
        return;
    }
    let label = info.pages.get(from.page).map(|p| p.label.clone()).filter(|l| !l.is_empty()).unwrap_or_else(|| (from.page + 1).to_string());
    let text = crate::i18n::fmt(tl!("Back to page {p}"), &[("p", &label)]);
    let t = Tokens::get(ctx);
    let shown = egui::Area::new(id).order(egui::Order::Foreground).pivot(egui::Align2::LEFT_BOTTOM).fixed_pos(corner).show(ctx, |ui| {
        ui.multiply_opacity(opacity);
        // The accent fill stands out from the pages and the pasteboard in either theme, and
        // the shadow lifts it off a page it sits over.
        let shadow = egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: egui::Color32::from_black_alpha(if t.dark() { 120 } else { 40 }) };
        egui::Frame::new()
            .corner_radius(10)
            .shadow(shadow)
            .show(ui, |ui| {
                ui.spacing_mut().button_padding = vec2(14.0, 8.0);
                let content = egui::RichText::new(text).font(theme::medium(15.0)).color(egui::Color32::WHITE);
                let b = egui::Button::image_and_text(icons::image("chevron-left", 18.0, egui::Color32::WHITE), content)
                    .fill(t.accent)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(10)
                    .min_size(vec2(0.0, SIZE_GUESS.y));
                ui.add(b).on_hover_text(tl!("Go back to where you followed the link")).clicked()
            })
            .inner
    });
    ctx.data_mut(|d| d.insert_temp(id.with("drawn"), shown.response.rect));
    if shown.inner && enabled {
        view.return_from_link(info);
    }
}

/// The way back's area, per view (two panes of split view each have their own).
fn area_id(view: &DocView) -> egui::Id {
    egui::Id::unique(("link-return", view.view_key()))
}

/// Where the way back was drawn this frame, if it was: the page size hint, which shares its
/// corner, goes above it.
pub(super) fn drawn(ctx: &egui::Context, view: &DocView) -> Option<Rect> {
    ctx.data(|d| d.get_temp(area_id(view).with("drawn")))
}
