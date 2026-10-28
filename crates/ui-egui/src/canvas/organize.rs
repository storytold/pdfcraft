use super::cache::{GRID_TAG, thumbnail_density, thumbnail_rows, thumbnail_scale};
use super::zoom::{GRID_ZOOM_RANGE, GRID_ZOOM_STEP};
use super::{
    Align2, Color32, CornerRadius, DocInfo, DocView, Edit, Pos2, Rect, RenderRequest, RequestKind, Sense, Stroke, Tokens, Vec2, ViewAction,
    device_pixels, icons, pos2, theme, vec2, widgets,
};

/// How far a sharp grid render may be from the size drawn before it is redone: a pinch changes
/// the size on every frame, and a slightly soft or slightly large image is fine meanwhile.
const GRID_SHARP: std::ops::RangeInclusive<f32> = 0.9..=1.6;

/// Whether an image `have` device pixels wide is good enough to draw `want` pixels wide.
pub(super) fn sharp_enough(have: u32, want: f32) -> bool {
    want > 0.0 && GRID_SHARP.contains(&(have as f32 / want))
}

impl DocView {
    /// Open Organize Pages on the page being read (issue #1910). An empty selection means the
    /// current page, so a selection left from earlier that does not include it is dropped, and
    /// the grid scrolls its cell into view. Every way into the grid goes through here; closing
    /// it is a plain `organize = false`.
    pub fn open_organize(&mut self) {
        self.organize = true;
        if !self.selected.is_empty() && !self.selected.contains(&self.current) {
            self.clear_page_selection(Some(self.current));
        }
        self.grid_reveal = Some(self.current);
    }

    /// Pages an organize command acts on: the selection, or the current page.
    pub fn target_pages(&self) -> Vec<usize> {
        if self.selected.is_empty() { vec![self.current] } else { self.selected.iter().copied().collect() }
    }

    pub fn select_pages(&mut self, pages: &[usize]) {
        self.selected = pages.iter().copied().filter(|p| *p < self.page_count).collect();
        if let Some(first) = self.selected.first() {
            self.current = *first;
        }
    }

    /// A click on page `i`'s thumbnail, in the organize grid or the Pages panel. ⇧ selects the
    /// range from the anchor (or the current page); ⌘/Ctrl toggles the page; a plain click
    /// selects only it. `seed_current` is for the Pages panel, where an empty selection means
    /// the current page: the first ⌘-click on another page keeps the current one selected too.
    pub fn click_page(&mut self, i: usize, modifiers: egui::Modifiers, seed_current: bool) {
        if i >= self.page_count {
            return;
        }
        if modifiers.shift {
            let a = self.select_anchor.unwrap_or(self.current);
            self.selected = (a.min(i)..=a.max(i)).collect();
        } else if modifiers.command {
            if seed_current && self.selected.is_empty() && i != self.current {
                self.selected.insert(self.current);
            }
            if !self.selected.remove(&i) {
                self.selected.insert(i);
            }
            self.select_anchor = Some(i);
        } else {
            self.selected = [i].into();
            self.select_anchor = Some(i);
        }
    }

    /// Drop the page selection; a later ⇧-click ranges from `anchor`.
    pub fn clear_page_selection(&mut self, anchor: Option<usize>) {
        self.selected.clear();
        self.select_anchor = anchor.filter(|a| *a < self.page_count);
    }
}

/// Where a page of `page` size (points, as shown: after its rotation) is drawn in the grid cell
/// `cell`: as large as the picture area allows and centred in it, so a landscape or rotated page
/// fills the cell's width in its middle, as Acrobat draws it, instead of a small strip at the top.
/// The picture area leaves room for the page number and actions below. Its sides narrow only when
/// a picture that wide would reach the selection checkbox in the cell's corner.
pub(crate) fn grid_page_rect(cell: Rect, page: Vec2) -> Rect {
    let area = Rect::from_min_max(pos2(cell.left(), cell.top() + 16.0), pos2(cell.right(), cell.bottom() - 72.0));
    // A page size from a damaged file can be zero, NaN or infinite; draw such a page square.
    let side = |v: f32| if v.is_finite() { v.max(1.0) } else { 1.0 };
    let (w, h) = (side(page.x), side(page.y));
    let fit = |margin: f32| {
        let s = ((area.width() - 2.0 * margin).max(1.0) / w).min(area.height().max(1.0) / h);
        Rect::from_center_size(area.center(), vec2(w * s, h * s))
    };
    let wide = fit(16.0);
    // The checkbox sits at (8, 8)-(28, 28) from the cell's corner; keep 4 px clear of it.
    let checkbox = Rect::from_min_size(cell.min + vec2(8.0, 8.0), Vec2::splat(20.0)).expand(4.0);
    if wide.intersects(checkbox) { fit(30.0) } else { wide }
}

