//! Filling in forms on the page (execution plan M6; Acrobat fills fields in every viewing mode).
//!
//! Text fields open an in-place editor over the widget (Enter or clicking away commits, Escape
//! cancels, Tab moves to the next field). Check boxes and radio buttons toggle on click. List
//! boxes are directly selectable in-place on the page (clicking picks an option; Ctrl-click
//! toggles on multi-select lists). Combo boxes open a list of their options. Every change is one
//! undoable engine edit.

use std::collections::HashSet;

use egui::{Color32, CornerRadius, Rect, Stroke, vec2};
use pdfcraft_engine::{Edit, FieldValue, FormField, FormFieldKind, FormWidget, field_flags, form_appearance};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageXform};
use crate::theme::Tokens;

const FOCUS_BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);
/// Background of the in-place text field editor, the same in every theme.
const FIELD_FILL: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xF4);

/// The field being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct Focus {
    pub name: String,
    /// Which of the field's widgets (a field can appear on several pages).
    pub widget: usize,
    /// Text being typed (text fields).
    pub text: String,
    /// Choices picked so far (list boxes with multiple selection).
    pub picked: Vec<String>,
    pub request_focus: bool,
    /// Select the whole value when the editor opens (entered with Tab), so typing replaces it.
    pub select_all: bool,
    /// Where the press that opened the editor landed: the editor puts its caret there and takes
    /// over the drag, so press-and-drag selects text in a field that was not focused (#2213).
    pub press: Option<egui::Pos2>,
    /// Date fields: the month the calendar shows (year, month 1–12), and where it was drawn.
    pub calendar: Option<(i32, u32)>,
    pub calendar_rect: Option<egui::Rect>,
    /// The text the calendar last saw, so it moves to a typed date once, when the typing becomes
    /// a whole date, and otherwise keeps the month its arrows turned to.
    pub calendar_text: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct FormView {
    pub focus: Option<Focus>,
    /// The draft whose edit is queued (`DocView::pending_edit`): if the field refuses the value,
    /// the editor reopens with it rather than losing the typing.
    pub(crate) committed: Option<Focus>,
    /// A press on a text field committed the field before it (#2213). Until the button is
    /// released, that field losing focus to the same gesture must not commit it again: a
    /// refused value would be validated (and alerted) twice.
    pub(crate) pressed: bool,
    /// Fields modified by the user whose appearance stream has not been rasterized by the render worker yet.
    /// Tracked as (field_name, page_index) so committed edits stay immediately visible without flicker.
    pub unbaked: HashSet<(String, usize)>,
    /// A message for the app to show (e.g. "buttons run JavaScript").
    pub notice: Option<FormNotice>,
    /// A push button was clicked: (its field name, what it does).
    pub button: Option<(String, pdfcraft_engine::form_scripts::ButtonAction)>,
}

/// A form message for the app to show, with document data kept separate so the visible
/// text follows the UI language.
#[derive(Clone, Debug)]
pub enum FormNotice {
    /// Filling is blocked by the document's security settings.
    Security,
    /// A read-only field was clicked (its name).
    ReadOnly(String),
    /// A push button with no action was clicked (its name).
    NoAction(String),
}

fn widget_rect(xf: &PageXform, info: &DocInfo, page: usize, r: [f64; 4]) -> Rect {
    xf.user_rect(info, page, [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32])
}

fn fillable(f: &FormField) -> bool {
    !f.read_only() && !matches!(f.kind, FormFieldKind::PushButton | FormFieldKind::Signature)
}

fn open_focus(f: &FormField, widget: usize) -> Focus {
    Focus {
        name: f.name.clone(),
        widget,
        text: f.value.first().cloned().unwrap_or_default(),
        picked: f.value.clone(),
        request_focus: true,
        select_all: false,
        press: None,
        calendar: None,
        calendar_rect: None,
        calendar_text: None,
    }
}

/// Where the pointer is relative to one page's form widgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FieldPointer {
    /// Not over a field.
    Away,
    /// Over a field, which has set the cursor. Nothing is pressed, so the layers below still see
    /// the hover, but they must not replace the cursor: a check box drawn with a glyph, or set
    /// on printed text, is a button, not text to select.
    Over,
    /// The field owns the press or click.
    Owned,
}

