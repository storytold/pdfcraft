use super::{Align, Align2, Color32, CornerRadius, DocInfo, Layout, Nav, Rect, Sense, Stroke, Tokens, icons, pos2, theme, vec2};

/// Page tools in the Pages panel's header (#4415): a menu, as Bookmark options is in its panel,
/// to insert a blank page or pages from a file, rotate, extract and delete. Each runs its
/// command on the selected pages (the current page when none is), so it does what Pages ▸ …
/// does; the items say so, apart from the thumbnails' own menu. `enabled` is false while the
/// document may not be assembled or a dialog is open. Returns the command a click asks for.
pub(super) fn page_tools(ui: &mut egui::Ui, enabled: bool) -> Option<&'static str> {
    let tools: [(&'static str, &str); 6] = [
        ("page.insert_blank", tl!("Insert a blank page after the selection")),
        ("page.insert", tl!("Insert pages from a file after the selection…")),
        ("page.rotate_ccw", tl!("Rotate the selected pages counterclockwise")),
        ("page.rotate", tl!("Rotate the selected pages clockwise")),
        ("page.extract", tl!("Extract the selected pages…")),
        ("page.delete", tl!("Delete the selected pages")),
    ];
    let mut picked = None;
    ui.add_enabled_ui(enabled, |ui| {
        let more = icons::button(ui, "ellipsis", 26.0, false, tl!("Page tools"));
        egui::Popup::menu(&more).show(|ui| {
            ui.set_min_width(260.0);
            for (id, label) in tools {
                let Some(spec) = pdfcraft_engine::commands::command(id) else { continue };
                let item = egui::Button::image_and_text(icons::image(spec.icon, 16.0, ui.visuals().text_color()), label);
                if ui.add(item).clicked() {
                    picked = Some(id);
                    ui.close();
                }
            }
        });
    });
    picked
}

/// Size controls stay above the scroll area, so they remain reachable on long documents.
pub(super) fn thumbnail_size(ui: &mut egui::Ui, t: &Tokens, view: &mut crate::DocView, enabled: bool) {
    let before = view.thumbnail_zoom();
    let mut zoom = before;
    let label = ui
        .horizontal(|ui| {
            let label = ui.label(egui::RichText::new(tl!("Thumbnail size")).small().color(t.text_muted));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(egui::RichText::new(format!("{:.0}%", before * 100.0)).small().color(t.text_muted));
            });
            label
        })
        .inner;
    ui.add_enabled_ui(enabled, |ui| {
        ui.spacing_mut().slider_width = (ui.available_width() - 52.0 - 2.0 * ui.spacing().item_spacing.x).max(1.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled_ui(zoom > *crate::canvas::THUMBNAIL_ZOOM_RANGE.start(), |ui| {
                    icons::button(ui, "zoom-out", 26.0, false, tl!("Smaller thumbnails"))
                })
                .inner
                .clicked()
            {
                zoom -= 0.25;
            }
            let mut percent = zoom * 100.0;
            let range = (*crate::canvas::THUMBNAIL_ZOOM_RANGE.start() * 100.0)..=(*crate::canvas::THUMBNAIL_ZOOM_RANGE.end() * 100.0);
            ui.scope(|ui| {
                // The regular field fill blends into this panel; keep the slider's full rail visible.
                ui.visuals_mut().widgets.inactive.bg_fill = t.border;
                let slider = ui.add(egui::Slider::new(&mut percent, range).step_by(1.0).show_value(false).trailing_fill(true)).labelled_by(label.id);
                slider.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), f64::from(percent), tl!("Thumbnail size")));
            });
            zoom = percent / 100.0;
            if ui
                .add_enabled_ui(zoom < *crate::canvas::THUMBNAIL_ZOOM_RANGE.end(), |ui| {
                    icons::button(ui, "zoom-in", 26.0, false, tl!("Larger thumbnails"))
                })
                .inner
                .clicked()
            {
                zoom += 0.25;
            }
        });
    });
    if zoom != before {
        view.set_thumbnail_zoom(zoom);
    }
    ui.add_space(6.0);
}

