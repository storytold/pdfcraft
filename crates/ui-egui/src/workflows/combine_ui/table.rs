use super::files::{Columns, Lock, RowAction, SortKey};
use super::parts::{ago, utc_stamp};
use super::*;

/// A column heading being dragged to a new place.
#[derive(Clone, Copy)]
struct HeadingDrag(SortKey);

enum HeadingEvent {
    Sort(SortKey),
    /// Put column `kind` before (or after) column `to`.
    Move {
        kind: SortKey,
        to: SortKey,
        after: bool,
    },
    Reset,
}

/// What the table reports besides row actions.
#[derive(Default)]
pub(super) struct TableEvents {
    pub(super) action: Option<RowAction>,
    pub(super) range_focused: bool,
    pub(super) range_changed: bool,
}

/// The file table.
pub(super) fn table(
    app: &mut PdfCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    checks: &[Result<usize, String>],
    selected: &[bool],
    conflicts: &[Vec<(String, String)>],
    twice: &[bool],
) -> TableEvents {
    let mut events = TableEvents::default();
    let dragging = egui::DragAndDrop::has_payload_of_type::<usize>(ui.ctx());
    // Until the user resizes a column, the widths follow the window.
    let gap = ui.spacing().item_spacing.x;
    let available = ui.available_width() - 18.0 - gap * 6.0;
    if app.combine_columns.widths.is_none() && (available - app.combine_tab.fitted).abs() > 1.0 {
        app.combine_tab.fitted = available;
        app.combine_tab.layout = app.combine_tab.layout.wrapping_add(1);
    }
    let widths = app.combine_columns.resolved(available);
    let rows = Rows { checks, selected, twice, conflicts, dragging, sort: app.combine_tab.sort };
    let mut heading_event = None;
    let mut shown = None;
    egui::ScrollArea::horizontal()
        .id_salt("combine-files-h")
        .show(ui, |ui| table_body(app, ui, t, &rows, widths, &mut events, &mut heading_event, &mut shown));
    // A column resized by hand keeps its width from now on.
    if let Some(shown) = shown
        && shown.iter().zip(&widths).any(|(a, b)| (a - b).abs() > 0.5)
    {
        app.combine_columns.widths = Some(shown);
    }
    match heading_event {
        Some(HeadingEvent::Sort(key)) => events.action = Some(RowAction::Sort(key)),
        Some(HeadingEvent::Move { kind, to, after }) => {
            let mut order: Vec<SortKey> = app.combine_columns.order.iter().copied().filter(|k| *k != kind).collect();
            let at = order.iter().position(|k| *k == to).map_or(order.len(), |p| if after { p + 1 } else { p });
            order.insert(at, kind);
            if let Ok(order) = <[SortKey; 5]>::try_from(order) {
                app.combine_columns.order = order;
                app.combine_tab.layout = app.combine_tab.layout.wrapping_add(1);
            }
        }
        Some(HeadingEvent::Reset) => {
            app.combine_columns = Columns::default();
            app.combine_tab.fitted = 0.0;
        }
        None => {}
    }
    events
}

/// What every row needs to know.
struct Rows<'a> {
    checks: &'a [Result<usize, String>],
    selected: &'a [bool],
    twice: &'a [bool],
    /// What combining would rename of this file's form fields (the field-name conflict preview).
    conflicts: &'a [Vec<(String, String)>],
    dragging: bool,
    sort: Option<(SortKey, bool)>,
}