/// The organize toolbar: page operations on the selection (Acrobat's Organize Pages bar).
fn organize_toolbar(view: &mut DocView, info: &DocInfo, editable: bool, dirty: bool, unobstructed: bool, ui: &mut egui::Ui, t: &Tokens) {
    let targets = view.target_pages();
    let n = info.pages.len();
    let (first, last) = (targets.first().copied().unwrap_or(0), targets.last().copied().unwrap_or(0));
    egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(16, 8)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let label = match view.selected.len() {
                0 => crate::i18n::fmt(tl!("Page {p} of {n}"), &[("p", &(view.current + 1).to_string()), ("n", &n.to_string())]),
                1 => tl!("1 page selected").to_string(),
                k => crate::i18n::fmt(tl!("{n} pages selected"), &[("n", &k.to_string())]),
            };
            // Fixed width so the buttons never shift as the selection text changes.
            ui.add_sized([150.0, 30.0], egui::Label::new(egui::RichText::new(label).font(theme::medium(13.0)).color(t.text_muted)).truncate());
            ui.add_enabled_ui(editable, |ui| {
                ui.add_space(8.0);
                if ui.add_enabled_ui(first > 0, |ui| icons::button(ui, "chevron-left", 30.0, false, tl!("Move earlier"))).inner.clicked() {
                    view.pending_edit = Some(Edit::MovePages { pages: targets.clone(), to: first - 1 });
                }
                if ui.add_enabled_ui(last + 1 < n, |ui| icons::button(ui, "chevron-right", 30.0, false, tl!("Move later"))).inner.clicked() {
                    view.pending_edit = Some(Edit::MovePages { pages: targets.clone(), to: first + 1 });
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::ghost_button(ui, "x", tl!("Close")).on_hover_text(tl!("Back to the document")).clicked() {
                    view.organize = false;
                }
                // What the grid shows is what is saved.
                let save = ui.add_enabled_ui(dirty, |ui| widgets::pill_button(ui, tl!("Save pages"), true)).inner;
                if save.on_hover_text(tl!("Save these pages as one PDF")).on_disabled_hover_text(tl!("No changes to save")).clicked() {
                    view.pending_action = Some(ViewAction::Save);
                }
                ui.add_space(8.0);
                // Page size in the grid (right to left: in, the percentage, out).
                let zoom = view.grid_zoom;
                if ui
                    .add_enabled_ui(zoom < *GRID_ZOOM_RANGE.end(), |ui| icons::button(ui, "zoom-in", 30.0, false, tl!("Larger pages")))
                    .inner
                    .clicked()
                {
                    view.grid_zoom_request = Some(zoom * GRID_ZOOM_STEP);
                }
                let percent = egui::Button::new(egui::RichText::new(format!("{:.0}%", zoom * 100.0)).font(theme::medium(12.0)).color(t.text_muted))
                    .frame(false);
                let percent = ui.add_sized([44.0, 30.0], percent);
                percent.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Reset page size")));
                if percent.on_hover_text(tl!("Reset page size")).clicked() {
                    view.grid_zoom_request = Some(1.0);
                }
                if ui
                    .add_enabled_ui(zoom > *GRID_ZOOM_RANGE.start(), |ui| icons::button(ui, "zoom-out", 30.0, false, tl!("Smaller pages")))
                    .inner
                    .clicked()
                {
                    view.grid_zoom_request = Some(zoom / GRID_ZOOM_STEP);
                }
            });
        });
    });
    // Keys act on the selection unless a text field or a dialog has them (Delete under
    // Preferences deleted the selected pages).
    if editable && unobstructed && !ui.ctx().egui_wants_keyboard_input() {
        use egui::{Key, Modifiers};
        let (del, esc) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        // ⌘C / ⌘X / ⌘V copy, cut and paste pages (egui delivers them as clipboard events).
        let (copy, cut, paste) = ui.input(|i| {
            let any = |f: &dyn Fn(&egui::Event) -> bool| i.events.iter().any(f);
            (any(&|e| matches!(e, egui::Event::Copy)), any(&|e| matches!(e, egui::Event::Cut)), any(&|e| matches!(e, egui::Event::Paste(_))))
        });
        if copy || cut {
            view.pending_action = Some(ViewAction::CopyPages { cut });
        }
        if paste {
            view.pending_action = Some(ViewAction::PastePages);
        }
        if del && targets.len() < n {
            view.pending_edit = Some(Edit::DeletePages { pages: targets });
        }
        if esc {
            view.selected.clear();
        }
    }
}