/// Clicks on form widgets of one page.
#[allow(clippy::too_many_arguments)]
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    form: &[FormField],
    allowed: bool,
    view: &mut DocView,
) -> FieldPointer {
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let Some(p) = pointer.filter(|p| xf.rect.contains(*p)) else { return FieldPointer::Away };
    let hit = form.iter().find_map(|f| {
        f.widgets
            .iter()
            .enumerate()
            .find(|(_, w)| w.page == Some(page) && !w.hidden && widget_rect(xf, info, page, w.rect).contains(p))
            .map(|(i, w)| (f, i, w))
    });
    let Some((f, wi, w)) = hit else { return FieldPointer::Away };
    // Empty signature fields are signed by clicking them (Use a certificate); push buttons run
    // their action.
    let pressable = f.kind == FormFieldKind::PushButton && !f.read_only() && f.button.is_some();
    let usable = allowed && (fillable(f) || pressable || f.kind == FormFieldKind::Signature);
    ui.ctx().set_cursor_icon(match (usable, f.kind) {
        (false, _) => egui::CursorIcon::NotAllowed,
        (true, FormFieldKind::Text) => egui::CursorIcon::Text,
        _ => egui::CursorIcon::PointingHand,
    });
    // A text field opens on the press rather than the click, so a drag that starts on it
    // selects its text (#2213).
    let pressed = f.kind == FormFieldKind::Text && usable && resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed());
    if pressed && view.forms.focus.as_ref().is_none_or(|x| x.name != f.name || x.widget != wi) {
        commit(view, form);
        view.forms.focus = Some(Focus { press: Some(p), ..open_focus(f, wi) });
        view.forms.pressed = true;
    }
    if !resp.clicked() {
        return if resp.is_pointer_button_down_on() { FieldPointer::Owned } else { FieldPointer::Over };
    }
    if !allowed {
        view.forms.notice = Some(FormNotice::Security);
        return FieldPointer::Owned;
    }
    match f.kind {
        _ if f.read_only() => view.forms.notice = Some(FormNotice::ReadOnly(f.name.clone())),
        FormFieldKind::PushButton => match &f.button {
            Some(a) => view.forms.button = Some((f.name.clone(), a.clone())),
            None => view.forms.notice = Some(FormNotice::NoAction(f.name.clone())),
        },
        FormFieldKind::Signature => view.sign.field = Some(f.name.clone()),
        FormFieldKind::CheckBox => {
            view.forms.focus = None;
            view.pending_edit = Some(Edit::SetFieldValue { name: f.name.clone(), value: FieldValue::Check(f.value.is_empty()) });
        }
        FormFieldKind::Radio => {
            view.forms.focus = None;
            let on = w.on_state.clone();
            // Clicking the selected button again turns it off unless the group forbids it.
            let value = if f.value.first() == on.as_ref() && !f.has(field_flags::NO_TOGGLE_TO_OFF) { None } else { on };
            if value.as_ref() != f.value.first() {
                view.pending_edit = Some(Edit::SetFieldValue { name: f.name.clone(), value: FieldValue::Radio(value) });
            }
        }
        // The press already committed the previous field and opened this one (above), unless
        // press and release came in one frame. Committing again would re-run a validation that
        // refused the value on the press.
        FormFieldKind::Text if !ui.input(|i| i.pointer.primary_pressed()) => {
            // An editor opened by the press cannot be hit until a frame has laid it out: the
            // release lands on the page and would take its focus away.
            if let Some(x) = view.forms.focus.as_mut().filter(|x| x.name == f.name && x.widget == wi) {
                x.request_focus = true;
            }
        }
        FormFieldKind::Text | FormFieldKind::Combo => {
            if view.forms.focus.as_ref().is_none_or(|x| x.name != f.name || x.widget != wi) {
                commit(view, form);
                view.forms.focus = Some(open_focus(f, wi));
            }
        }
        FormFieldKind::List => {
            commit(view, form);
            view.forms.focus = Some(open_focus(f, wi));
            let r = widget_rect(xf, info, page, w.rect);
            let height_pt = if matches!(w.rotation, 90 | 270) { (w.rect[2] - w.rect[0]).max(1.0) } else { (w.rect[3] - w.rect[1]).max(1.0) };
            let zoom = (r.height() as f64) / height_pt;
            let y_pt = (p.y - r.min.y) as f64 / zoom;
            let rows = w.list.unwrap_or(form_appearance::ListRows { top: 0, inset: 2.0, row: 12.0 * 1.15, size: 12.0 });
            let picked_indices: Vec<usize> = f.value.iter().filter_map(|v| f.options.iter().position(|(e, _)| e == v)).collect();
            let top = form_appearance::ListRows::top_showing(rows.top, &picked_indices, rows.visible(height_pt), f.options.len());
            if let Some(opt_idx) = rows.option_at(top, y_pt).filter(|&i| i < f.options.len()) {
                let export = &f.options[opt_idx].0;
                let ctrl = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
                let shift = ui.input(|i| i.modifiers.shift);
                let multi = f.has(field_flags::MULTI_SELECT);
                let new_value = if multi && ctrl {
                    let mut picked = f.value.clone();
                    if let Some(pos) = picked.iter().position(|x| x == export) {
                        picked.remove(pos);
                    } else {
                        picked.push(export.clone());
                    }
                    FieldValue::Choice(picked)
                } else if multi && shift && !f.value.is_empty() {
                    let anchor = f.value.last().and_then(|last| f.options.iter().position(|(e, _)| e == last)).unwrap_or(opt_idx);
                    let (min, max) = (anchor.min(opt_idx), anchor.max(opt_idx));
                    let picked = f.options[min..=max].iter().map(|(e, _)| e.clone()).collect();
                    FieldValue::Choice(picked)
                } else {
                    FieldValue::Choice(vec![export.clone()])
                };
                if new_value != FieldValue::Choice(f.value.clone()) {
                    view.pending_edit = Some(Edit::SetFieldValue { name: f.name.clone(), value: new_value });
                }
            }
        }
    }
    FieldPointer::Owned
}