/// A column heading: the whole cell sorts by its column when clicked, moves the column when
/// dragged, and offers Reset columns on a right-click.
fn heading(ui: &mut egui::Ui, t: &Tokens, key: SortKey, sort: Option<(SortKey, bool)>) -> Option<HeadingEvent> {
    let text = key.label();
    let rect = ui.max_rect();
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    let arrow = sort.filter(|(k, _)| *k == key).map(|(_, ascending)| ascending);
    let state = match arrow {
        Some(true) => tl!("sorted ascending"),
        Some(false) => tl!("sorted descending"),
        None => "",
    };
    let a11y = if state.is_empty() { text.to_owned() } else { format!("{text}, {state}") };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &a11y));
    let being_dragged = egui::DragAndDrop::payload::<HeadingDrag>(ui.ctx()).is_some_and(|p| p.0 == key);
    if being_dragged || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), t.pressed);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), t.hover);
    }
    let colour = if arrow.is_some() { t.text } else { t.text_muted };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), theme::medium(12.5), colour);
    let text_pos = egui::pos2(rect.left() + 4.0, rect.center().y - galley.size().y / 2.0);
    let text_w = galley.size().x;
    ui.painter().with_clip_rect(rect).galley(text_pos, galley, colour);
    if let Some(ascending) = arrow {
        let r = Rect::from_center_size(egui::pos2(rect.left() + text_w + 14.0, rect.center().y), vec2(12.0, 12.0));
        icons::paint(ui, r, if ascending { "chevron-up" } else { "chevron-down" }, 12.0, t.text);
    }
    if response.drag_started() {
        egui::DragAndDrop::set_payload(ui.ctx(), HeadingDrag(key));
    }
    // Another heading dragged over this one: a line where it would go.
    let after = |r: &egui::Response| r.ctx.pointer_latest_pos().is_some_and(|p| p.x > rect.center().x);
    if let Some(dragged) = response.dnd_hover_payload::<HeadingDrag>()
        && dragged.0 != key
    {
        let x = if after(&response) { rect.right() } else { rect.left() };
        ui.painter().with_clip_rect(rect.expand(3.0)).vline(x, rect.y_range(), Stroke::new(2.5, t.accent));
    }
    let mut event = None;
    if let Some(dragged) = response.dnd_release_payload::<HeadingDrag>()
        && dragged.0 != key
    {
        event = Some(HeadingEvent::Move { kind: dragged.0, to: key, after: after(&response) });
    }
    let response = response
        .on_hover_cursor(if being_dragged { egui::CursorIcon::Grabbing } else { egui::CursorIcon::PointingHand })
        .on_hover_text(tl!("Click to sort, drag to move the column. Drag its edge to resize it."));
    response.context_menu(|ui| {
        if ui.button(tl!("Reset columns")).clicked() {
            event = Some(HeadingEvent::Reset);
            ui.close();
        }
    });
    if response.clicked() {
        event = Some(HeadingEvent::Sort(key));
    }
    event
}