/// The page thumbnails. A click goes to the page; ⌘/Ctrl-click and ⇧-click pick several pages
/// (the selection page commands and Print act on) without moving the document. Right-click
/// offers Extract, Cut, Copy and Paste on the selection, or on the page clicked; dragging
/// moves the selection, or the page grabbed, to the gap it is dropped on.
pub(super) fn pages(ui: &mut egui::Ui, t: &Tokens, info: &DocInfo, view: &mut crate::DocView, modal: bool, editable: bool, nav: &mut Option<Nav>) {
    // Escape drops the selection while the pointer is over the panel and nothing else wants the key.
    if !modal
        && !view.selected.is_empty()
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.rect_contains_pointer(ui.clip_rect())
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        view.clear_page_selection(Some(view.current));
    }
    // Delete (or Backspace) deletes the selection, or the current page, likewise.
    if !modal
        && editable
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.rect_contains_pointer(ui.clip_rect())
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace))
    {
        delete_pages(view, info.pages.len());
    }
    // A new current page scrolls its thumbnail into view, as little as it takes (Acrobat), but
    // not while thumbnails are dragged: then the change counts as seen. A wheel turned or a
    // scroll bar dragged by the user already wins over a scroll to a place in egui.
    let follow = view.panel_followed != Some(view.current) && view.panel_drag.is_none();
    view.panel_followed = Some(view.current);
    let available = (ui.available_width() - 16.0).max(1.0);
    let usual = (ui.available_width() - 40.0).clamp(1.0, 150.0);
    let w = (usual * view.thumbnail_zoom()).min(available);
    let count = info.pages.len();
    let mut rows: Vec<Rect> = Vec::with_capacity(info.pages.len());
    let mut dropped = false;
    for (i, p) in info.pages.iter().enumerate() {
        ui.vertical_centered(|ui| {
            let h = w * p.height / p.width.max(1.0);
            let sense = if editable { Sense::click_and_drag() } else { Sense::click() };
            let (rect, resp) = ui.allocate_exact_size(vec2(w + 16.0, h + 16.0), sense);
            rows.push(rect);
            if rect.intersects(ui.clip_rect().expand(200.0)) {
                view.need_thumbnail(i, rect.intersects(ui.clip_rect()));
            }
            // Only the primary button drags pages (the middle button scrolls, the secondary one opens the menu).
            if resp.drag_started_by(egui::PointerButton::Primary) {
                if !view.target_pages().contains(&i) {
                    view.click_page(i, egui::Modifiers::NONE, true);
                }
                view.panel_drag = Some(view.target_pages());
            }
            if resp.drag_stopped() {
                dropped = true;
            }
            let info = crate::i18n::fmt(tl!("Page {label}"), &[("label", &p.label)]);
            let picked = view.selected.contains(&i);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, picked, info.clone()));
            // The current page keeps its heavier border; picked pages share its fill.
            let selected = i == view.current;
            let press = theme::Press::track(ui, &resp);
            press.wash(ui, rect, 8, selected || picked);
            if picked {
                ui.painter().rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
            }
            let pr = rect.translate(press.offset()).shrink(8.0);
            ui.painter().rect_filled(pr, CornerRadius::ZERO, Color32::WHITE);
            if let Some(tex) = view.thumb(i) {
                ui.painter().image(tex.id(), pr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            ui.painter().rect_stroke(
                pr,
                CornerRadius::ZERO,
                Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { t.accent } else { t.border }),
                egui::StrokeKind::Outside,
            );
            let label = ui.label(egui::RichText::new(&p.label).font(theme::medium(12.0)).color(if selected || picked {
                t.accent_text
            } else {
                t.text_muted
            }));
            if follow && selected {
                ui.scroll_to_rect(rect.union(label.rect), None);
            }
            let resp = theme::hand(resp);
            if resp.clicked() {
                let m = ui.input(|i| i.modifiers);
                if m.shift || m.command {
                    view.click_page(i, m, true);
                } else {
                    view.clear_page_selection(Some(i));
                    // A page clicked here is in view already: the panel stays put.
                    view.panel_followed = Some(i);
                    *nav = Some(Nav::Page(i));
                }
            }
            resp.context_menu(|ui| {
                // A page outside the selection becomes the selection, as in the organize grid.
                if !view.target_pages().contains(&i) {
                    view.click_page(i, egui::Modifiers::NONE, true);
                }
                if ui.button(tl!("Extract pages")).clicked() {
                    view.pending_action = Some(crate::canvas::ViewAction::Extract);
                    ui.close();
                }
                if ui.button(tl!("Split document…")).clicked() {
                    if view.selected.is_empty() {
                        view.select_pages(&[i]);
                    }
                    view.pending_action = Some(crate::canvas::ViewAction::SplitBeforeSelected);
                    ui.close();
                }
                if ui.add_enabled(editable, egui::Button::new(tl!("Replace pages…"))).clicked() {
                    view.pending_action = Some(crate::canvas::ViewAction::ReplacePages);
                    ui.close();
                }
                ui.separator();
                if ui.add_enabled(editable, egui::Button::new(tl!("Cut"))).clicked() {
                    view.pending_action = Some(crate::canvas::ViewAction::CopyPages { cut: true });
                    ui.close();
                }
                if ui.button(tl!("Copy")).clicked() {
                    view.pending_action = Some(crate::canvas::ViewAction::CopyPages { cut: false });
                    ui.close();
                }
                if ui.add_enabled(editable, egui::Button::new(tl!("Paste after"))).clicked() {
                    view.pending_action = Some(crate::canvas::ViewAction::PastePages);
                    ui.close();
                }
                ui.separator();
                let some_left = view.target_pages().len() < count;
                // Never every page: a document keeps at least one.
                if ui.add_enabled(editable && some_left, egui::Button::new(tl!("Delete pages"))).clicked() {
                    delete_pages(view, count);
                    ui.close();
                }
            });
        });
        ui.add_space(4.0);
    }
    page_drag(ui, t, view, &rows, dropped);
    file_drop(ui, t, view, &rows, editable && !modal);
}