/// The edit the focused field's draft makes, if it differs from the field's value.
pub(crate) fn draft_edit(focus: &Focus, form: &[FormField]) -> Option<Edit> {
    let f = form.iter().find(|f| f.name == focus.name)?;
    let value = match f.kind {
        FormFieldKind::Text if f.value.first().map(String::as_str).unwrap_or("") != focus.text => FieldValue::Text(focus.text.clone()),
        _ => return None,
    };
    Some(Edit::SetFieldValue { name: f.name.clone(), value })
}

/// Close the focused field's editor, queueing the edit for its draft if it differs from the
/// field's value.
pub(crate) fn commit(view: &mut DocView, form: &[FormField]) {
    let Some(focus) = view.forms.focus.take() else { return };
    if let Some(edit) = draft_edit(&focus, form) {
        view.pending_edit = Some(edit);
        view.forms.committed = Some(focus);
    }
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// The date picker under a focused date field. Returns the picked date, formatted with the
/// field's own format.
fn calendar(ctx: &egui::Context, view: &mut DocView, field: egui::Rect, fmt: &str, today: (i64, u32, u32)) -> Option<String> {
    use pdfcraft_engine::form_scripts::{DateTime, format_date, parse_date};
    let fx = view.forms.focus.as_mut()?;
    let current = parse_date(&fx.text, fmt);
    // A whole date typed since the last frame moves the calendar to it; half a date, or text
    // that hasn't changed, leaves it on the month it shows (the one the arrows turned to).
    let changed = fx.calendar_text.as_deref() != Some(fx.text.as_str());
    let (mut y, mut m) = match (fx.calendar, current) {
        (Some(_), Some(d)) if changed => (d.y, d.m),
        (Some(shown), _) => shown,
        (None, Some(d)) => (d.y, d.m),
        (None, None) => (today.0 as i32, today.1),
    };
    if changed {
        fx.calendar_text = Some(fx.text.clone());
    }
    let mut picked = None;
    let area = egui::Area::new(egui::Id::new(("date-picker", view.id.0)))
        .order(egui::Order::Foreground)
        .fixed_pos(field.left_bottom() + vec2(0.0, 2.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.small_button("‹").on_hover_text(tl!("Previous month")).clicked() {
                        (y, m) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
                    }
                    ui.label(
                        egui::RichText::new(crate::i18n::fmt(
                            tl!("{month} {y}"),
                            &[("month", tl!(MONTHS[(m.clamp(1, 12) - 1) as usize])), ("y", &y.to_string())],
                        ))
                        .strong(),
                    );
                    if ui.small_button("›").on_hover_text(tl!("Next month")).clicked() {
                        (y, m) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
                    }
                });
                // Weekday of the 1st (0 = Sunday), Zeller-style via days since 1970-01-01 (a Thursday).
                let days_from_civil = |y: i32, m: u32, d: u32| -> i64 {
                    let (y, m) = if m <= 2 { (y as i64 - 1, m as i64 + 9) } else { (y as i64, m as i64 - 3) };
                    let era = y.div_euclid(400);
                    let yoe = y - era * 400;
                    let doy = (153 * m + 2) / 5 + d as i64 - 1;
                    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
                };
                let first = (days_from_civil(y, m, 1) + 4).rem_euclid(7) as usize;
                let len = (days_from_civil(if m == 12 { y + 1 } else { y }, if m == 12 { 1 } else { m + 1 }, 1) - days_from_civil(y, m, 1)) as usize;
                egui::Grid::new(("date-grid", view.id.0)).spacing([4.0, 2.0]).show(ui, |ui| {
                    for d in ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"] {
                        ui.label(egui::RichText::new(tl_ctx!("calendar weekday", d)).small());
                    }
                    ui.end_row();
                    for cell in 0..(first + len).div_ceil(7) * 7 {
                        if cell >= first && cell < first + len {
                            let day = (cell - first + 1) as u32;
                            let selected = current.is_some_and(|c| (c.y, c.m, c.d) == (y, m, day));
                            let is_today = (today.0 as i32, today.1, today.2) == (y, m, day);
                            let mut text = egui::RichText::new(day.to_string());
                            if is_today {
                                text = text.strong();
                            }
                            if ui
                                .selectable_label(selected, text)
                                .on_hover_text(crate::i18n::fmt(
                                    tl!("{month} {day}, {y}"),
                                    &[
                                        ("month", tl_ctx!("calendar date", MONTHS[(m - 1) as usize])),
                                        ("day", &day.to_string()),
                                        ("y", &y.to_string()),
                                    ],
                                ))
                                .clicked()
                            {
                                picked = Some(format_date(DateTime { y, m, d: day, hh: 0, mm: 0, ss: 0 }, fmt));
                            }
                        } else {
                            ui.label("");
                        }
                        if cell % 7 == 6 {
                            ui.end_row();
                        }
                    }
                });
            });
        });
    if let Some(fx) = view.forms.focus.as_mut() {
        fx.calendar = Some((y, m));
        fx.calendar_rect = Some(area.response.rect);
    }
    picked
}

