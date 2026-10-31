use super::state::PageCx;
use super::{
    Annotation, Color32, CommentPrefs, CornerRadius, DocInfo, DocView, Edit, PagePointer, PageXform, Pos2, QuickTool, Rect, Stroke, Tokens, pos2,
    theme,
};

/// A comment with a pop-up window of its own (not a text box, a Fill & Sign mark or a reply).
pub(super) fn has_popup(a: &Annotation) -> bool {
    a.subtype != "FreeText" && !a.fill_sign && a.in_reply_to.is_none()
}

/// The line from a comment to its open pop-up, in the comment's colour, while the pointer is on
/// either or the comment is selected (Acrobat's "lines connecting comment markups to their
/// pop-ups on mouse rollover"). It runs from the point of the comment nearest the window to the
/// point of the window nearest the comment.
pub(super) fn paint_connectors(painter: &egui::Painter, cx: &PageCx<'_>, view: &DocView, pointer: Option<Pos2>) {
    let cv = &view.comments;
    for ((page, index), window) in &cv.popup_rects {
        if *page != cx.page {
            continue;
        }
        let Some(a) = cx.get(*index) else { continue };
        if !a.popup.is_some_and(|p| p.open) {
            continue;
        }
        let rects: Vec<Rect> = cx.screen_rects(a).collect();
        let on_mark = pointer.is_some_and(|p| rects.iter().any(|r| r.expand(3.0).contains(p)));
        if !(on_mark || cv.selected == Some((*page, *index)) || cv.popup_hovered == Some((*page, *index))) {
            continue;
        }
        let Some(mark) = rects.first() else { continue };
        let from = nearest_on(*mark, window.center());
        let to = nearest_on(*window, from);
        if from.distance(to) < 4.0 {
            continue;
        }
        let color = a
            .color
            .map_or(Color32::from_rgb(0xDC, 0x26, 0x26), |c| Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8));
        painter.line_segment([from, to], Stroke::new(1.5, color));
    }
}

/// The point of `r`'s boundary nearest `p` (`p` inside `r` is pushed to the nearer vertical edge).
fn nearest_on(r: Rect, p: Pos2) -> Pos2 {
    let x = p.x.clamp(r.left(), r.right());
    let y = p.y.clamp(r.top(), r.bottom());
    if r.left() < x && x < r.right() && r.top() < y && y < r.bottom() {
        pos2(if (p.x - r.left()).abs() <= (p.x - r.right()).abs() { r.left() } else { r.right() }, y)
    } else {
        pos2(x, y)
    }
}