/// Files dragged in from outside go to the gap under the pointer: remember where the panel is
/// and that gap, for the app to take the drop, and show where the pages would go.
fn file_drop(ui: &mut egui::Ui, t: &Tokens, view: &mut crate::DocView, rows: &[Rect], editable: bool) {
    if !editable {
        return;
    }
    let clip = ui.clip_rect();
    view.pages_panel = Some(clip);
    let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|p| clip.contains(*p));
    view.panel_gap = pointer.filter(|_| view.panel_drag.is_none()).and_then(|p| panel_gap(rows, p.y));
    if pointer.is_none() || ui.input(|i| i.raw.hovered_files.is_empty()) {
        return;
    }
    let painter = ui.painter().with_clip_rect(clip);
    painter.rect_stroke(clip.shrink(2.0), CornerRadius::same(6), Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
    if let (Some(gap), Some(first), Some(last)) = (view.panel_gap, rows.first(), rows.last()) {
        let y = rows.get(gap).map_or(last.bottom() + 2.0, |r| r.top() - 3.0);
        painter.line_segment([pos2(first.left(), y), pos2(first.right(), y)], Stroke::new(3.0, t.accent));
    }
}

/// Queue deleting the selection (or the current page) of a document of `n` pages, unless that
/// would leave no page. The selection goes; the current page stays on the page it showed, or on
/// the page that follows the deleted ones.
fn delete_pages(view: &mut crate::DocView, n: usize) {
    let mut pages = view.target_pages();
    pages.sort_unstable();
    pages.dedup();
    if pages.is_empty() || pages.len() >= n {
        return;
    }
    // Deleted pages before it shift it up; if it was deleted itself, the page after the deleted
    // run lands where it was (or the last page, when the deletion ran to the end).
    let before = pages.iter().filter(|p| **p < view.current).count();
    view.current = view.current.saturating_sub(before).min(n.saturating_sub(pages.len() + 1));
    view.clear_page_selection(None);
    view.pending_edit = Some(pdfcraft_engine::Edit::DeletePages { pages });
}

/// The gap (0 = before the first page, n = after the last) the pointer points at in the Pages
/// panel, whose thumbnails are `rows`, top to bottom.
fn panel_gap(rows: &[Rect], y: f32) -> Option<usize> {
    let (i, r) = rows.iter().enumerate().min_by(|(_, a), (_, b)| (a.center().y - y).abs().total_cmp(&(b.center().y - y).abs()))?;
    Some(if y < r.center().y { i } else { i + 1 })
}