/// Paint focus and hover frames on a page.
pub(crate) fn paint_page(ui: &egui::Ui, painter: &egui::Painter, xf: &PageXform, page: usize, info: &DocInfo, form: &[FormField], view: &DocView) {
    let pointer = ui.input(|i| i.pointer.hover_pos());
    for f in form {
        for (wi, w) in f.widgets.iter().enumerate().filter(|(_, w)| w.page == Some(page) && !w.hidden) {
            let r = widget_rect(xf, info, page, w.rect);
            let focused = view.forms.focus.as_ref().is_some_and(|x| x.name == f.name && x.widget == wi);
            if focused {
                painter.rect_stroke(r.expand(1.0), CornerRadius::same(2), Stroke::new(2.0, FOCUS_BLUE), egui::StrokeKind::Outside);
            }
            if view.forms.unbaked.contains(&(f.name.clone(), page)) {
                paint_unbaked_widget(painter, xf, f, w, r);
            } else if fillable(f) && pointer.is_some_and(|p| r.contains(p)) {
                painter.rect_stroke(r, CornerRadius::same(1), Stroke::new(1.0, FOCUS_BLUE.gamma_multiply(0.8)), egui::StrokeKind::Outside);
            }
        }
    }
}

/// The size, in points, a text field shows `text` at on screen: its `/DA` size, or for auto size
/// (`0 Tf`) the page appearance's fit, measured in the font the screen draws with so the text
/// stays inside the box (the interface font runs wider than the page's Helvetica).
pub fn field_text_size(painter: &egui::Painter, f: &FormField, w: &FormWidget, text: &str) -> f32 {
    let da = f.da.split_whitespace().collect::<Vec<_>>();
    let fixed = da
        .iter()
        .position(|x| *x == "Tf")
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| da.get(i))
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0);
    let size = fixed.unwrap_or_else(|| {
        // The appearance's padding: 2 pt inside a 1 pt border.
        let pad = 3.0;
        let (width, height) = ((w.rect[2] - w.rect[0]).abs() - 2.0 * pad, (w.rect[3] - w.rect[1]).abs() - 2.0 * pad);
        // Comb cells hold one character each, so only the height limits them.
        let comb = f.has(field_flags::COMB) && f.max_len.is_some_and(|m| m > 0);
        let fit = if comb { "" } else { text };
        let font = |size: f64| egui::FontId::proportional(size as f32);
        let line_width = |size: f64| f64::from(painter.layout_no_wrap(fit.to_string(), font(size), Color32::BLACK).size().x);
        // Wrapped as the editor and the page preview wrap it.
        let wrap = width.max(1.0) as f32;
        let wrapped_height = |size: f64| f64::from(painter.layout(fit.to_string(), font(size), Color32::BLACK, wrap).size().y);
        pdfcraft_engine::field_auto_text_size_by(width, height, f.has(field_flags::MULTILINE), line_width, wrapped_height)
    });
    size.min(300.0) as f32
}

