//! Filling in forms on the page (execution plan M6; Acrobat fills fields in every viewing mode).
//!
//! Text fields open an in-place editor over the widget (Enter or clicking away commits, Escape
//! cancels, Tab moves to the next field). Check boxes and radio buttons toggle on click. Combo
//! and list boxes open a list of their options. Every change is one undoable engine edit.

use std::collections::HashMap;

use egui::{Color32, CornerRadius, Rect, Stroke, vec2};
use pdfcraft_engine::{Edit, FieldValue, FillMark, FormField, FormFieldKind, NewAnnotation, Shape, Style, field_flags};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageXform};
use crate::theme::Tokens;

const FOCUS_BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);

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
    /// Date fields: the month the calendar shows (year, month 1–12), and where it was drawn.
    pub calendar: Option<(i32, u32)>,
    pub calendar_rect: Option<egui::Rect>,
}

#[derive(Clone, Debug, Default)]
pub struct FormView {
    pub focus: Option<Focus>,
    /// The draft whose edit is queued (`DocView::pending_edit`): if the field refuses the value,
    /// the editor reopens with it rather than losing the typing.
    pub(crate) committed: Option<Focus>,
    /// A message for the app to show (e.g. "buttons run JavaScript").
    pub notice: Option<FormNotice>,
    /// A push button was clicked: (its field name, what it does).
    pub button: Option<(String, pdfcraft_engine::form_scripts::ButtonAction)>,
    /// Toggle-field popup target (name, widget) and its last rect (gap stickiness).
    pub offer: Option<(String, usize)>,
    pub offer_rect: Option<Rect>,
    /// Printed squares per page (user space), detected lazily.
    pub flat: HashMap<usize, Vec<[f64; 4]>>,
    /// Document generation `flat` was detected at.
    pub flat_gen: u64,
    /// Toggle waiting behind a committing text draft.
    pub queued: Option<Edit>,
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
        calendar: None,
        calendar_rect: None,
    }
}

/// Clicks on form widgets of one page. Returns `true` when the click belongs to a field.
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
) -> bool {
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let Some(p) = pointer.filter(|p| xf.rect.contains(*p)) else { return false };
    let hit = form.iter().find_map(|f| {
        f.widgets
            .iter()
            .enumerate()
            .find(|(_, w)| w.page == Some(page) && !w.hidden && widget_rect(xf, info, page, w.rect).contains(p))
            .map(|(i, w)| (f, i, w))
    });
    let Some((f, wi, w)) = hit else { return false };
    // Empty signature fields are signed by clicking them (Use a certificate); push buttons run
    // their action.
    let pressable = f.kind == FormFieldKind::PushButton && !f.read_only() && f.button.is_some();
    let usable = allowed && (fillable(f) || pressable || f.kind == FormFieldKind::Signature);
    ui.ctx().set_cursor_icon(match (usable, f.kind) {
        (false, _) => egui::CursorIcon::NotAllowed,
        (true, FormFieldKind::Text) => egui::CursorIcon::Text,
        _ => egui::CursorIcon::PointingHand,
    });
    if !resp.clicked() {
        return resp.is_pointer_button_down_on();
    }
    if !allowed {
        view.forms.notice = Some(FormNotice::Security);
        return true;
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
        FormFieldKind::Text | FormFieldKind::Combo | FormFieldKind::List => {
            if view.forms.focus.as_ref().is_none_or(|x| x.name != f.name || x.widget != wi) {
                commit(view, form);
                view.forms.focus = Some(open_focus(f, wi));
            }
        }
    }
    true
}

/// The edit the focused field's draft makes, if it differs from the field's value.
pub(crate) fn draft_edit(focus: &Focus, form: &[FormField]) -> Option<Edit> {
    let f = form.iter().find(|f| f.name == focus.name)?;
    let value = match f.kind {
        FormFieldKind::Text if f.value.first().map(String::as_str).unwrap_or("") != focus.text => FieldValue::Text(focus.text.clone()),
        FormFieldKind::List if f.has(field_flags::MULTI_SELECT) && focus.picked != f.value => FieldValue::Choice(focus.picked.clone()),
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
    let (mut y, mut m) = fx.calendar.unwrap_or_else(|| match parse_date(&fx.text, fmt) {
        Some(d) => (d.y, d.m),
        None => (today.0 as i32, today.1),
    });
    let current = parse_date(&fx.text, fmt);
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
                        ui.label(egui::RichText::new(d).small());
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
            } else if fillable(f) && pointer.is_some_and(|p| r.contains(p)) {
                painter.rect_stroke(r, CornerRadius::same(1), Stroke::new(1.0, FOCUS_BLUE.gamma_multiply(0.8)), egui::StrokeKind::Outside);
            }
        }
    }
}