/// Organize pages: a thumbnail grid (Acrobat's Organize Pages view). Click selects, ⌘-click
/// toggles, ⇧-click extends; double-click opens the page.
/// The gap (0 = before the first page, n = after the last) the pointer points at in the grid.
pub(crate) fn drop_gap(cells: &[(usize, Rect)], p: Pos2) -> Option<usize> {
    let (i, r) = cells.iter().min_by(|(_, a), (_, b)| a.distance_sq_to_pos(p).total_cmp(&b.distance_sq_to_pos(p)))?;
    Some(if p.x < r.center().x { *i } else { i + 1 })
}

/// Ctrl (Option on macOS) held when a page drag drops copies the dragged pages into the gap
/// instead of moving them, as Acrobat's Organize Pages does.
///
/// The OS comes from the context, not `cfg!`: the wasm build runs on macOS too, where
/// `cfg!(target_os = "macos")` is false, so Option has to copy there as well (#3092 review).
fn copy_modifier(ctx: &egui::Context, m: &egui::Modifiers) -> bool {
    if crate::commands::mac_shortcuts(ctx) { m.alt } else { m.ctrl }
}

/// A "+" on a gap of the page grid: insert files there. Returns whether it was clicked.
fn gap_button(ui: &mut egui::Ui, gap: usize, at: Pos2, height: f32, label: String, t: &Tokens) -> bool {
    let resp = ui.interact(Rect::from_center_size(at, Vec2::splat(22.0)), ui.scope_id().with(("org-gap", gap)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.clone()));
    let hot = resp.hovered();
    if hot {
        ui.painter().line_segment([pos2(at.x, at.y - height / 2.0), pos2(at.x, at.y + height / 2.0)], Stroke::new(3.0, t.accent));
    }
    ui.painter().circle(at, 9.0, if hot { t.accent } else { t.chrome }, Stroke::new(1.0, if hot { t.accent } else { t.border }));
    let ink = Stroke::new(1.5, if hot { Color32::WHITE } else { t.text_muted });
    ui.painter().line_segment([at - vec2(4.0, 0.0), at + vec2(4.0, 0.0)], ink);
    ui.painter().line_segment([at - vec2(0.0, 4.0), at + vec2(0.0, 4.0)], ink);
    resp.on_hover_text(label).clicked()
}

/// Middle-button auto-scroll: its delta scrolls the grid, and the page controls under it are
/// disabled (but not dimmed) while the gesture lasts.
fn apply_auto_scroll(ui: &mut egui::Ui, middle_gesture: bool, auto_delta: Vec2) {
    if middle_gesture {
        let opacity = ui.opacity();
        ui.disable();
        ui.set_opacity(opacity);
    }
    if auto_delta != Vec2::ZERO {
        ui.scroll_with_delta_animation(auto_delta, egui::style::ScrollAnimation::none());
    }
}

/// Drag pages to move them (the selection, or the page grabbed); `drop` is set when the drag ends.
fn drag_pages(view: &mut DocView, resp: &egui::Response, page: usize, drop: &mut bool) {
    if resp.drag_started() {
        if !view.selected.contains(&page) {
            view.selected = [page].into();
            view.select_anchor = Some(page);
        }
        let mut pages: Vec<usize> = view.selected.iter().copied().collect();
        pages.sort_unstable();
        view.org_drag = Some(pages);
    }
    if resp.drag_stopped() {
        *drop = true;
    }
}