fn paint_unbaked_widget(painter: &egui::Painter, xf: &PageXform, f: &FormField, w: &FormWidget, r: Rect) {
    match f.kind {
        FormFieldKind::Text | FormFieldKind::Combo => {
            let text = f.value.first().map(String::as_str).unwrap_or("");
            painter.rect_filled(r, CornerRadius::same(1), Color32::from_rgb(0xFF, 0xFF, 0xF4));
            painter.rect_stroke(r, CornerRadius::same(1), Stroke::new(1.0, Color32::from_gray(180)), egui::StrokeKind::Inside);
            if !text.is_empty() {
                let unrotated_w = if xf.rot % 180 == 90 { xf.ph } else { xf.pw };
                let zoom = xf.rect.width() / unrotated_w.max(1.0);
                let font_size = (field_text_size(painter, f, w, text) * zoom).clamp(3.0, 64.0);
                let display_text = if f.has(field_flags::PASSWORD) { "•".repeat(text.chars().count()) } else { text.to_string() };
                let p = painter.with_clip_rect(r);
                let (align, pos) = match f.quadding {
                    1 => (egui::Align2::CENTER_TOP, egui::pos2(r.center().x, r.min.y + 1.0)),
                    2 => (egui::Align2::RIGHT_TOP, egui::pos2(r.max.x - 3.0, r.min.y + 1.0)),
                    _ => (egui::Align2::LEFT_TOP, egui::pos2(r.min.x + 3.0, r.min.y + 1.0)),
                };
                // Multiline fields wrap at the box, as their page appearance does.
                // At the width the size was fitted to: the box less 3 pt each side, scaled.
                let wrap = if f.has(field_flags::MULTILINE) { (r.width() - 6.0 * zoom).max(1.0) } else { f32::INFINITY };
                let galley = p.layout(display_text, egui::FontId::proportional(font_size), Color32::BLACK, wrap);
                p.galley(align.anchor_size(pos, galley.size()).min, galley, Color32::BLACK);
            }
        }
        FormFieldKind::List => {
            painter.rect_filled(r, CornerRadius::same(1), Color32::from_rgb(0xFF, 0xFF, 0xF4));
            painter.rect_stroke(r, CornerRadius::same(1), Stroke::new(1.0, Color32::from_gray(180)), egui::StrokeKind::Inside);
            let unrotated_w = if xf.rot % 180 == 90 { xf.ph } else { xf.pw };
            let zoom = (xf.rect.width() / unrotated_w.max(1.0)) as f64;
            let rows = w.list.unwrap_or(form_appearance::ListRows { top: 0, inset: 2.0, row: 12.0 * 1.15, size: 12.0 });
            let height_pt = if matches!(w.rotation, 90 | 270) { (w.rect[2] - w.rect[0]).max(1.0) } else { (w.rect[3] - w.rect[1]).max(1.0) };
            let picked_indices: Vec<usize> = f.value.iter().filter_map(|v| f.options.iter().position(|(e, _)| e == v)).collect();
            let top = form_appearance::ListRows::top_showing(rows.top, &picked_indices, rows.visible(height_pt), f.options.len());
            let font_size = (rows.size * zoom).clamp(6.0, 64.0) as f32;
            let row_h = (rows.row * zoom) as f32;
            let inset = (rows.inset * zoom) as f32;
            let p = painter.with_clip_rect(r);
            let mut y = r.min.y + inset;
            for (export, display) in f.options.iter().skip(top) {
                if y + row_h > r.max.y {
                    break;
                }
                let row_rect = Rect::from_min_size(egui::pos2(r.min.x + 1.0, y), egui::vec2(r.width() - 2.0, row_h));
                if f.value.contains(export) {
                    p.rect_filled(row_rect, CornerRadius::ZERO, Color32::from_rgb(0x99, 0xC0, 0xDC));
                }
                p.text(
                    egui::pos2(r.min.x + 3.0, y + font_size * 0.15),
                    egui::Align2::LEFT_TOP,
                    display,
                    egui::FontId::proportional(font_size),
                    Color32::BLACK,
                );
                y += row_h;
            }
        }
        FormFieldKind::CheckBox => {
            painter.rect_filled(r, CornerRadius::same(1), Color32::from_rgb(0xFF, 0xFF, 0xF4));
            painter.rect_stroke(r, CornerRadius::same(1), Stroke::new(1.0, Color32::from_gray(180)), egui::StrokeKind::Inside);
            let checked = f.value.first().is_some_and(|v| v != "Off" && !v.is_empty());
            if checked {
                let sz = (r.height() * 0.75).clamp(8.0, 32.0);
                painter.text(r.center(), egui::Align2::CENTER_CENTER, "✔", egui::FontId::proportional(sz), Color32::BLACK);
            }
        }
        FormFieldKind::Radio => {
            painter.rect_filled(r, CornerRadius::same(1), Color32::from_rgb(0xFF, 0xFF, 0xF4));
            painter.rect_stroke(r, CornerRadius::same(1), Stroke::new(1.0, Color32::from_gray(180)), egui::StrokeKind::Inside);
            let selected = f.value.first().map(String::as_str) == w.on_state.as_deref();
            if selected {
                let radius = (r.height().min(r.width()) * 0.25).clamp(2.0, 16.0);
                painter.circle_filled(r.center(), radius, Color32::BLACK);
            }
        }
        _ => {}
    }
}