/// Hovering a toggle field offers Check/Uncheck (Select/Deselect) in a popup.
pub(crate) fn hover_offer(ctx: &egui::Context, view: &mut DocView, info: &DocInfo, form: &[FormField], allowed: bool) -> Option<Edit> {
    widget_offer(ctx, view, info, form, allowed).flatten()
}

/// The toggle-field popup; `Some(None)` means visible but unclicked.
fn widget_offer(ctx: &egui::Context, view: &mut DocView, info: &DocInfo, form: &[FormField], can_fill: bool) -> Option<Option<Edit>> {
    let pointer = ctx.input(|i| i.pointer.hover_pos());
    // The widget under the cursor, when it is a toggle field we can fill.
    let hovered: Option<(String, usize)> = pointer.and_then(|p| {
        form.iter().find_map(|f| {
            if !matches!(f.kind, FormFieldKind::CheckBox | FormFieldKind::Radio) || !fillable(f) {
                return None;
            }
            f.widgets.iter().enumerate().find_map(|(wi, w)| {
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
    let mut out = None;
    let area = egui::Area::new(egui::Id::new(("form-hover-offer", view.id.0, &name, wi)))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.right_top() + vec2(6.0, -4.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(120.0);
                ui.label(egui::RichText::new(&name).small().color(Tokens::get(ui.ctx()).text_faint));
                if ui.add_enabled(enabled, egui::Button::new(tl!(label))).clicked() {
                    out = edit.map(|value| Edit::SetFieldValue { name: name.clone(), value });
                }
            });
        });
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

/// Whether a printed square contains the pointer (user space, 2 pt pad).
fn square_at(squares: &[[f64; 4]], at: [f64; 2]) -> Option<usize> {
    squares.iter().position(|s| at[0] >= s[0] - 2.0 && at[0] <= s[2] + 2.0 && at[1] >= s[1] - 2.0 && at[1] <= s[3] + 2.0)
}

/// A placed check on the square (position in `/Annots`); only `PCCheck` counts.
fn square_mark(info: &DocInfo, page: usize, square: [f64; 4]) -> Option<usize> {
    info.annotations
        .iter()
        .filter(|a| a.page == page && a.subtype == "Stamp" && a.stamp.as_deref() == Some(FillMark::Check.name()))
        .find(|a| {
            let r = [f64::from(a.rect[0]), f64::from(a.rect[1]), f64::from(a.rect[2]), f64::from(a.rect[3])];
            r[0] >= square[0] - 3.0 && r[1] >= square[1] - 3.0 && r[2] <= square[2] + 3.0 && r[3] <= square[3] + 3.0
        })
        .map(|a| a.index)
}

/// Hovering a printed square shows a hand and a check chip; clicking toggles a
/// Fill & Sign check on it. Works with no Prepare-a-form step.
#[allow(clippy::too_many_arguments)]
pub(crate) fn flat_page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    form: &[FormField],
    view: &mut DocView,
    allowed: bool,
    author: &str,
    generation: u64,
    detect: &dyn Fn(usize) -> Vec<[f64; 4]>,
) -> bool {
    // A printed square must not take over a drag or swallow its release.
    if view.comments.gesture.is_some() || resp.dragged() || resp.drag_stopped() {
        return false;
    }
    if generation != view.forms.flat_gen {
        view.forms.flat.clear();
        view.forms.flat_gen = generation;
    }
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let Some(p) = pointer.filter(|p| xf.rect.contains(*p)) else { return false };
    // AcroForm widgets own their clicks.
    let on_widget = form.iter().flat_map(|f| &f.widgets).any(|w| w.page == Some(page) && widget_rect(xf, info, page, w.rect).contains(p));
    if on_widget {
        return false;
    }
    view.forms.flat.entry(page).or_insert_with(|| detect(page));
    let (vx, vy) = xf.screen_to_view(p);
    let at = info.pages.get(page).map(|pi| {
        let u = pi.view_to_user(vx, vy);
        [f64::from(u[0]), f64::from(u[1])]
    });
    let hit = at.and_then(|at| view.forms.flat.get(&page).and_then(|squares| square_at(squares, at).map(|i| squares[i])));
    let Some(sq) = hit else { return false };
    // A placed check toggles off again; real comments sitting on a square are left alone.
    let placed = square_mark(info, page, sq);
    if !allowed {
        ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
        if resp.clicked() {
            view.forms.notice = Some(FormNotice::Security);
            return true;
        }
        return true;
    }
    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    // Claim the gesture so the ballot glyph underneath can't force an I-beam.
    if !resp.clicked() {
        // Check chip following the pointer (the OS cursor set has no check).
        if placed.is_none() {
            egui::Area::new(egui::Id::new(("form-check-cursor", view.id.0))).order(egui::Order::Foreground).fixed_pos(p + vec2(14.0, 18.0)).show(
                ui.ctx(),
                |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        let (rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::hover());
                        crate::icons::paint(ui, rect, "check", 16.0, crate::theme::Tokens::get(ui.ctx()).accent);
                    });
                },
            );
        }
        return true;
    }
    // Preserve a draft open elsewhere before placing or removing the check.
    // Queue the annotation edit so it cannot overwrite the draft's commit.
    commit(view, form);
    let queued = view.pending_edit.is_some();
    if let Some(index) = placed {
        let delete = Edit::DeleteAnnotation { page, index };
        if queued {
            view.forms.queued = Some(delete);
        } else {
            view.pending_edit = Some(delete);
        }
        return true;
    }
    let shape = Shape::Mark { rect: sq, mark: FillMark::Check };
    let style = Style::default_for(&shape);
    let check = Edit::AddAnnotation(NewAnnotation { page, shape, style, contents: String::new(), author: author.to_string() });
    if queued {
        view.forms.queued = Some(check);
    } else {
        view.pending_edit = Some(check);
    }
    true
}

