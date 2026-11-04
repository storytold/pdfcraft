//! Printed check boxes and the toggle-field hover offer, on the shared page router.

use super::{commit, fillable, widget_rect};
use crate::canvas::{DocView, PageToolContext, PageXform};
use crate::page_cursor::Layer;
use crate::page_router::{Hit, Owner};
use crate::theme::Tokens;
use egui::vec2;
use pdfcraft_engine::{Edit, FieldValue, FillMark, FormField, FormFieldKind, NewAnnotation, Shape, Style, field_flags};
use pdfcraft_render::DocInfo;

/// Detect once per page and edit generation, only when the pointer reaches the page.
pub(crate) fn cache_printed_boxes(view: &mut DocView, generation: u64, page: usize, detect: &dyn Fn() -> Vec<[f64; 4]>) {
    if view.forms.flat_gen != generation {
        view.forms.flat.clear();
        view.forms.flat_gen = generation;
    }
    view.forms.flat.entry(page).or_insert_with(detect);
}

/// The printed square under a point; actual form widgets keep their own clicks.
pub(crate) fn printed_hit(
    xf: &PageXform,
    info: &DocInfo,
    page: usize,
    form: &[FormField],
    view: &DocView,
    p: egui::Pos2,
    allowed: bool,
) -> Option<Hit> {
    if !xf.rect.contains(p)
        || form.iter().flat_map(|f| &f.widgets).any(|w| w.page == Some(page) && !w.hidden && widget_rect(xf, info, page, w.rect).contains(p))
    {
        return None;
    }
    let squares = view.forms.flat.get(&page)?;
    let index = squares.iter().position(|s| square_rect(xf, info, page, *s).expand(2.0 * xf.zoom()).contains(p))?;
    let icon = if allowed { egui::CursorIcon::PointingHand } else { egui::CursorIcon::NotAllowed };
    Some(Hit::new(Owner::PrintedSquare { index }, Layer::Placed, icon))
}

fn square_rect(xf: &PageXform, info: &DocInfo, page: usize, square: [f64; 4]) -> egui::Rect {
    xf.user_rect(info, page, square.map(|v| v as f32))
}

/// Where a synthetic printed square is on screen (tests and automation).
pub fn square_screen_rect(view: &DocView, info: &DocInfo, page: usize, square: [f64; 4]) -> Option<egui::Rect> {
    Some(square_rect(&view.page_xform(page)?, info, page, square))
}

/// Only a Fill & Sign check on this square can be removed by its next click.
fn square_mark(info: &DocInfo, page: usize, square: [f64; 4]) -> Option<usize> {
    info.annotations
        .iter()
        .filter(|a| a.page == page && a.stamp.as_deref() == Some(FillMark::Check.name()))
        .find(|a| {
            let r = a.rect.map(f64::from);
            r[0] >= square[0] - 3.0 && r[1] >= square[1] - 3.0 && r[2] <= square[2] + 3.0 && r[3] <= square[3] + 3.0
        })
        .map(|a| a.index)
}

/// A printed square acts only on the press captured for it, released over the same square.
pub(crate) fn printed_page_input(cx: PageToolContext<'_>, form: &[FormField], allowed: bool, author: &str, view: &mut DocView) -> bool {
    let PageToolContext { ui, resp, xf, ptr, page, info, .. } = cx;
    let captured = view.router.owner(page);
    if captured.is_some_and(|o| !matches!(o, Owner::PrintedSquare { .. })) && ptr.any_down {
        return false;
    }
    let hovered = ptr.pos.and_then(|p| printed_hit(xf, info, page, form, view, p, allowed));
    let Some(Owner::PrintedSquare { index }) = captured.or_else(|| hovered.as_ref().map(|h| h.owner)) else { return false };
    let Some(square) = view.forms.flat.get(&page).and_then(|s| s.get(index)).copied() else { return false };
    let over = hovered.as_ref().is_some_and(|h| h.owner == Owner::PrintedSquare { index });
    if !resp.clicked() {
        if over
            && allowed
            && !ptr.any_down
            && square_mark(info, page, square).is_none()
            && let Some(p) = ptr.pos
        {
            egui::Area::new(egui::Id::unique(("form-check-cursor", view.id.0)))
                .order(egui::Order::Tooltip)
                .interactable(false)
                .fixed_pos(p + vec2(14.0, 18.0))
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        let (rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::hover());
                        crate::icons::paint(ui, rect, "check", 16.0, Tokens::get(ui.ctx()).accent);
                    });
                });
        }
        return captured.is_some() || over;
    }
    if !matches!(captured, Some(Owner::PrintedSquare { .. })) || !over {
        return false;
    }
    if !allowed {
        view.forms.notice = Some(super::FormNotice::Security);
        return true;
    }
    let edit = if let Some(index) = square_mark(info, page, square) {
        Edit::DeleteAnnotation { page, index }
    } else {
        let shape = Shape::Mark { rect: square, mark: FillMark::Check };
        let style = Style::default_for(&shape);
        Edit::AddAnnotation(NewAnnotation { page, shape, style, contents: String::new(), author: author.into() })
    };
    commit(view, form);
    if view.pending_edit.is_some() {
        view.forms.queued = Some(edit);
    } else {
        view.pending_edit = Some(edit);
    }
    true
}

/// A field draft goes first; a refused draft reopens its editor and cancels the pending mark.
pub(crate) fn finish_printed_edit(view: &mut DocView) {
    if view.pending_edit.is_none() && view.forms.committed.is_none() {
        let edit = view.forms.queued.take();
        if view.forms.focus.is_none() {
            view.pending_edit = edit;
        }
    }
}