/// Draw the Combine files table body: reorder handles, editable page ranges with validation, file metadata
/// and per-row warnings.
#[allow(clippy::too_many_arguments)]
fn table_body(
    app: &mut PdfCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    rows: &Rows,
    widths: [f32; 5],
    events: &mut TableEvents,
    heading_event: &mut Option<HeadingEvent>,
    shown: &mut Option<[f32; 5]>,
) {
    let n = app.combine_draft.len();
    let order = app.combine_columns.order;
    // The heading being dragged follows the pointer.
    if let (Some(dragged), Some(pos)) = (egui::DragAndDrop::payload::<HeadingDrag>(ui.ctx()), ui.ctx().pointer_latest_pos()) {
        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("combine-heading-drag")));
        let galley = painter.layout_no_wrap(dragged.0.label().to_owned(), theme::medium(12.5), t.text);
        let r = Rect::from_min_size(pos + vec2(10.0, 8.0), galley.size() + vec2(16.0, 8.0));
        painter.rect(r, CornerRadius::same(4), t.card, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        painter.galley(r.min + vec2(8.0, 4.0), galley, t.text);
    }
    let mut builder = TableBuilder::new(ui)
        // Columns moved, reset or refitted: a new table state, built from `widths`.
        .id_salt(("combine-files", app.combine_tab.layout))
        .striped(true)
        .sense(Sense::click())
        .auto_shrink([false, true])
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::exact(18.0));
    for kind in order {
        builder = builder.column(Column::initial(widths[kind.index()]).at_least(kind.min_width()).resizable(true).clip(true));
    }
    builder
        .header(26.0, |mut row| {
            row.col(|_| {});
            for kind in order {
                row.col(|ui| {
                    if let Some(e) = heading(ui, t, kind, rows.sort) {
                        *heading_event = Some(e);
                    }
                });
            }
        })
        .body(|mut body| {
            if !body.ui_mut().is_sizing_pass() {
                let w = body.widths();
                let mut out = widths;
                for (kind, width) in order.iter().zip(w.iter().skip(1)) {
                    out[kind.index()] = *width;
                }
                *shown = Some(out);
            }
            for i in 0..n {
                let Some(f) = app.combine_draft.get_mut(i) else { break };
                let check = rows.checks.get(i).cloned().unwrap_or(Ok(f.pages));
                body.row(32.0, |mut row| {
                    row.set_selected(rows.selected.get(i).copied().unwrap_or(false));
                    // Drag handle.
                    row.col(|ui| {
                        let id = ui.scope_id().with(("combine-drag", f.id));
                        ui.dnd_drag_source(id, i, |ui| {
                            let (rect, resp) = ui.allocate_exact_size(vec2(12.0, 18.0), Sense::hover());
                            grip(ui, rect, if resp.hovered() || rows.dragging { t.text_muted } else { t.text_faint });
                        })
                        .response
                        .on_hover_cursor(egui::CursorIcon::Grab)
                        .on_hover_text(tl!("Drag to reorder"));
                    });
                    for kind in order {
                        row.col(|ui| match kind {
                            SortKey::Name => {
                                ui.add(icons::image("file-text", 16.0, t.icon));
                                // A file name is file/path chrome: explicitly LTR, extension last.
                                crate::widgets::label_ltr_truncated(ui, f.name.clone(), theme::regular(13.0), t.text);
                            }
                            // The range field and how many pages it takes.
                            SortKey::Pages => {
                                crate::widgets::begin_dialog(ui);
                                let edit = ui.add_enabled(
                                    f.problem.is_none(),
                                    crate::widgets::line(&mut f.range).frame(egui::Frame::NONE).hint_text(tl!("All pages")).desired_width(88.0),
                                );
                                // A visible field on plain and striped rows alike; red when the range is wrong.
                                let border = if check.is_err() { Stroke::new(1.5, ERROR) } else { Stroke::new(1.0, t.border) };
                                ui.painter().rect_stroke(edit.rect, CornerRadius::same(4), border, egui::StrokeKind::Inside);
                                if let Err(e) = &check {
                                    edit.clone().on_hover_text(e);
                                } else {
                                    edit.clone()
                                        .on_hover_text(crate::i18n::fmt(tl!("Pages of {name} to combine, e.g. 1-3, 6"), &[("name", &f.name)]));
                                }
                                if edit.gained_focus() {
                                    events.range_focused = true;
                                    events.action = Some(RowAction::Click(i, Modifiers::NONE));
                                }
                                if edit.changed() {
                                    events.range_changed = true;
                                }
                                let count = match (&check, f.problem.is_some()) {
                                    (_, true) => "—".to_string(),
                                    (Ok(k), _) => crate::i18n::fmt(tl!("{k} of {n}"), &[("k", &k.to_string()), ("n", &f.pages.to_string())]),
                                    (Err(_), _) => crate::i18n::fmt(tl!("? of {n}"), &[("n", &f.pages.to_string())]),
                                };
                                ui.label(egui::RichText::new(count).small().color(if check.is_err() { ERROR } else { t.text_faint }));
                            }
                            SortKey::Size => {
                                ui.label(egui::RichText::new(crate::panels::human_size(f.bytes.len())).color(t.text_muted));
                            }
                            SortKey::Modified => match f.modified {
                                Some(m) => {
                                    ui.label(egui::RichText::new(ago(m)).color(t.text_muted)).on_hover_text(utc_stamp(m));
                                }
                                None => {
                                    ui.label(egui::RichText::new("—").color(t.text_faint));
                                }
                            },
                            // Blocking problems first, in red.
                            SortKey::Warnings => {
                                let mut errors: Vec<String> = f.problem.iter().cloned().collect();
                                if let Err(e) = &check {
                                    errors.push(e.clone());
                                }
                                let mut warnings = f.notes.clone();
                                if rows.twice.get(i).copied().unwrap_or(false) {
                                    warnings.push(tl!("Added more than once").to_string());
                                }
                                if let Some(renames) = rows.conflicts.get(i).filter(|r| !r.is_empty()) {
                                    let shown: Vec<String> = renames.iter().take(3).map(|(old, new)| format!("{old} → {new}")).collect();
                                    let list = shown.join(", ");
                                    warnings.push(crate::i18n::fmt(
                                        &crate::i18n::trn(
                                            crate::i18n::current(),
                                            renames.len() as u64,
                                            "{n} field name collides with earlier files and will be renamed: {list}",
                                            "{n} field names collide with earlier files and will be renamed: {list}",
                                        ),
                                        &[("list", &list)],
                                    ));
                                }
                                let (icon, colour, first) = match (errors.first(), warnings.first()) {
                                    (Some(e), _) => ("circle-x", ERROR, e.clone()),
                                    (None, Some(w)) => ("triangle-alert", WARNING, w.clone()),
                                    (None, None) => return,
                                };
                                let all = errors.iter().chain(&warnings).cloned().collect::<Vec<_>>().join("\n");
                                ui.add(icons::image(icon, 15.0, colour));
                                if f.lock.is_some() {
                                    let link =
                                        ui.add(egui::Link::new(egui::RichText::new(tl!("Unlock…")).color(t.accent))).on_hover_text(match f.lock {
                                            Some(Lock::Permissions) => tl!("Enter the permissions password to allow combining"),
                                            _ => tl!("Enter the password that opens it"),
                                        });
                                    if link.clicked() {
                                        events.action = Some(RowAction::Unlock(Some(i), link.rect.left_bottom() + vec2(0.0, 6.0)));
                                    }
                                }
                                ui.add(
                                    egui::Label::new(egui::RichText::new(first).color(if colour == ERROR { ERROR } else { t.text }))
                                        .truncate()
                                        .selectable(false),
                                )
                                .on_hover_text(all);
                            }
                        });
                    }
                    let response = row.response();
                    if f.lock.is_some() {
                        let at = response.rect.left_bottom();
                        response.context_menu(|ui| {
                            if ui
                                .add(egui::Button::image_and_text(icons::image("lock-open", 15.0, ui.visuals().text_color()), tl!("Unlock…")))
                                .clicked()
                            {
                                events.action = Some(RowAction::Unlock(Some(i), at));
                                ui.close();
                            }
                        });
                    }
                    if response.clicked() {
                        let modifiers = response.ctx.input(|i| i.modifiers);
                        events.action = Some(RowAction::Click(i, modifiers));
                    }
                    // A row dragged over this one goes where the line is drawn.
                    if let Some(from) = response.dnd_hover_payload::<usize>().map(|p| *p) {
                        let r = response.rect;
                        let y = if from > i { r.top() } else { r.bottom() };
                        if from != i {
                            response.ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("combine-drop-line"))).hline(
                                r.x_range(),
                                y,
                                Stroke::new(2.0, t.accent),
                            );
                        }
                    }
                    if let Some(from) = response.dnd_release_payload::<usize>().map(|p| *p) {
                        events.action = Some(RowAction::Drop { from, to: i });
                    }
                });
            }
        });
}

/// Six dots: the drag handle.
fn grip(ui: &egui::Ui, rect: Rect, colour: Color32) {
    for dx in [-2.5, 2.5] {
        for dy in [-5.0, 0.0, 5.0] {
            ui.painter().circle_filled(rect.center() + vec2(dx, dy), 1.4, colour);
        }
    }
}