/// The in-place editor or option list for the focused field. Returns an edit to apply.
pub(crate) fn overlay(ctx: &egui::Context, view: &mut DocView, info: &DocInfo, form: &[FormField], today: (i64, u32, u32)) -> Option<Edit> {
    let pressed = view.forms.pressed;
    if !ctx.input(|i| i.pointer.primary_down()) {
        view.forms.pressed = false;
    }
    let focus = view.forms.focus.clone()?;
    let Some(f) = form.iter().find(|f| f.name == focus.name) else {
        view.forms.focus = None;
        return None;
    };
    let w = f.widgets.get(focus.widget)?;
    let page = w.page?;
    let xf = view.page_xform(page)?;
    let rect = widget_rect(&xf, info, page, w.rect);
    let zoom = xf.rect.width() / xf.pw.max(1.0);
    let t = Tokens::get(ctx);
    let mut edit = None;
    let mut close = false;
    let mut next: Option<bool> = None; // Tab / Shift+Tab
    match f.kind {
        FormFieldKind::Text => {
            let multiline = f.has(field_flags::MULTILINE);
            egui::Area::new(egui::Id::new(("form-editor", view.id.0))).order(egui::Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
                ui.set_min_size(rect.size());
                let Some(fx) = view.forms.focus.as_mut() else { return };
                // Sized from the text being typed, so an auto-sized field shrinks to keep it in view.
                let font = (field_text_size(ui.painter(), f, w, &fx.text) * zoom).clamp(3.0, 64.0);
                // The field is pale yellow in every theme, so the caret must not follow the theme (pale in dark mode) (#655).
                ui.visuals_mut().text_cursor.stroke.color = crate::edit_text_ui::caret_color_on(FIELD_FILL);
                let mut te = if multiline { egui::TextEdit::multiline(&mut fx.text) } else { egui::TextEdit::singleline(&mut fx.text) };
                te = te
                    // Wraps (inside the 3 px side margins) at the width the size was fitted to:
                    // the box less 3 pt each side, scaled.
                    .desired_width((rect.width() - 6.0 * zoom + 6.0).max(1.0))
                    .font(egui::FontId::proportional(font))
                    .background_color(FIELD_FILL)
                    .text_color(Color32::BLACK)
                    .margin(egui::Margin::symmetric(3, 1))
                    .password(f.has(field_flags::PASSWORD))
                    .id(egui::Id::new(("form-field", view.id.0, &f.name, focus.widget)));
                if let Some(max) = f.max_len {
                    te = te.char_limit(max);
                }
                if multiline {
                    te = te.desired_rows(((rect.height() / (font * 1.3)).floor() as usize).max(1));
                }
                let mut laid_out = None;
                let r = ui.add_sized(rect.size(), |ui: &mut egui::Ui| {
                    let out = te.show(ui);
                    laid_out = Some((out.galley, out.galley_pos));
                    out.response.response
                });
                if let Some(press) = fx.press.take()
                    && let Some((galley, pos)) = laid_out
                    && let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), r.id)
                {
                    // The press went to the page, not to this editor, which did not exist yet.
                    let at = galley.cursor_from_pos(press - pos + vec2(galley.rect.left(), 0.0));
                    st.cursor.set_char_range(Some(egui::text::CCursorRange::one(at)));
                    st.store(ui.ctx(), r.id);
                    if ui.input(|i| i.pointer.primary_down()) {
                        ui.ctx().set_dragged_id(r.id);
                    }
                }
                if fx.request_focus {
                    r.request_focus();
                    fx.request_focus = false;
                    if std::mem::take(&mut fx.select_all)
                        && let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), r.id)
                    {
                        let n = fx.text.chars().count();
                        st.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(n))));
                        st.store(ui.ctx(), r.id);
                    }
                }
                let (enter, esc, tab, shift) = ui
                    .input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Tab), i.modifiers.shift));
                if esc {
                    close = true;
                } else if tab {
                    next = Some(!shift);
                } else if r.lost_focus()
                    && (!multiline || !enter)
                    && !fx.calendar_rect.is_some_and(|c| ui.input(|i| i.pointer.interact_pos().is_some_and(|p| c.contains(p))))
                {
                    if pressed {
                        // The field reopened with a refused value; keep it open and focused.
                        fx.request_focus = true;
                    } else {
                        // Enter in a single-line field, or clicking elsewhere, commits.
                        commit(view, form);
                    }
                }
            });
            // Date fields: a calendar under the field (Acrobat's date picker).
            if let pdfcraft_engine::form_scripts::Format::Date(fmt) = &f.actions.format
                && let Some(picked) = calendar(ctx, view, rect, fmt, today)
            {
                if let Some(fx) = view.forms.focus.as_mut() {
                    fx.text = picked;
                }
                commit(view, form);
                return view.pending_edit.take();
            }
        }
        FormFieldKind::Combo => {
            let resp = egui::Area::new(egui::Id::new(("form-choices", view.id.0)))
                .order(egui::Order::Foreground)
                .fixed_pos(rect.left_bottom() + vec2(0.0, 2.0))
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(rect.width().max(160.0));
                        ui.label(egui::RichText::new(&f.name).small().color(t.text_faint));
                        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                            for (export, display) in &f.options {
                                let on = f.value.first() == Some(export);
                                if ui.selectable_label(on, display).clicked() {
                                    edit = Some(Edit::SetFieldValue { name: f.name.clone(), value: FieldValue::Choice(vec![export.clone()]) });
                                    close = true;
                                }
                            }
                        });
                    });
                });
            let clicked_away = ctx.input(|i| i.pointer.any_click())
                && !resp.response.contains_pointer()
                && !ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p));
            let (tab, shift) = ctx.input(|i| (i.key_pressed(egui::Key::Tab), i.modifiers.shift));
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                close = true;
            } else if tab {
                next = Some(!shift);
            } else if clicked_away {
                commit(view, form);
                close = true;
            }
        }
        FormFieldKind::List => {
            let (up, down, esc, tab, shift) = ctx.input_mut(|i| {
                (
                    // Consumed, so the arrows move the selection without also scrolling the page.
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::Escape),
                    i.key_pressed(egui::Key::Tab),
                    i.modifiers.shift,
                )
            });
            if esc {
                close = true;
            } else if tab {
                next = Some(!shift);
            } else if (up || down) && !f.options.is_empty() {
                let current_idx = f.value.last().and_then(|v| f.options.iter().position(|(e, _)| e == v)).unwrap_or(0);
                let next_idx = if up { current_idx.saturating_sub(1) } else { (current_idx + 1).min(f.options.len().saturating_sub(1)) };
                if next_idx != current_idx && next_idx < f.options.len() {
                    let export = f.options[next_idx].0.clone();
                    edit = Some(Edit::SetFieldValue { name: f.name.clone(), value: FieldValue::Choice(vec![export]) });
                }
            }
            let clicked_away = ctx.input(|i| i.pointer.any_click()) && !ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p));
            if clicked_away {
                close = true;
            }
        }
        _ => close = true,
    }
    if close {
        view.forms.focus = None;
    }
    if let Some(forward) = next {
        commit(view, form);
        let pending = view.pending_edit.take();
        // The next widget that takes typing or a choice, in the document's tab order (each
        // page's /Tabs: rows, columns or structure), wrapping.
        let mut order: Vec<(usize, &FormField, usize)> = form
            .iter()
            .filter(|x| fillable(x) && matches!(x.kind, FormFieldKind::Text | FormFieldKind::Combo | FormFieldKind::List))
            .flat_map(|x| x.widgets.iter().enumerate().filter(|(_, w)| w.page.is_some() && !w.hidden).map(move |(wi, w)| (w.tab, x, wi)))
            .collect();
        order.sort_by_key(|o| o.0);
        if let Some(pos) = order
            .iter()
            .position(|(_, x, wi)| x.name == f.name && *wi == focus.widget)
            .or_else(|| order.iter().position(|(_, x, _)| x.name == f.name))
        {
            let k = if forward { (pos + 1) % order.len() } else { (pos + order.len() - 1) % order.len() };
            let (_, target, wi) = order[k];
            view.forms.focus = Some(Focus { select_all: true, ..open_focus(target, wi) });
            if let Some(p) = target.widgets[wi].page
                && view.page_screen_rect(p).is_none()
            {
                view.go_to_page(p);
            }
        }
        edit = edit.or(pending);
    }
    edit.or_else(|| view.pending_edit.take())
}

/// Where a field's widget is on screen (tests and automation).
pub fn field_screen_rect(view: &DocView, info: &DocInfo, f: &FormField, widget: usize) -> Option<Rect> {
    let w = f.widgets.get(widget)?;
    let page = w.page?;
    let xf = view.page_xform(page)?;
    Some(widget_rect(&xf, info, page, w.rect))
}