/// A page drawn larger than its thumbnail gets a render at the size drawn, while it is in view;
/// until that arrives the thumbnail stands in.
fn request_sharper(
    view: &DocView,
    page: usize,
    want: f32,
    page_width: f32,
    drawn_width: Option<u32>,
    in_view: &mut Vec<(usize, f32)>,
    sharper: &mut Vec<RenderRequest>,
) {
    in_view.push((page, want));
    if !drawn_width.is_some_and(|w| sharp_enough(w, want)) && !view.errors.contains_key(&page) {
        sharper.push(RenderRequest {
            page,
            kind: RequestKind::Pixels,
            tile: None,
            scale: want / page_width.max(1.0),
            tag: GRID_TAG,
            // Grid cells are previews: tiny images are skipped for them.
            preview: true,
        });
    }
}

pub(super) fn organize_grid(view: &mut DocView, info: &DocInfo, editable: bool, dirty: bool, unobstructed: bool, ui: &mut egui::Ui, t: &Tokens) {
    let ppp = ui.ctx().pixels_per_point();
    // The page image scales with the zoom; the padding and the page number don't.
    let cell = vec2(146.0 * view.grid_zoom + 44.0, 194.0 * view.grid_zoom + 88.0);
    let anchor = view.grid_anchor.take();
    let reveal = view.grid_reveal.take();
    let mut open_page = None;
    organize_toolbar(view, info, editable, dirty, unobstructed, ui, t);
    let viewport = ui.available_rect_before_wrap();
    view.viewport_screen = viewport;
    let auto_delta = if unobstructed {
        view.auto_scroll.update(ui, viewport, true)
    } else {
        view.auto_scroll.cancel();
        Vec2::ZERO
    };
    let middle_gesture = view.auto_scroll.blocks_input();
    let mut cells: Vec<(usize, Rect)> = Vec::new();
    // The pages in view that are drawn larger than their thumbnails, and those of them that
    // need a sharper render than they have. The width is the size drawn, in device pixels.
    let (mut in_view, mut sharper): (Vec<(usize, f32)>, Vec<RenderRequest>) = (Vec::new(), Vec::new());
    // The source cell can scroll out of the virtualized rows during a drag.
    let mut drop = view.org_drag.is_some() && ui.input(|i| i.pointer.primary_released());
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_viewport(ui, |ui, clip| {
        apply_auto_scroll(ui, middle_gesture, auto_delta);
        ui.add_space(20.0);
        let cols = ((ui.available_width() - 40.0) / cell.x).floor().max(1.0) as usize;
        let rows = info.pages.len().div_ceil(cols);
        let left = (ui.available_width() - cols as f32 * cell.x) / 2.0;
        let (grid, _) = ui.allocate_exact_size(vec2(ui.available_width(), rows as f32 * cell.y), Sense::hover());
        // The page Organize opened on may lie in a row the virtualized grid has not laid out,
        // so its cell is computed rather than found.
        if let Some(page) = reveal.filter(|p| *p < info.pages.len()) {
            let row = page / cols;
            let col = page % cols;
            let c = Rect::from_min_size(pos2(grid.left() + left + col as f32 * cell.x, grid.top() + row as f32 * cell.y), cell);
            ui.scroll_to_rect_animation(c, Some(egui::Align::Center), egui::style::ScrollAnimation::none());
        }
        for row in thumbnail_rows(clip.top() - 20.0, clip.bottom() - 20.0, cell.y, rows) {
            let row_rect = Rect::from_min_size(grid.min + vec2(0.0, row as f32 * cell.y), vec2(grid.width(), cell.y));
            for col in 0..cols {
                let i = row * cols + col;
                let Some(p) = info.pages.get(i) else { break };
                let c = Rect::from_min_size(pos2(row_rect.left() + left + col as f32 * cell.x, row_rect.top()), cell);
                view.need_thumbnail(i, c.intersects(ui.clip_rect()));
                let resp = ui.interact(c, ui.scope_id().with(("org", i)), if editable { Sense::click_and_drag() } else { Sense::click() });
                cells.push((i, c));
                if let Some((_, above)) = anchor.filter(|(page, _)| *page == i) {
                    ui.scroll_to_rect_animation(c.translate(vec2(0.0, -above)), Some(egui::Align::Min), egui::style::ScrollAnimation::none());
                }
                drag_pages(view, &resp, i, &mut drop);
                let page_label = crate::i18n::fmt(tl!("Page {label}"), &[("label", &p.label)]);
                resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, view.selected.contains(&i), page_label.clone()));
                let pr = grid_page_rect(c, vec2(p.width, p.height));
                let size = pr.size();
                let selected = view.selected.contains(&i) || (view.selected.is_empty() && i == view.current);
                if selected || resp.hovered() {
                    ui.painter().rect_filled(c.shrink(6.0), CornerRadius::same(8), if selected { t.accent_soft } else { t.hover });
                }
                if view.selected.contains(&i) {
                    ui.painter().rect_stroke(c.shrink(6.0), CornerRadius::same(8), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
                }
                ui.painter().rect_filled(pr.translate(vec2(0.0, 1.5)), CornerRadius::same(1), t.page_shadow);
                ui.painter().rect_filled(pr, CornerRadius::ZERO, Color32::WHITE);
                // A page drawn larger than its thumbnail gets a render at the size drawn, while
                // it is in view; until that arrives the thumbnail stands in.
                let want = size.x * ppp;
                let thumb = view.thumbs.get(&i).map(|t| &t.tex);
                let thumb_ok = thumb.is_some_and(|t| t.size()[0] as f32 >= want * GRID_SHARP.start());
                let sharp = view.grid_pages.get(&i).filter(|_| !thumb_ok);
                if !thumb_ok && ui.is_rect_visible(c) {
                    request_sharper(view, i, want, p.width, sharp.map(|(w, _)| *w), &mut in_view, &mut sharper);
                }
                if let Some(tex) = sharp.map(|(_, t)| t).or(thumb) {
                    ui.painter().image(tex.id(), pr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                ui.painter().rect_stroke(pr, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
                let label_font = theme::medium(12.0);
                let label_w = (c.width() - 12.0).max(0.0);
                let page_label = crate::widgets::fit_line(&p.label, label_w, |s| crate::widgets::text_width(ui, s, &label_font));
                let label_pos = pos2(c.center().x, c.bottom() - 48.0);
                ui.painter().with_clip_rect(Rect::from_center_size(label_pos, vec2(label_w, 18.0))).text(
                    label_pos,
                    Align2::CENTER_CENTER,
                    page_label,
                    label_font,
                    t.text_muted,
                );
                // The checkbox toggles just this page without requiring a modifier. Keep it
                // outside the page image, including on very tall or wide pages.
                let mut checked = view.selected.contains(&i);
                let check_rect = Rect::from_min_size(c.min + vec2(8.0, 8.0), Vec2::splat(20.0));
                let check = ui.push_id(("org-check", i), |ui| ui.put(check_rect, widgets::checkbox_without_text(&mut checked))).inner;
                let label = crate::i18n::fmt(tl!("Select page {label}"), &[("label", &p.label)]);
                check.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, checked, label.clone()));
                if check.on_hover_text(label).changed() {
                    view.click_page(i, egui::Modifiers::COMMAND, false);
                    // The document shows the page picked here when the grid closes, as after a
                    // Pages-panel click, so reopening the grid keeps the pick (issue #1910).
                    view.go_to_page(i);
                }
                // Local actions always target the labelled page. The toolbar above the grid
                // remains the place for actions on the whole selection.
                if selected || resp.contains_pointer() || resp.has_focus() {
                    let actions = Rect::from_center_size(pos2(c.center().x, c.bottom() - 24.0), vec2(84.0, 26.0));
                    ui.scope_builder(
                        egui::UiBuilder::new().id_salt(("org-actions", i)).max_rect(actions).layout(egui::Layout::left_to_right(egui::Align::Center)),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            ui.add_enabled_ui(editable, |ui| {
                                for (icon, degrees, key) in
                                    [("rotate-ccw", -90, "Rotate page {label} counterclockwise"), ("rotate-cw", 90, "Rotate page {label} clockwise")]
                                {
                                    let label = crate::i18n::fmt(tl!(key), &[("label", &p.label)]);
                                    if icons::button(ui, icon, 26.0, false, &label).clicked() {
                                        view.pending_edit = Some(Edit::RotatePages { pages: vec![i], degrees });
                                    }
                                }
                                let label = crate::i18n::fmt(tl!("Delete page {label}"), &[("label", &p.label)]);
                                if ui.add_enabled_ui(info.pages.len() > 1, |ui| icons::button(ui, "trash-2", 26.0, false, &label)).inner.clicked() {
                                    view.pending_edit = Some(Edit::DeletePages { pages: vec![i] });
                                }
                            });
                        },
                    );
                }
                if resp.clicked() {
                    let m = ui.input(|i| i.modifiers);
                    view.click_page(i, m, false);
                    view.go_to_page(i);
                }
                if resp.double_clicked() {
                    open_page = Some(i);
                }
                // Right-click: Extract, Replace, Cut, Copy, Paste (on the selection, or this page).
                resp.context_menu(|ui| {
                    if !view.selected.contains(&i) {
                        view.selected = [i].into();
                        view.go_to_page(i);
                    }
                    if ui.button(tl!("Extract pages")).clicked() {
                        view.pending_action = Some(ViewAction::Extract);
                        ui.close();
                    }
                    if ui.button(tl!("Split document…")).clicked() {
                        view.pending_action = Some(ViewAction::SplitBeforeSelected);
                        ui.close();
                    }
                    if ui.add_enabled(editable, egui::Button::new(tl!("Replace pages…"))).clicked() {
                        view.pending_action = Some(ViewAction::ReplacePages);
                        ui.close();
                    }
                    ui.separator();
                    if ui.add_enabled(editable, egui::Button::new(tl!("Cut"))).clicked() {
                        view.pending_action = Some(ViewAction::CopyPages { cut: true });
                        ui.close();
                    }
                    if ui.button(tl!("Copy")).clicked() {
                        view.pending_action = Some(ViewAction::CopyPages { cut: false });
                        ui.close();
                    }
                    if ui.add_enabled(editable, egui::Button::new(tl!("Paste after"))).clicked() {
                        view.pending_action = Some(ViewAction::PastePages);
                        ui.close();
                    }
                });
            }
        }
        // Between pages (and at both ends): insert files right there.
        if editable && !middle_gesture && view.org_drag.is_none() {
            let mid = |r: &Rect| r.top() + 16.0 + (cell.y - 88.0) / 2.0;
            for (i, c) in &cells {
                let label = info
                    .pages
                    .get(*i)
                    .map(|p| crate::i18n::fmt(tl!("Insert a file before page {label}"), &[("label", &p.label)]))
                    .unwrap_or_default();
                if gap_button(ui, *i, pos2(c.left(), mid(c)), cell.y - 88.0, label, t) {
                    view.pending_action = Some(ViewAction::InsertFromFileAt(*i));
                }
            }
            if let Some((i, c)) = cells.last()
                && i + 1 == info.pages.len()
                && gap_button(ui, i + 1, pos2(c.right(), mid(c)), cell.y - 88.0, tl!("Insert a file at the end").to_string(), t)
            {
                view.pending_action = Some(ViewAction::InsertFromFileAt(i + 1));
            }
        }
        // While dragging: the gap the pages would go to, drawn as a bar.
        if let (Some(pages), Some(p)) = (view.org_drag.clone(), ui.input(|i| i.pointer.hover_pos())) {
            // Ctrl (Option on macOS) held over the drop copies the pages; see #2997.
            let copying = copy_modifier(ui.ctx(), &ui.input(|i| i.modifiers));
            if let Some(gap) = drop_gap(&cells, p) {
                let x = match cells.iter().find(|(i, _)| *i == gap) {
                    Some((_, r)) => r.left() + 3.0,
                    None => cells.last().map_or(0.0, |(_, r)| r.right() - 3.0),
                };
                let row = cells.iter().find(|(i, _)| *i == gap).or(cells.last()).map(|(_, r)| r.y_range()).unwrap_or(egui::Rangef::new(0.0, 0.0));
                ui.painter().line_segment([pos2(x, row.min + 10.0), pos2(x, row.max - 10.0)], Stroke::new(3.0, t.accent));
                let mut text_at = p + vec2(14.0, 14.0);
                if copying {
                    // A "+" by the pointer: this drop copies the pages, it doesn't move them.
                    let badge = p + vec2(12.0, 12.0);
                    ui.painter().circle(badge, 9.0, t.accent, Stroke::new(1.0, t.accent));
                    let ink = Stroke::new(1.5, Color32::WHITE);
                    ui.painter().line_segment([badge - vec2(4.0, 0.0), badge + vec2(4.0, 0.0)], ink);
                    ui.painter().line_segment([badge - vec2(0.0, 4.0), badge + vec2(0.0, 4.0)], ink);
                    text_at.x += 18.0;
                }
                ui.painter().text(
                    text_at,
                    Align2::LEFT_TOP,
                    if pages.len() == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &pages.len().to_string())]) },
                    theme::medium(12.0),
                    t.accent_text,
                );
            }
            if drop {
                view.org_drag = None;
                if let Some(gap) = drop_gap(&cells, p) {
                    if copying {
                        // The copies land at the gap as it is: nothing leaves its place.
                        view.pending_edit = Some(Edit::DuplicatePages { pages: pages.clone(), at: Some(gap) });
                        view.selected = (gap..gap + pages.len()).collect();
                    } else {
                        // `to` counts positions without the moving pages.
                        let to = gap - pages.iter().filter(|x| **x < gap).count();
                        let first = pages[0];
                        let contiguous = pages.windows(2).all(|w| w[1] == w[0] + 1);
                        if !(contiguous && to == first) {
                            view.pending_edit = Some(Edit::MovePages { pages: pages.clone(), to });
                            view.selected = (to..to + pages.len()).collect();
                        }
                    }
                }
            }
        } else if drop {
            view.org_drag = None;
        }
    });
    // Files dragged in from outside go to the gap under the pointer. Some platforms don't say
    // where the pointer is until the files are dropped; then the whole grid is the target.
    let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|p| viewport.contains(*p));
    view.grid_gap = pointer.filter(|_| editable && view.org_drag.is_none()).and_then(|p| drop_gap(&cells, p));
    if editable && ui.input(|i| !i.raw.hovered_files.is_empty()) {
        let painter = ui.painter().with_clip_rect(viewport);
        painter.rect_stroke(viewport.shrink(2.0), CornerRadius::same(6), Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
        if let Some(gap) = view.grid_gap {
            let at = cells.iter().find(|(i, _)| *i == gap).map(|(_, r)| (r.left() + 3.0, r.y_range()));
            if let Some((x, row)) = at.or(cells.last().map(|(_, r)| (r.right() - 3.0, r.y_range()))) {
                painter.line_segment([pos2(x, row.min + 10.0), pos2(x, row.max - 10.0)], Stroke::new(3.0, t.accent));
            }
        }
        // A "+" that follows the pointer.
        if let Some(p) = pointer {
            let at = p + vec2(16.0, 16.0);
            painter.circle_filled(at, 9.0, t.accent);
            painter.line_segment([at - vec2(4.0, 0.0), at + vec2(4.0, 0.0)], Stroke::new(1.5, Color32::WHITE));
            painter.line_segment([at - vec2(0.0, 4.0), at + vec2(0.0, 4.0)], Stroke::new(1.5, Color32::WHITE));
        }
    }
    view.auto_scroll.paint(ui, viewport);
    // Pinch, or Ctrl/⌘ with the wheel, zooms the grid about the page under the pointer.
    let pinch = crate::motion::zoom_delta(ui.ctx());
    if (pinch - 1.0).abs() > 0.001 && pointer.is_some() && !middle_gesture {
        view.grid_zoom_request = Some(view.grid_zoom * pinch);
    }
    if let Some(zoom) = view.grid_zoom_request.take() {
        let before = view.grid_zoom;
        view.set_grid_zoom(zoom);
        if view.grid_zoom != before {
            // Keep the page under the pointer (or else the first one in view) where it is.
            let under = pointer.and_then(|p| cells.iter().find(|(_, r)| r.contains(p)));
            let anchor = under.or_else(|| cells.iter().find(|(_, r)| r.bottom() > viewport.top()));
            view.grid_anchor = anchor.map(|(i, r)| (*i, r.top() - viewport.top()));
            ui.ctx().request_repaint();
        }
    }
    // When this frame's thumbnail will cover the cell, drop the sharp render without waiting
    // for that texture to arrive.
    let density = thumbnail_density(view.thumb_demand.len(), ppp);
    in_view.retain(|&(page, want)| {
        info.pages.get(page).is_none_or(|p| (device_pixels(p.width, thumbnail_scale(p.width, p.height, density)) as f32) < want * GRID_SHARP.start())
    });
    sharper.retain(|req| in_view.iter().any(|(page, _)| *page == req.page));
    view.grid_pages.retain(|page, _| in_view.iter().any(|(kept, _)| kept == page));
    // Sharp renders of the pages in view join this frame's demand ahead of the thumbnails.
    view.frame_queue.extend(sharper);
    if let Some(p) = open_page {
        view.organize = false;
        view.go_to_page(p);
    }
}