/// Where page `p` goes when `moving` (sorted) move to `to` (counted without them).
fn moved_index(p: usize, moving: &[usize], to: usize) -> usize {
    match moving.iter().position(|m| *m == p) {
        Some(k) => to + k,
        None => {
            let rest = p.saturating_sub(moving.iter().filter(|m| **m < p).count());
            if rest >= to { rest + moving.len() } else { rest }
        }
    }
}

/// While thumbnails are dragged: a bar on the gap they would go to, the page count by the
/// pointer, and scrolling near the panel's edges. On release, queue the move.
fn page_drag(ui: &mut egui::Ui, t: &Tokens, view: &mut crate::DocView, rows: &[Rect], dropped: bool) {
    let Some(mut moving) = view.panel_drag.clone() else { return };
    // A drag that ended while the panel was closed (or elsewhere) leaves nothing to drop.
    if !dropped && !ui.input(|i| i.pointer.primary_down()) {
        view.panel_drag = None;
        return;
    }
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let gap = pointer.and_then(|p| panel_gap(rows, p.y));
    if let (Some(gap), Some(first), Some(last)) = (gap, rows.first(), rows.last()) {
        let y = rows.get(gap).map_or(last.bottom() + 20.0, |r| r.top() - 3.0);
        ui.painter().line_segment([pos2(first.left(), y), pos2(first.right(), y)], Stroke::new(3.0, t.accent));
    }
    if let Some(p) = pointer {
        let n = moving.len();
        let what = if n == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &n.to_string())]) };
        ui.painter().text(p + vec2(14.0, 14.0), Align2::LEFT_TOP, what, theme::medium(12.0), t.accent_text);
        let clip = ui.clip_rect();
        let edge = 40.0;
        if p.y < clip.top() + edge {
            ui.scroll_with_delta(vec2(0.0, 10.0));
        } else if p.y > clip.bottom() - edge {
            ui.scroll_with_delta(vec2(0.0, -10.0));
        }
        ui.ctx().request_repaint();
    }
    if !dropped {
        return;
    }
    view.panel_drag = None;
    let Some(gap) = gap else { return };
    moving.sort_unstable();
    moving.dedup();
    let Some(&first) = moving.first() else { return };
    // `to` counts positions without the moving pages.
    let to = gap.saturating_sub(moving.iter().filter(|x| **x < gap).count());
    let contiguous = moving.windows(2).all(|w| w[1] == w[0] + 1);
    if contiguous && to == first {
        return;
    }
    view.current = moved_index(view.current, &moving, to);
    view.selected = (to..to + moving.len()).collect();
    view.pending_edit = Some(pdfcraft_engine::Edit::MovePages { pages: moving, to });
}

#[cfg(test)]
mod page_moves {
    use super::{moved_index, panel_gap};
    use egui::{Rect, pos2};

    /// Above a thumbnail's middle is the gap before it; below is the gap after it.
    #[test]
    fn the_gap_follows_the_nearest_thumbnail() {
        let rows: Vec<Rect> = (0..3).map(|i| Rect::from_min_max(pos2(0.0, i as f32 * 100.0), pos2(50.0, i as f32 * 100.0 + 90.0))).collect();
        assert_eq!(panel_gap(&rows, -20.0), Some(0));
        assert_eq!(panel_gap(&rows, 30.0), Some(0));
        assert_eq!(panel_gap(&rows, 60.0), Some(1));
        assert_eq!(panel_gap(&rows, 260.0), Some(3));
        assert_eq!(panel_gap(&rows, 900.0), Some(3));
        assert_eq!(panel_gap(&[], 10.0), None);
    }

    /// The current page follows a move, whether it moved or others moved around it.
    #[test]
    fn pages_keep_their_identity_across_a_move() {
        // [0 1 2 3 4], move [1 2] to the end: [0 3 4 1 2].
        let order: Vec<usize> = (0..5).map(|p| moved_index(p, &[1, 2], 3)).collect();
        assert_eq!(order, [0, 3, 4, 1, 2]);
        // Move [3] to the front: [3 0 1 2 4].
        let order: Vec<usize> = (0..5).map(|p| moved_index(p, &[3], 0)).collect();
        assert_eq!(order, [1, 2, 3, 0, 4]);
    }
}