/// The in-place editor or option list for the focused field. Returns an edit to apply.
pub(crate) fn overlay(ctx: &egui::Context, view: &mut DocView, info: &DocInfo, form: &[FormField], today: (i64, u32, u32)) -> Option<Edit> {
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
            let da = f.da.split_whitespace().collect::<Vec<_>>();
            let size =
                da.iter().position(|x| *x == "Tf").and_then(|i| da.get(i.wrapping_sub(1))).and_then(|s| s.parse::<f32>().ok()).filter(|s| *s > 0.0);
            let multiline = f.has(field_flags::MULTILINE);
            let font = (size.unwrap_or(((w.rect[3] - w.rect[1]) as f32 * 0.6).clamp(6.0, 12.0)) * zoom).clamp(6.0, 64.0);
            egui::Area::new(egui::Id::new(("form-editor", view.id.0))).order(egui::Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
                ui.set_min_size(rect.size());
                let Some(fx) = view.forms.focus.as_mut() else { return };
                let mut te = if multiline { egui::TextEdit::multiline(&mut fx.text) } else { egui::TextEdit::singleline(&mut fx.text) };
                te = te
                    .desired_width(rect.width() - 6.0)
                    .font(egui::FontId::proportional(font))
                    .background_color(Color32::from_rgb(0xFF, 0xFF, 0xF4))
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
                let r = ui.add_sized(rect.size(), te);
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
                    // Enter in a single-line field, or clicking elsewhere, commits.
                    commit(view, form);
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
        FormFieldKind::Combo | FormFieldKind::List => {
            let multi = f.kind == FormFieldKind::List && f.has(field_flags::MULTI_SELECT);
            let resp = egui::Area::new(egui::Id::new(("form-choices", view.id.0)))
                .order(egui::Order::Foreground)
                .fixed_pos(rect.left_bottom() + vec2(0.0, 2.0))
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(rect.width().max(160.0));
                        ui.label(egui::RichText::new(&f.name).small().color(t.text_faint));
                        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                            let Some(fx) = view.forms.focus.as_mut() else { return };
                            for (export, display) in &f.options {
                                let on = if multi { fx.picked.contains(export) } else { f.value.first() == Some(export) };
                                if multi {
                                    let mut c = on;
                                    if ui.checkbox(&mut c, display).changed() {
                                        if c {
                                            fx.picked.push(export.clone());
                                        } else {
                                            fx.picked.retain(|x| x != export);
                                        }
                                    }
                                } else if ui.selectable_label(on, display).clicked() {
                                    edit = Some(Edit::SetFieldValue { name: f.name.clone(), value: FieldValue::Choice(vec![export.clone()]) });
                                    close = true;
                                }
                            }
                        });
                        if multi && ui.button(tl!("Done")).clicked() {
                            commit(view, form);
                        }
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

/// Where a printed square is on screen (tests).
pub fn square_screen_rect(view: &DocView, info: &DocInfo, page: usize, sq: [f64; 4]) -> Option<Rect> {
    let xf = view.page_xform(page)?;
    Some(xf.user_rect(info, page, [sq[0] as f32, sq[1] as f32, sq[2] as f32, sq[3] as f32]))
}