/// The offer must leave other widgets uncovered: a touch has no hover frame to move it
/// away before its press lands (a radio button beside the one just selected).
fn offer_position(ctx: &egui::Context, view: &DocView, info: &DocInfo, form: &[FormField], field: egui::Rect) -> Option<egui::Pos2> {
    let size = view.forms.offer_rect.map_or(vec2(144.0, 72.0), |r| r.size().max(vec2(144.0, 72.0)));
    let fields: Vec<egui::Rect> = form
        .iter()
        .flat_map(|f| &f.widgets)
        .filter(|w| !w.hidden)
        .filter_map(|w| {
            let page = w.page?;
            Some(widget_rect(&view.page_xform(page)?, info, page, w.rect))
        })
        .collect();
    [
        field.right_top() + vec2(6.0, -size.y - 6.0),
        field.right_bottom() + vec2(6.0, 6.0),
        field.left_top() - size - vec2(6.0, 6.0),
        field.left_bottom() + vec2(-size.x - 6.0, 6.0),
    ]
    .into_iter()
    .find(|p| {
        let popup = egui::Rect::from_min_size(*p, size);
        ctx.content_rect().contains_rect(popup) && fields.iter().all(|r| !r.expand(2.0).intersects(popup))
    })
}

/// The toggle-field popup; `Some(None)` means visible but unclicked.
pub(super) fn widget_offer(
    ctx: &egui::Context,
    view: &mut DocView,
    info: &DocInfo,
    form: &[FormField],
    pointer: Option<egui::Pos2>,
) -> Option<Option<Edit>> {
    let can_fill = view.forms.offer_allowed;
    // The widget under the cursor, when it is a toggle field we can fill.
    let hovered: Option<(String, usize)> = pointer.and_then(|p| {
        form.iter().find_map(|f| {
            if !matches!(f.kind, FormFieldKind::CheckBox | FormFieldKind::Radio) || !fillable(f) {
                return None;
            }
            f.widgets.iter().enumerate().find_map(|(wi, w)| {
                if w.hidden || w.read_only {
                    return None;
                }
                let page = w.page?;
                let xf = view.page_xform(page)?;
                widget_rect(&xf, info, page, w.rect).contains(p).then(|| (f.name.clone(), wi))
            })
        })
    });
    if let Some(h) = hovered {
        view.forms.offer = Some(h);
    } else {
        // Keep the popup alive over the field, the popup, and the gap between them.
        let over_offer = pointer.is_some_and(|p| view.forms.offer_rect.is_some_and(|r| r.contains(p)));
        let over_field = view.forms.offer.clone().and_then(|(ref name, wi)| {
            let f = form.iter().find(|f| &f.name == name)?;
            let w = f.widgets.get(wi)?;
            let page = w.page?;
            let xf = view.page_xform(page)?;
            let field = widget_rect(&xf, info, page, w.rect);
            let corridor = view.forms.offer_rect.map_or(field, |popup| field.union(popup));
            pointer.map(|p| corridor.contains(p))
        });
        if !over_offer && !over_field.unwrap_or(false) {
            view.forms.offer = None;
            view.forms.offer_rect = None;
            return None;
        }
        if !can_fill {
            view.forms.offer = None;
            view.forms.offer_rect = None;
            return None;
        }
    }
    if !can_fill {
        return None;
    }
    let (name, wi) = view.forms.offer.clone()?;
    let f = form.iter().find(|f| f.name == name)?;
    if !matches!(f.kind, FormFieldKind::CheckBox | FormFieldKind::Radio) || !fillable(f) {
        view.forms.offer = None;
        return None;
    }
    let w = f.widgets.get(wi)?;
    if w.hidden || w.read_only {
        view.forms.offer = None;
        return None;
    }
    let page = w.page?;
    let xf = view.page_xform(page)?;
    let rect = widget_rect(&xf, info, page, w.rect);
    let (label, edit, enabled) = match f.kind {
        FormFieldKind::CheckBox if f.value.is_empty() => ("Check", Some(FieldValue::Check(true)), true),
        FormFieldKind::CheckBox => ("Uncheck", Some(FieldValue::Check(false)), true),
        FormFieldKind::Radio => {
            let on = w.on_state.clone();
            if f.value.first() == on.as_ref() {
                if f.has(field_flags::NO_TOGGLE_TO_OFF) { ("Selected", None, false) } else { ("Deselect", Some(FieldValue::Radio(None)), true) }
            } else {
                ("Select", Some(FieldValue::Radio(on)), true)
            }
        }
        _ => return None,
    };
    let position = offer_position(ctx, view, info, form, rect)?;
    let mut out = None;
    let area = egui::Area::new(egui::Id::unique(("form-hover-offer", view.id.0, &name, wi))).order(egui::Order::Foreground).fixed_pos(position).show(
        ctx,
        |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(120.0);
                ui.label(egui::RichText::new(&name).small().color(Tokens::get(ui.ctx()).text_faint));
                if ui.add_enabled(enabled, egui::Button::new(tl!(label))).clicked() {
                    out = edit.map(|value| Edit::SetFieldValue { name: name.clone(), value });
                }
            });
        },
    );
    view.forms.offer_rect = Some(area.response.rect);
    if let Some(edit) = out.take() {
        commit(view, form);
        if view.pending_edit.is_some() {
            view.forms.queued = Some(edit);
        } else {
            out = Some(edit);
        }
        view.forms.offer = None;
        view.forms.offer_rect = None;
    }
    Some(out)
}