/// The open pop-up windows of the comments on screen: author, date and the comment's text,
/// editable in place; the title bar drags the window (its new place is saved with the comment);
/// × closes it. Returns the edits to apply (one, or a batch when a click both saves typed text
/// and closes). Hidden comments show none.
pub(crate) fn popups(ctx: &egui::Context, view: &mut DocView, info: &DocInfo, hidden: bool, allowed: bool) -> Option<Edit> {
    let t = Tokens::get(ctx);
    let pointer = ctx.input(|i| i.pointer.hover_pos());
    let mut edits: Vec<Edit> = Vec::new();
    let mut rects = Vec::new();
    let mut hovered = None;
    let xforms: Vec<(usize, PageXform)> = view.page_xforms();
    for (page, xf) in xforms {
        let shown: Vec<Annotation> =
            info.annotations.iter().filter(|a| !hidden && a.page == page && has_popup(a) && a.popup.is_some_and(|p| p.open)).cloned().collect();
        for a in shown {
            let Some(p) = a.popup else { continue };
            let key = (page, a.index);
            let placed = xf.user_rect(info, page, p.rect);
            // The window keeps a readable size whatever the zoom: its place follows the page,
            // its width stays between what a note needs and what fits.
            let width = placed.width().clamp(220.0, 420.0);
            let dragging = view.comments.popup_drag.filter(|(dp, di, _)| (*dp, *di) == key).map(|(_, _, at)| at);
            let pos = dragging.unwrap_or(placed.left_top());
            let id = egui::Id::unique(("comment-popup", view.id.0, page, a.index));
            let color = a
                .color
                .map_or(Color32::from_rgb(0xFF, 0xD6, 0x5C), |c| Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8));
            let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(egui::Margin::ZERO).corner_radius(CornerRadius::same(6)).show(ui, |ui| {
                    ui.set_width(width);
                    // Title bar: the comment's colour, author and date; drag to move, × to close.
                    let bar = egui::Frame::NONE.fill(color.gamma_multiply(0.35)).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                        ui.set_width(width - 20.0);
                        ui.horizontal(|ui| {
                            let who = a.author.clone().unwrap_or_else(|| crate::comments_panel::kind_label(&a).to_string());
                            ui.label(egui::RichText::new(who.as_str()).font(theme::semibold(13.0)).color(t.text));
                            if let Some(m) = &a.modified {
                                ui.label(egui::RichText::new(m).font(theme::regular(11.0)).color(t.text_faint));
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                // Closing changes the comment's /Open, which needs comment permission.
                                if ui.add_enabled_ui(allowed, |ui| crate::icons::button(ui, "x", 18.0, false, tl!("Close"))).inner.clicked() {
                                    edits.push(Edit::SetAnnotationPopup { page, index: a.index, rect: None, open: Some(false) });
                                }
                            });
                        });
                    });
                    // The bar drags the window, except where the × is: a region registered after
                    // the button would take its clicks.
                    let bar_rect = bar.response.rect;
                    let grip = ui.interact(
                        Rect::from_min_max(bar_rect.min, pos2(bar_rect.right() - 30.0, bar_rect.bottom())),
                        id.with("grip"),
                        egui::Sense::drag(),
                    );
                    if grip.hovered() || grip.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                    if grip.drag_started() {
                        view.comments.popup_drag = Some((page, a.index, pos));
                    }
                    if grip.dragged()
                        && let Some((_, _, at)) = view.comments.popup_drag.as_mut()
                    {
                        *at += grip.drag_delta();
                    }
                    if grip.drag_stopped()
                        && let Some((_, _, at)) = view.comments.popup_drag.take()
                        && allowed
                    {
                        // The same size on the page, at the new place.
                        let moved = Rect::from_min_size(at, placed.size());
                        // Only converts the rectangle to user space: the pointer and the
                        // redaction look are unused.
                        let cx = PageCx {
                            page,
                            xf: &xf,
                            ptr: &PagePointer::default(),
                            info,
                            tool: QuickTool::Select,
                            prefs: &CommentPrefs::default(),
                            allowed,
                            hidden,
                            redact_mark: ([0.0; 3], 1.0),
                        };
                        let rect = cx.rect_to_user(moved);
                        if at.distance(placed.left_top()) >= 2.0 {
                            edits.push(Edit::SetAnnotationPopup { page, index: a.index, rect: Some(rect), open: None });
                        }
                    }
                    // Body: the text, typed in place; saved when the field loses focus.
                    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
                        ui.set_width(width - 20.0);
                        if allowed && !a.locked {
                            let typed = view.comments.popup_text.as_ref().filter(|(p, i, _)| (*p, *i) == key).map(|(_, _, s)| s.clone());
                            let mut text = typed.unwrap_or_else(|| a.contents.clone().unwrap_or_default());
                            let field = ui.add(
                                egui::TextEdit::multiline(&mut text)
                                    .hint_text(tl!("Add a comment"))
                                    .desired_rows(3)
                                    .desired_width(f32::INFINITY)
                                    .id_salt(("popup-text", page, a.index)),
                            );
                            if field.changed() {
                                view.comments.popup_text = Some((page, a.index, text.clone()));
                            }
                            if field.lost_focus()
                                && let Some((_, _, s)) = view.comments.popup_text.take_if(|(p, i, _)| (*p, *i) == key)
                                && s.trim_end() != a.contents.as_deref().unwrap_or_default()
                            {
                                edits.push(Edit::SetAnnotationContents { page, index: a.index, text: s.trim_end().to_string() });
                            }
                        } else {
                            ui.label(a.contents.as_deref().unwrap_or_default());
                        }
                        // Replies, as the thread reads: text replies only, as in the Comments
                        // panel. A review status or a checkmark (`/State`) is a reply too, but
                        // not one to read here.
                        let replies: Vec<&Annotation> = info
                            .annotations
                            .iter()
                            .filter(|r| r.page == page && r.in_reply_to.is_some() && r.in_reply_to == a.name && r.state.is_none())
                            .collect();
                        for r in replies {
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.label(egui::RichText::new(r.author.as_deref().unwrap_or("?")).font(theme::semibold(12.0)));
                                ui.label(egui::RichText::new(r.contents.as_deref().unwrap_or_default()).font(theme::regular(12.0)));
                            });
                        }
                    });
                });
            });
            let shown_at = area.response.rect;
            if pointer.is_some_and(|p| shown_at.contains(p)) {
                hovered = Some(key);
            }
            rects.push((key, shown_at));
        }
    }
    view.comments.popup_rects = rects;
    view.comments.popup_hovered = hovered;
    match edits.len() {
        0 => None,
        1 => edits.pop(),
        _ => Some(Edit::Batch { label: "Edit comment".into(), edits }),
    }
}
