//! Filling in forms on the page (execution plan M6; Acrobat fills fields in every viewing mode).
//!
//! Every shown widget is a part of the page ([`field_parts`]) that takes its presses ([`field_press`]) and, once
//! focused, its keys ([`field_key`]). Text fields open an in-place editor over the widget (Enter or clicking away
//! commits, Escape cancels, Tab moves to the next field). Check boxes and radio buttons toggle on click, or on
//! Space once Tab or a click has focused them. List boxes are directly selectable in-place on the page (clicking
//! picks an option; Ctrl-click toggles on multi-select lists). Combo boxes open a list of their options; an
//! editable one (Edit flag) also takes typed text in a box over the field. Every change is one undoable engine edit.

use std::collections::HashSet;

use craft::behavior::CursorIcon;
use craft::behavior::hit::HitShape;
use craft::behavior::intent::{Intent, NavMove};
use craft::behavior::press::PressPhase;
use craft::keymap::Mods;
use craft::text::ui::editor::TextSelection;
use craft::ui::primitives::{Draw, PaintShape, StrokeWidth};
use craft::ui::style::{Corners, DataColor, SurfaceKind};
use egui::{Color32, CornerRadius, Pos2, Rect, vec2};
use pdfcraft_cos::ObjRef;
use pdfcraft_engine::{Edit, FieldValue, FormField, FormFieldKind, FormWidget, field_flags, form_appearance};
use pdfcraft_render::DocInfo;

use crate::canvas::page_parts::{Owner, PageLayer, PagePart, PagePress};
use crate::canvas::{DocView, PageGeometry, PageSession, PageXform};

mod calendar;
mod choice_list;
pub(crate) mod editor;

use calendar::calendar;
use choice_list::choice_list;

/// The page's paper: what an unbaked widget is filled with until its appearance is rendered.
const PAPER: Color32 = Color32::WHITE;
/// Background of the in-place text field editor, the same in every theme (as production).
const FIELD_FILL: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xF4);
/// Border of the field being edited.
const FOCUS_BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);

/// The field being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct Focus {
    pub name: String,
    /// The focused field's object. Same-name twins are one field (ISO 32000-2 §12.7.3.1) but come
    /// as separate fields here, each with its own widgets and option order, and this says which
    /// one has the focus. `None` goes by the name: the first field with it.
    pub obj: Option<ObjRef>,
    /// Which of the field's widgets (a field can appear on several pages).
    pub widget: usize,
    /// Text being typed (text fields).
    pub text: String,
    /// Choices picked so far (list boxes with multiple selection).
    pub picked: Vec<String>,
    /// The selection in the text: the caret and anchor as char offsets. A press puts the caret where it landed and a
    /// drag moves it; entering with Tab selects the whole value, so typing replaces it.
    pub selection: TextSelection,
    /// Drop-downs: whether the option list is open.
    pub list_open: bool,
    /// Date fields: whether the calendar is open.
    pub calendar_open: bool,
}

#[derive(Clone, Debug, Default)]
pub struct FormView {
    pub focus: Option<Focus>,
    /// The draft whose edit is queued (`DocView::pending_edit`): if the field refuses the value,
    /// the editor reopens with it rather than losing the typing.
    pub(crate) committed: Option<Focus>,
    /// The editor handed the keys on (Tab, Shift+Tab): the field the session moved to takes the key focus in the
    /// next pass, from its page part, as Tab onto the part would give it.
    pub(crate) refocus: bool,
    /// Fields modified by the user whose appearance stream has not been rasterized by the render worker yet.
    /// Tracked as (field_name, page_index) so committed edits stay immediately visible without flicker.
    pub unbaked: HashSet<(String, usize)>,
    /// A message for the app to show (e.g. "buttons run JavaScript").
    pub notice: Option<FormNotice>,
    /// A push button was clicked: (its field name, exact PDF identity, what it does).
    pub button: Option<(String, u64, pdfcraft_engine::form_scripts::ButtonAction)>,
    /// The field and widget that had focus when it was last looked at: when the focus moves, the
    /// one it left runs its XFA exit scripts and the one it reached its enter scripts
    /// (`PdfCraftApp::run_focus_scripts`).
    pub(crate) focused: Option<(String, Option<ObjRef>, usize)>,
    /// The list box row clicked last, without Shift: (its field name, the row). A Shift-click picks
    /// the rows from it to the one clicked; the field's value can't say which row came last when
    /// rows share an export value, and `/I` lists them in ascending order.
    pub(crate) list_anchor: Option<(String, usize)>,
    /// The field and widget under the pointer when it was last looked at: when it changes, the
    /// one left runs its Mouse Exit JavaScript and the one reached its Mouse Enter (#269).
    pub(crate) hovered: Option<WidgetRef>,
    /// Pointer triggers waiting to run their widget's JavaScript, in order (field name, widget,
    /// trigger): `PdfCraftApp::run_field_triggers` runs them after the frame's edit, so a check
    /// box's Mouse Up sees the value its click just stored (#247).
    pub(crate) triggers: Vec<(WidgetRef, pdfcraft_engine::FieldTrigger)>,
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

/// Where a field's text sits on screen. It is laid out upright in `upright`, the field's screen
/// rectangle with its sides swapped when the text runs up or down the screen, and turned onto
/// the field about its centre. The field's `/MK /R` turns its content counterclockwise on the
/// page (ISO 32000-2 Table 192), as its page appearance draws it; the page's `/Rotate` and the
/// view's rotation turn the page clockwise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TextFrame {
    pub upright: Rect,
    /// Clockwise on screen: 0, 90, 180 or 270 degrees.
    pub turn: i64,
}

impl TextFrame {
    pub(crate) fn new(field: Rect, mk_r: i64, page_rotate: u16, view_rotation: u16) -> Self {
        // Each term is reduced first, so the sum cannot overflow.
        let turn = (i64::from(page_rotate % 360) + i64::from(view_rotation % 360) - mk_r.rem_euclid(360)).rem_euclid(360) / 90 * 90;
        let upright = if turn % 180 == 90 { Rect::from_center_size(field.center(), vec2(field.height(), field.width())) } else { field };
        Self { upright, turn }
    }

    fn of(xf: &PageXform, info: &DocInfo, page: usize, w: &FormWidget, field: Rect) -> Self {
        Self::new(field, w.rotation, info.pages.get(page).map_or(0, |p| p.rotation), xf.rot)
    }

    fn angle(self) -> f32 {
        (self.turn as f32).to_radians()
    }

    /// A point of the upright box, on screen.
    pub(crate) fn on_screen(self, p: egui::Pos2) -> egui::Pos2 {
        let c = self.upright.center();
        c + egui::emath::Rot2::from_angle(self.angle()) * (p - c)
    }

    /// A screen point, in the upright box.
    pub(crate) fn in_box(self, p: egui::Pos2) -> egui::Pos2 {
        let c = self.upright.center();
        c + egui::emath::Rot2::from_angle(-self.angle()) * (p - c)
    }

    /// Paint `galley` with its top-left corner at `pos` in the upright box.
    fn paint_galley(self, painter: &egui::Painter, pos: egui::Pos2, galley: std::sync::Arc<egui::Galley>, color: Color32) {
        painter.add(egui::epaint::TextShape::new(self.on_screen(pos), galley, color).with_angle(self.angle()));
    }
}

fn fillable(f: &FormField) -> bool {
    !f.read_only() && !matches!(f.kind, FormFieldKind::PushButton | FormFieldKind::Signature)
}

/// The fillable widgets in the order Tab and Shift+Tab visit them.
pub fn tab_order(form: &[FormField], info: &DocInfo) -> Vec<(usize, usize)> {
    let mut order: Vec<(usize, usize, usize)> = form
        .iter()
        .enumerate()
        .filter(|(_, field)| fillable(field))
        .flat_map(|(fi, field)| {
            field
                .widgets
                .iter()
                .enumerate()
                .filter(|(_, widget)| {
                    let Some(page) = widget.page.and_then(|page| info.pages.get(page)) else { return false };
                    let [x0, y0, x1, y1] = widget.rect;
                    let [cx0, cy0, cx1, cy1] = page.crop.map(f64::from);
                    !widget.hidden
                        && !widget.read_only
                        && [x0, y0, x1, y1].into_iter().all(f64::is_finite)
                        && x0 < x1
                        && y0 < y1
                        && x1 > cx0
                        && x0 < cx1
                        && y1 > cy0
                        && y0 < cy1
                })
                .map(move |(wi, widget)| (widget.tab, fi, wi))
        })
        .collect();
    order.sort_by_key(|(tab, _, _)| *tab);
    order.into_iter().map(|(_, fi, wi)| (fi, wi)).collect()
}

/// Whether a combo box takes typed text as well as a choice from its list: the Edit flag
/// (ISO 32000-2 Table 233), which only means something with Combo set.
fn editable_combo(f: &FormField) -> bool {
    f.kind == FormFieldKind::Combo && f.has(field_flags::EDIT)
}

/// The text a field's editor starts with: its value, or for a combo box the shown text of the
/// option it picks (a free value typed into an editable one has no option, and shows as is).
pub(crate) fn draft_text(f: &FormField) -> String {
    let value = f.value.first().cloned().unwrap_or_default();
    if f.kind == FormFieldKind::Combo
        && let Some((_, shown)) = f.options.iter().find(|(export, _)| *export == value)
    {
        return shown.clone();
    }
    value
}

impl Focus {
    /// Whether this is the focus of widget `widget` of `f` (of that twin, for same-name fields).
    fn is_on(&self, f: &FormField, widget: usize) -> bool {
        self.name == f.name && self.widget == widget && self.obj.is_none_or(|o| o == f.obj)
    }
}

/// The field `focus` is on: the twin with its field object, or else the first field with its name.
pub(crate) fn focused_field<'a>(focus: &Focus, form: &'a [FormField]) -> Option<&'a FormField> {
    focus.obj.and_then(|o| form.iter().find(|f| f.obj == o && f.name == focus.name)).or_else(|| form.iter().find(|f| f.name == focus.name))
}

/// The page part of the field the focus is on, for the page parts' focus.
pub(crate) fn focus_owner(forms: &FormView, form: &[FormField]) -> Option<Owner> {
    let focus = forms.focus.as_ref()?;
    let field = form.iter().position(|f| f.name == focus.name && focus.obj.is_none_or(|o| o == f.obj))?;
    Some(Owner::Field { field, widget: focus.widget })
}

/// Focus arrived on a field's part (Tab, a focus request or a click): the field opens, unless it is open already,
/// with its whole value selected so typing replaces it, as Tab between fields opens it.
pub(crate) fn field_focused(session: &mut PageSession, form: &[FormField], owner: &Owner) {
    let Owner::Field { field, widget } = *owner else { return };
    let Some(f) = form.get(field) else { return };
    if session.forms.focus.as_ref().is_some_and(|x| x.is_on(f, widget)) {
        return;
    }
    commit(session, form);
    let mut opened = open_focus(f, widget);
    opened.selection = TextSelection::new(0, opened.text.chars().count());
    session.forms.focus = Some(opened);
}

/// The edit picking the options at `picked` (positions in `f`'s options) in the list box `f`.
/// The edit names the field, and positions are read in the first field of that name: a same-name
/// twin listing its options in another order gives its picks as positions there. One option
/// missing there goes by export value.
fn list_pick_edit(form: &[FormField], f: &FormField, picked: Vec<usize>) -> Edit {
    let first = form.iter().find(|x| x.name == f.name).filter(|x| x.obj != f.obj);
    let value = match first {
        None => FieldValue::ChoiceIndices(picked),
        Some(first) => match f.positions_in(first, &picked) {
            Some(there) => FieldValue::ChoiceIndices(there),
            None => FieldValue::Choice(picked.iter().filter_map(|&i| f.options.get(i)).map(|(e, _)| e.clone()).collect()),
        },
    };
    Edit::SetFieldValue { name: f.name.clone(), value }
}

/// Editor state for opening widget `widget` of `f`: it starts from the field's value with the caret at its start, a
/// drop-down with its list open and a date field with its calendar open.
pub(crate) fn open_focus(f: &FormField, widget: usize) -> Focus {
    Focus {
        name: f.name.clone(),
        obj: Some(f.obj),
        widget,
        text: draft_text(f),
        picked: f.value.clone(),
        selection: TextSelection::caret_at(0),
        list_open: f.kind == FormFieldKind::Combo,
        calendar_open: matches!(f.actions.format, pdfcraft_engine::form_scripts::Format::Date(_)),
    }
}

/// After a script changed the field being edited from `before` to `after`, each given as its
/// editor text ([`draft_text`]: a combo box's shown option) and its value: the editor shows
/// `after`, as [`open_focus`] would, unless the person has typed there already (the draft no
/// longer holds `before`), whose typing then stays theirs to commit.
pub(crate) fn refresh_draft(focus: &mut Focus, before: (&str, &[String]), after: (String, &[String])) {
    if focus.picked.as_slice() == before.1 && focus.text == before.0 {
        focus.text = after.0;
        focus.picked = after.1.to_vec();
    }
}

/// Select radio button `wi`, if it isn't selected already. The button itself, not its on state:
/// buttons sharing a state are exclusive unless the group has RadiosInUnison (ISO 32000-2
/// Table 229).
fn select_edit(f: &FormField, wi: usize) -> Option<Edit> {
    // A button read-only on its own (its widget's `/Ff`, Table 227) keeps its focus stop, but
    // the form would refuse turning it on.
    let selectable = f.widgets.get(wi).is_some_and(|w| w.on_state.is_some() && !w.read_only) && !f.widget_on(wi);
    selectable.then(|| Edit::SetFieldValue { name: f.name.clone(), value: FieldValue::Widget(wi) })
}

/// What clicking widget `wi` of a check box or radio button does.
fn toggle_edit(f: &FormField, wi: usize) -> Option<Edit> {
    let w = f.widgets.get(wi)?;
    let value = match f.kind {
        // A check box's kids can have different on states (ISO 32000-2 §12.7.5.2.3, Table 230):
        // clicking one that is off turns on its state (it, and any kid sharing that state, on;
        // the others off); clicking one that is on turns the field off.
        FormFieldKind::CheckBox => match &w.on_state {
            Some(s) if !f.widget_on(wi) => FieldValue::Text(s.clone()),
            Some(_) => FieldValue::Check(false),
            None => FieldValue::Check(f.value.is_empty()),
        },
        // Clicking the selected button again turns it off unless the group forbids it
        // (NoToggleToOff, Table 229).
        FormFieldKind::Radio if f.widget_on(wi) => {
            if f.has(field_flags::NO_TOGGLE_TO_OFF) {
                return None;
            }
            FieldValue::Radio(None)
        }
        FormFieldKind::Radio => return select_edit(f, wi),
        _ => return None,
    };
    Some(Edit::SetFieldValue { name: f.name.clone(), value })
}

/// Whether `w`, a widget of `f`, takes a press: fillable, a push button with an action (a click
/// or mouse-down one), or an
/// empty signature field (signed by clicking it, Use a certificate). A radio button of a group can
/// be read-only on its own.
fn usable(f: &FormField, w: &FormWidget, allowed: bool) -> bool {
    // A button acting only on mouse down (`/AA /D`) takes the press too, which runs it (#4751).
    let pressable = f.kind == FormFieldKind::PushButton && !f.read_only() && (f.button.is_some() || w.mouse_down);
    allowed && !w.read_only && (fillable(f) || pressable || f.kind == FormFieldKind::Signature)
}

/// A widget a pointer trigger is for: its field's name and object (so a field sharing another's
/// name runs its own script), and the widget's index in the field.
pub(crate) type WidgetRef = (String, u64, usize);

/// `f`'s widget `widget` as a trigger names it.
fn widget_ref(f: &FormField, widget: usize) -> WidgetRef {
    (f.name.clone(), pdfcraft_engine::js::field_object_id(f.obj), widget)
}

/// The field and widget under the pointer on a page, when a press there would take it (`hover`, the page parts'
/// hovered owner).
pub(crate) fn hovered(form: &[FormField], hover: Option<&Owner>) -> Option<WidgetRef> {
    match hover {
        Some(&Owner::Field { field, widget }) => form.get(field).map(|f| widget_ref(f, widget)),
        _ => None,
    }
}

/// Queue Mouse Exit for the widget the pointer left and Mouse Enter for the one it reached, when
/// they differ from the last frame's (`now`: the widget under the pointer on any page).
pub(crate) fn hover_moved(session: &mut PageSession, now: Option<WidgetRef>) {
    if session.forms.hovered == now {
        return;
    }
    let left = std::mem::replace(&mut session.forms.hovered, now.clone());
    if let Some(w) = left {
        session.forms.triggers.push((w, pdfcraft_engine::FieldTrigger::MouseExit));
    }
    if let Some(w) = now {
        session.forms.triggers.push((w, pdfcraft_engine::FieldTrigger::MouseEnter));
    }
}

/// The parts of the form widgets on `page`, one per shown widget (as the document's page actions and timers show
/// them, [`pdfcraft_engine::Document::shown_by_scripts`]), hit over its turned rectangle. Each has the cursor a press
/// would earn, its tooltip (`/TU`, the name the user interface identifies the field by, ISO 32000-2 Table 226, as
/// Acrobat shows it on hover, #3854) and, when it can be filled, a focus stop. The stops come first in the document's
/// tab order (each page's `/Tabs`), so the framework's Tab and Shift+Tab follow it. Over the page's text too: a check
/// box drawn with a glyph, or set on printed text, is a button, not text to select.
pub(crate) fn field_parts(
    session: &PageSession,
    info: &DocInfo,
    doc: &pdfcraft_engine::Document,
    page: usize,
    can_fill: bool,
    badge_tips: bool,
    geo: PageGeometry<'_>,
) -> Vec<PagePart> {
    let Some(xf) = session.page_xform(page, geo) else { return Vec::new() };
    let form = doc.form.as_slice();
    let badges = if badge_tips { crate::canvas::signature_badges(&doc.signatures, page) } else { Vec::new() };
    let tabbed = tab_order(form, info);
    let in_order: HashSet<(usize, usize)> = tabbed.iter().copied().collect();
    let part = |fi: usize, wi: usize| -> Option<PagePart> {
        let f = form.get(fi)?;
        let w = f.widgets.get(wi)?;
        if w.page != Some(page) || !doc.shown_by_scripts(&f.name).unwrap_or(!w.hidden) {
            return None;
        }
        let r = widget_rect(&xf, info, page, w.rect);
        let corners = vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
        let icon = match (usable(f, w, can_fill), f.kind) {
            (false, _) => CursorIcon::NotAllowed,
            (true, FormFieldKind::Text) => CursorIcon::Text,
            _ => CursorIcon::PointingHand,
        };
        // Where widgets overlap, the field earlier in the form takes the press, whatever the tab order.
        let stacking = u32::try_from(form.len().saturating_sub(fi)).unwrap_or(u32::MAX);
        let mut part =
            PagePart::new(Owner::Field { field: fi, widget: wi }, page, HitShape::Polygon(corners), PageLayer::Field).cursor(icon).stacked(stacking);
        let badge_tip = (f.kind == FormFieldKind::Signature)
            .then(|| {
                badges.iter().find(|b| b.rect.iter().zip(w.rect).all(|(a, b)| (a - b).abs() < 1.0)).map(|b| crate::i18n::t(b.summary).to_string())
            })
            .flatten();
        if let Some(tip) = badge_tip {
            part = part.tip_now(tip);
        } else if let Some(tip) = f.tooltip.as_deref().filter(|t| !t.trim().is_empty()) {
            part = part.tip(tip.to_owned());
        }
        let is_focused = session.forms.focus.as_ref().is_some_and(|x| x.is_on(f, wi));
        if in_order.contains(&(fi, wi)) || is_focused {
            part = part.focusable();
        }
        Some(part)
    };
    let mut parts: Vec<PagePart> = tabbed.iter().filter_map(|&(fi, wi)| part(fi, wi)).collect();
    for (fi, f) in form.iter().enumerate() {
        for wi in 0..f.widgets.len() {
            if !in_order.contains(&(fi, wi)) {
                parts.extend(part(fi, wi));
            }
        }
    }
    parts
}

/// A press on a form widget part. `allowed`: whether the document lets its fields be filled. A text field opens on
/// the press, its caret where it landed, and a drag moves the caret, so press-and-drag selects text in a field that
/// was not focused (#2213). The rest act on the click, which counts when the button comes up over the widget its press
/// began on, as a button's does. The caret's offset under a point is the editor's one mapping ([`editor::offset_at`]).
pub(crate) fn field_press(
    ctx: &egui::Context,
    session: &mut PageSession,
    doc: &pdfcraft_engine::Document,
    info: &DocInfo,
    allowed: bool,
    press: &PagePress,
    geo: PageGeometry<'_>,
) {
    let Owner::Field { field, widget: wi } = press.owner else { return };
    let form = doc.form.as_slice();
    let Some((f, w)) = form.get(field).and_then(|f| f.widgets.get(wi).map(|w| (f, w))) else { return };
    let usable = usable(f, w, allowed);
    match &press.phase {
        PressPhase::Down { start, .. } => {
            // The press runs the widget's Mouse Down JavaScript, as in Acrobat (#269).
            if usable {
                session.forms.triggers.push((widget_ref(f, wi), pdfcraft_engine::FieldTrigger::MouseDown));
            }
            if f.kind == FormFieldKind::Text && usable {
                if session.forms.focus.as_ref().is_none_or(|x| !x.is_on(f, wi)) {
                    commit(session, form);
                    session.forms.focus = Some(open_focus(f, wi));
                }
                let at = editor::offset_at(ctx, session, form, info, field, wi, start.origin, geo);
                if let Some(x) = session.forms.focus.as_mut() {
                    x.selection = TextSelection::new(at, at);
                }
            }
        }
        PressPhase::DragStart { pos, .. } | PressPhase::Drag { pos, .. } => {
            if f.kind == FormFieldKind::Text && session.forms.focus.as_ref().is_some_and(|x| x.is_on(f, wi)) {
                let at = editor::offset_at(ctx, session, form, info, field, wi, *pos, geo);
                if let Some(x) = session.forms.focus.as_mut() {
                    x.selection = TextSelection::new(x.selection.anchor, at);
                }
            }
        }
        PressPhase::Release { pos, click: true, .. } => clicked(session, doc, info, allowed, (f, w, wi), (press.page, *pos, press.mods), geo),
        _ => {}
    }
}

/// What a click at `at` on widget `wi` of `f` (field index `field`, widget `w`) does, with `mods` held.
fn clicked(
    session: &mut PageSession,
    doc: &pdfcraft_engine::Document,
    info: &DocInfo,
    allowed: bool,
    (f, w, wi): (&FormField, &FormWidget, usize),
    (page, at, mods): (usize, Pos2, Mods),
    geo: PageGeometry<'_>,
) {
    let form = doc.form.as_slice();
    if !allowed {
        session.forms.notice = Some(FormNotice::Security);
        return;
    }
    match f.kind {
        _ if f.read_only() || w.read_only => session.forms.notice = Some(FormNotice::ReadOnly(f.name.clone())),
        FormFieldKind::PushButton => match &f.button {
            Some(a) => session.forms.button = Some((f.name.clone(), pdfcraft_engine::js::field_object_id(f.obj), a.clone())),
            // Its press already ran its mouse-down action.
            None if w.mouse_down => {}
            None => session.forms.notice = Some(FormNotice::NoAction(f.name.clone())),
        },
        FormFieldKind::Text if let Some(script) = &w.mouse_up_script => {
            // Close the editor so the script can replace its value without a stale draft.
            commit(session, form);
            session.forms.button = Some((
                f.name.clone(),
                pdfcraft_engine::js::field_object_id(f.obj),
                pdfcraft_engine::form_scripts::ButtonAction::Script(script.clone()),
            ));
        }
        FormFieldKind::Signature => session.sign.field = Some(f.name.clone()),
        FormFieldKind::CheckBox | FormFieldKind::Radio => {
            // The box takes the focus, as in Acrobat, so Tab goes on from it (#2223).
            if session.forms.focus.as_ref().is_none_or(|x| !x.is_on(f, wi)) {
                commit(session, form);
                session.forms.focus = Some(open_focus(f, wi));
            }
            session.forms.refocus = true;
            // The toggle goes after the edit for any text the click committed, in one batch: if that text is
            // refused, its field reopens and the toggle is dropped with it.
            let toggle = toggle_edit(f, wi);
            session.pending_edit = match (session.pending_edit.take(), toggle) {
                (Some(first), Some(toggle)) => Some(Edit::Batch { label: "Edit field".into(), edits: vec![first, toggle] }),
                (first, toggle) => first.or(toggle),
            };
            // The click's Mouse Up JavaScript runs once the toggle is stored, so it reads the new value (#247).
            session.forms.triggers.push((widget_ref(f, wi), pdfcraft_engine::FieldTrigger::MouseUp));
        }
        // The press already committed the previous field and opened this one.
        FormFieldKind::Text => {}
        FormFieldKind::Combo => {
            if session.forms.focus.as_ref().is_none_or(|x| !x.is_on(f, wi)) {
                commit(session, form);
                session.forms.focus = Some(open_focus(f, wi));
                session.forms.refocus = true;
            }
            // A combo box's click runs its Mouse Up JavaScript (#269).
            session.forms.triggers.push((widget_ref(f, wi), pdfcraft_engine::FieldTrigger::MouseUp));
        }
        FormFieldKind::List => {
            let Some(xf) = session.page_xform(page, geo) else { return };
            if !xf.rect.contains(at) {
                return;
            }
            commit(session, form);
            session.forms.focus = Some(open_focus(f, wi));
            session.forms.refocus = true;
            let r = widget_rect(&xf, info, page, w.rect);
            let height_pt = if matches!(w.rotation, 90 | 270) { (w.rect[2] - w.rect[0]).max(1.0) } else { (w.rect[3] - w.rect[1]).max(1.0) };
            let zoom = (r.height() as f64) / height_pt;
            let y_pt = (at.y - r.min.y) as f64 / zoom;
            let rows = w.list.unwrap_or(form_appearance::ListRows { top: 0, inset: 2.0, row: 12.0 * 1.15, size: 12.0 });
            let top = form_appearance::ListRows::top_showing(rows.top, &f.selected, rows.visible(height_pt), f.options.len());
            if let Some(opt_idx) = rows.option_at(top, y_pt).filter(|&i| i < f.options.len()) {
                // `cmd` is Control off macOS, where `ctrl` is always false.
                let ctrl = mods.cmd || mods.ctrl;
                let shift = mods.shift;
                let multi = f.has(field_flags::MULTI_SELECT);
                // Options by position: two options can share an export value (ISO 32000-2
                // Table 234, `/I`), and the click picks the row, not the first option with its value.
                let in_order = f.picked_in_order();
                let picked: Vec<usize> = if multi && ctrl {
                    let mut picked = in_order.clone();
                    if let Some(pos) = picked.iter().position(|&x| x == opt_idx) {
                        picked.remove(pos);
                    } else {
                        picked.push(opt_idx);
                    }
                    picked
                } else if multi
                    && shift
                    && let Some(anchor) = session
                        .forms
                        .list_anchor
                        .as_ref()
                        .filter(|(name, _)| *name == f.name)
                        .map(|(_, row)| *row)
                        .or_else(|| in_order.last().copied())
                {
                    let (min, max) = (anchor.min(opt_idx), anchor.max(opt_idx));
                    (min..=max).collect()
                } else {
                    vec![opt_idx]
                };
                if !(multi && shift) {
                    session.forms.list_anchor = Some((f.name.clone(), opt_idx));
                }
                // A single-select list holding an array is written as one value even when its shown
                // row is clicked again.
                if picked != in_order || f.single_value_is_array {
                    session.pending_edit = Some(list_pick_edit(form, f, picked));
                }
            }
            // A list box's click runs its Mouse Up JavaScript after the pick is stored (#269).
            session.forms.triggers.push((widget_ref(f, wi), pdfcraft_engine::FieldTrigger::MouseUp));
        }
    }
}

/// Tab or Shift+Tab on a focused non-editor field (check box, radio button, list box, non-editable combo):
/// commits and moves to the next or previous fillable widget in the document's tab order, across pages too.
pub(crate) fn field_tab(session: &mut PageSession, info: &DocInfo, form: &[FormField], forward: bool, geo: PageGeometry<'_>) -> Option<Edit> {
    let focus = session.forms.focus.clone()?;
    let f = focused_field(&focus, form)?;
    let edit = tab_to_next_field(session, info, form, f, focus.widget, forward, geo);
    session.forms.refocus = true;
    edit
}

/// What a key the focused check box, radio button or list box got does: the edit to apply, if any. Space (or Enter)
/// toggles a check box and selects a radio button, as in browsers: it never turns the selected button off. The arrows
/// step through a radio group (selecting as they go) and move the pick in a list box, Shift extending it in a
/// multi-select one. Tab goes on to the next field in the tab order. Escape ends the session. A page key drops the
/// focus and is the page's.
pub(crate) fn field_key(session: &mut PageSession, info: &DocInfo, form: &[FormField], intent: &Intent, geo: PageGeometry<'_>) -> Option<Edit> {
    let focus = session.forms.focus.clone()?;
    let f = focused_field(&focus, form)?;
    let widget = focus.widget;
    let read_only = f.read_only() || f.widgets.get(widget).is_some_and(|w| w.read_only);
    match intent {
        Intent::Cancel => {
            session.forms.focus = None;
            None
        }
        Intent::Tab => tab_to_next_field(session, info, form, f, widget, true, geo),
        Intent::Activate | Intent::Space if !read_only => match f.kind {
            FormFieldKind::Radio => select_edit(f, widget),
            FormFieldKind::CheckBox => toggle_edit(f, widget),
            _ => None,
        },
        Intent::Move(NavMove::PageUp | NavMove::PageDown | NavMove::Start | NavMove::End, _) => {
            session.forms.focus = None;
            None
        }
        Intent::Move(step @ (NavMove::Up | NavMove::Down | NavMove::Back | NavMove::Forward), mods) if !read_only => {
            let forward = matches!(step, NavMove::Down | NavMove::Forward);
            match f.kind {
                FormFieldKind::Radio => step_radio(session, f, widget, forward),
                FormFieldKind::List if matches!(step, NavMove::Up | NavMove::Down) && !f.options.is_empty() => {
                    let current = f.picked_in_order().last().copied().unwrap_or(0);
                    let next = if forward { current.saturating_add(1).min(f.options.len() - 1) } else { current.saturating_sub(1) };
                    if next == current {
                        return None;
                    }
                    // By position, so the arrows step through options that share an export value, in the
                    // focused twin's own order.
                    let picked = if mods.extend && f.has(field_flags::MULTI_SELECT) {
                        let anchor = session.forms.list_anchor.as_ref().filter(|(name, _)| *name == f.name).map_or(current, |(_, row)| *row);
                        (anchor.min(next)..=anchor.max(next)).collect()
                    } else {
                        session.forms.list_anchor = Some((f.name.clone(), next));
                        vec![next]
                    };
                    Some(list_pick_edit(form, f, picked))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The edit the focused field's draft makes, if it differs from the field's value.
pub(crate) fn draft_edit(focus: &Focus, form: &[FormField]) -> Option<Edit> {
    let f = focused_field(focus, form)?;
    if f.read_only() || f.widgets.get(focus.widget).is_some_and(|widget| widget.read_only) {
        return None;
    }
    let value = match f.kind {
        FormFieldKind::Text if f.value.first().map(String::as_str).unwrap_or("") != focus.text => FieldValue::Text(focus.text.clone()),
        // Typed into an editable combo box: a shown text picks its option, anything else is the
        // value itself (the engine maps it, and refuses free text when Edit is clear).
        FormFieldKind::Combo if editable_combo(f) && focus.text != draft_text(f) && f.value.first() != Some(&focus.text) => {
            FieldValue::Choice(if focus.text.is_empty() { Vec::new() } else { vec![focus.text.clone()] })
        }
        _ => return None,
    };
    Some(Edit::SetFieldValue { name: f.name.clone(), value })
}

/// Close the focused field's editor, queueing the edit for its draft if it differs from the
/// field's value.
pub(crate) fn commit(session: &mut PageSession, form: &[FormField]) {
    let Some(focus) = session.forms.focus.take() else { return };
    if let Some(edit) = draft_edit(&focus, form) {
        session.pending_edit = Some(edit);
        session.forms.committed = Some(focus);
    }
}

/// Paint the hover frame round the widget a press under the pointer would take (`hover`, the page parts' hovered
/// owner), the unbaked widgets and the drop-down arrows. Focus is the framework's ring on the part. `painter` lays
/// out and paints the field's own document text, whose auto size `Draw` cannot measure.
pub(crate) fn paint_page(
    draw: &Draw<'_>,
    painter: &egui::Painter,
    (xf, page): (&PageXform, usize),
    info: &DocInfo,
    form: &[FormField],
    session: &PageSession,
    hover: Option<&Owner>,
) {
    let hovered = match hover {
        Some(&Owner::Field { field, widget }) => Some((field, widget)),
        _ => None,
    };
    for (fi, f) in form.iter().enumerate() {
        for (wi, w) in f.widgets.iter().enumerate().filter(|(_, w)| w.page == Some(page) && !w.hidden) {
            let r = widget_rect(xf, info, page, w.rect);
            if session.forms.unbaked.contains(&(f.name.clone(), page)) {
                paint_unbaked_widget(draw, painter, xf, TextFrame::of(xf, info, page, w, r), (f, wi, w), r);
            } else if fillable(f) && hovered == Some((fi, wi)) {
                draw.surface(r.expand(1.0), SurfaceKind::Frame, Corners::All);
            }
            if f.kind == FormFieldKind::Combo && fillable(f) {
                paint_drop_down_arrow(draw, r);
            }
        }
    }
}

/// A drop-down's arrow at the right of its box, which says the field opens a list (its page
/// appearance shows only the value). Drawn on screen only, not into the file.
fn paint_drop_down_arrow(draw: &Draw<'_>, r: Rect) {
    let width = (r.height() * 0.45).clamp(5.0, 12.0);
    let inset = (r.height() * 0.25).clamp(2.0, 6.0);
    // Too narrow a box keeps its value visible instead.
    if r.width() < width * 3.0 {
        return;
    }
    let (x, y, half) = (r.max.x - inset - width / 2.0, r.center().y, width / 2.0);
    let points = vec![egui::pos2(x - half, y - half / 2.0), egui::pos2(x + half, y - half / 2.0), egui::pos2(x, y + half / 2.0)];
    draw.data_fill(&PaintShape::Polygon(points), DataColor::new(Color32::BLACK));
}

/// The page's paper under an unbaked widget, with the frame round it.
fn paint_paper(draw: &Draw<'_>, r: Rect) {
    draw.data_fill(&PaintShape::Rect(r, CornerRadius::ZERO), DataColor::new(PAPER));
    draw.surface(r, SurfaceKind::Frame, Corners::All);
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
        let (across, up) = ((w.rect[2] - w.rect[0]).abs(), (w.rect[3] - w.rect[1]).abs());
        // A field turned a quarter lays its text out along its height, as its appearance does.
        let (across, up) = if matches!(w.rotation, 90 | 270) { (up, across) } else { (across, up) };
        let (width, height) = (across - 2.0 * pad, up - 2.0 * pad);
        // Comb cells hold one character each, so only the height limits them, as on the page.
        if f.is_comb() {
            return pdfcraft_engine::field_comb_auto_text_size(height);
        }
        let font = |size: f64| egui::FontId::proportional(size as f32);
        let line_width = |size: f64| {
            bounded_line_width(size, |at| painter.layout_no_wrap(text.to_string(), egui::FontId::proportional(at), Color32::BLACK).size().x)
        };
        // Wrapped as the editor and the page preview wrap it.
        let wrap = width.max(1.0) as f32;
        let wrapped_height = |size: f64| f64::from(painter.layout(text.to_string(), font(size), Color32::BLACK, wrap).size().y);
        pdfcraft_engine::field_auto_text_size_by(width, height, f.has(field_flags::MULTILINE), line_width, wrapped_height)
    });
    size as f32
}

/// The width of a line at `size` points from `measure`, the width at a given size, called at no
/// more than [`MAX_SCREEN_FONT`] and scaled: egui rasterizes every glyph it lays out, and a tall
/// field's automatic size can be thousands of points.
fn bounded_line_width(size: f64, measure: impl Fn(f32) -> f32) -> f64 {
    let at = size.min(f64::from(MAX_SCREEN_FONT));
    if at > 0.0 { f64::from(measure(at as f32)) * size / at } else { f64::from(measure(at as f32)) }
}

/// The largest font, in screen points, the on-page field editor draws with: a bound that keeps
/// one rasterized glyph (font size times pixels per point) well inside egui's font atlas.
pub const MAX_SCREEN_FONT: f32 = 256.0;

/// The font, in screen points, a field's editor shows `text` at, `zoom` screen points per page
/// point: [`field_text_size`] scaled, within 3 and [`MAX_SCREEN_FONT`].
pub fn field_screen_font(painter: &egui::Painter, f: &FormField, w: &FormWidget, text: &str, zoom: f32) -> f32 {
    let font = field_text_size(painter, f, w, text) * zoom;
    if font.is_nan() { 3.0 } else { font.clamp(3.0, MAX_SCREEN_FONT) }
}

fn paint_unbaked_widget(
    draw: &Draw<'_>,
    painter: &egui::Painter,
    xf: &PageXform,
    frame: TextFrame,
    (f, wi, w): (&FormField, usize, &FormWidget),
    r: Rect,
) {
    let selected_row = draw.role(|c| c.state.selected).egui();
    match f.kind {
        FormFieldKind::Text | FormFieldKind::Combo => {
            let text = f.value.first().map(String::as_str).unwrap_or("");
            paint_paper(draw, r);
            if !text.is_empty() {
                let unrotated_w = if xf.rot % 180 == 90 { xf.ph } else { xf.pw };
                let zoom = xf.rect.width() / unrotated_w.max(1.0);
                let font_size = field_screen_font(painter, f, w, text, zoom);
                let display_text = if f.has(field_flags::PASSWORD) { "•".repeat(text.chars().count()) } else { text.to_string() };
                let p = painter.with_clip_rect(r);
                // Laid out upright in the frame's box, then turned onto the field (#4296).
                let b = frame.upright;
                let (align, pos) = match f.quadding {
                    1 => (egui::Align2::CENTER_TOP, egui::pos2(b.center().x, b.min.y + 1.0)),
                    2 => (egui::Align2::RIGHT_TOP, egui::pos2(b.max.x - 3.0, b.min.y + 1.0)),
                    _ => (egui::Align2::LEFT_TOP, egui::pos2(b.min.x + 3.0, b.min.y + 1.0)),
                };
                // Multiline fields wrap at the box, as their page appearance does.
                // At the width the size was fitted to: the box less 3 pt each side, scaled.
                let wrap = if f.has(field_flags::MULTILINE) { (b.width() - 6.0 * zoom).max(1.0) } else { f32::INFINITY };
                let galley = p.layout(display_text, egui::FontId::proportional(font_size), Color32::BLACK, wrap);
                frame.paint_galley(&p, align.anchor_size(pos, galley.size()).min, galley, Color32::BLACK);
            }
        }
        FormFieldKind::List => {
            paint_paper(draw, r);
            let unrotated_w = if xf.rot % 180 == 90 { xf.ph } else { xf.pw };
            let zoom = (xf.rect.width() / unrotated_w.max(1.0)) as f64;
            let rows = w.list.unwrap_or(form_appearance::ListRows { top: 0, inset: 2.0, row: 12.0 * 1.15, size: 12.0 });
            let height_pt = if matches!(w.rotation, 90 | 270) { (w.rect[2] - w.rect[0]).max(1.0) } else { (w.rect[3] - w.rect[1]).max(1.0) };
            let top = form_appearance::ListRows::top_showing(rows.top, &f.selected, rows.visible(height_pt), f.options.len());
            let font_size = (rows.size * zoom).clamp(6.0, 64.0) as f32;
            let row_h = (rows.row * zoom) as f32;
            let inset = (rows.inset * zoom) as f32;
            let p = painter.with_clip_rect(r);
            let mut y = r.min.y + inset;
            for (i, (_, display)) in f.options.iter().enumerate().skip(top) {
                if y + row_h > r.max.y {
                    break;
                }
                let row_rect = Rect::from_min_size(egui::pos2(r.min.x + 1.0, y), egui::vec2(r.width() - 2.0, row_h));
                // By position: options can share an export value.
                if f.selected.contains(&i) {
                    p.rect_filled(row_rect, CornerRadius::ZERO, selected_row);
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
            paint_paper(draw, r);
            let checked = if w.on_state.is_some() { f.widget_on(wi) } else { f.value.first().is_some_and(|v| v != "Off" && !v.is_empty()) };
            if checked {
                let sz = (r.height() * 0.75).clamp(8.0, 32.0);
                let c = r.center();
                let tick =
                    vec![egui::pos2(c.x - sz * 0.3, c.y), egui::pos2(c.x - sz * 0.1, c.y + sz * 0.25), egui::pos2(c.x + sz * 0.3, c.y - sz * 0.25)];
                draw.stroke(&PaintShape::Path(tick), StrokeWidth::Medium, DataColor::new(Color32::BLACK));
            }
        }
        FormFieldKind::Radio => {
            paint_paper(draw, r);
            let selected = f.widget_on(wi);
            if selected {
                let radius = (r.height().min(r.width()) * 0.25).clamp(2.0, 16.0);
                draw.data_fill(&PaintShape::Circle(r.center(), radius), DataColor::new(Color32::BLACK));
            }
        }
        _ => {}
    }
}

/// Tab / Shift+Tab: commits the field and moves to the next fillable widget in the document's tab
/// order, opening it with its whole value selected so typing replaces it. A widget not wholly in view is scrolled
/// into it after the pass. Returns the commit's edit to apply.
fn tab_to_next_field(
    session: &mut PageSession,
    info: &DocInfo,
    form: &[FormField],
    f: &FormField,
    widget: usize,
    forward: bool,
    geo: PageGeometry<'_>,
) -> Option<Edit> {
    commit(session, form);
    let pending = session.pending_edit.take();
    // The next field that can be filled in, check boxes and radio buttons included (#2223), in
    // the document's tab order (each page's /Tabs: rows, columns or structure), wrapping.
    let order = tab_order(form, info);
    let exact = order.iter().position(|(fi, wi)| form.get(*fi).is_some_and(|x| x.obj == f.obj && *wi == widget));
    let same_field = order.iter().position(|(fi, _)| form.get(*fi).is_some_and(|x| x.name == f.name));
    let next_after_removed = if exact.is_none() && same_field.is_none() {
        form.iter().position(|field| field.name == f.name).and_then(|fi| {
            let tab = form.get(fi)?.widgets.get(widget)?.tab;
            let key = (tab, fi, widget);
            let tab_key = |(fi, wi): &(usize, usize)| form.get(*fi).and_then(|field| field.widgets.get(*wi)).map(|other| (other.tab, *fi, *wi));
            if forward {
                order.iter().position(|entry| tab_key(entry).is_some_and(|other| other > key)).or_else(|| (!order.is_empty()).then_some(0))
            } else {
                order.iter().rposition(|entry| tab_key(entry).is_some_and(|other| other < key)).or_else(|| order.len().checked_sub(1))
            }
        })
    } else {
        None
    };
    if let Some(pos) = exact.or(same_field).or(next_after_removed) {
        // A focused widget that left the order is replaced by the field's first remaining stop.
        let k = if next_after_removed.is_some() || (forward && exact.is_none()) {
            pos
        } else if forward {
            (pos + 1) % order.len()
        } else {
            (pos + order.len() - 1) % order.len()
        };
        let Some((fi, wi)) = order.get(k).copied() else { return pending };
        let Some(target) = form.get(fi) else { return pending };
        let mut opened = open_focus(target, wi);
        opened.selection = TextSelection::new(0, opened.text.chars().count());
        session.forms.focus = Some(opened);
        if let Some(widget) = target.widgets.get(wi)
            && let Some(page) = widget.page
            && !session
                .page_xform(page, geo)
                .map(|xf| widget_rect(&xf, info, page, widget.rect))
                .is_some_and(|rect| geo.viewport.contains(rect.min) && geo.viewport.contains(rect.max))
        {
            session.pending_page = Some((page, Some(widget.rect.map(|coord| coord as f32))));
        }
    }
    pending
}

/// The arrows move to the group's next or previous button and select it, wrapping.
fn step_radio(session: &mut PageSession, f: &FormField, widget: usize, forward: bool) -> Option<Edit> {
    let mut edit = None;
    let stops: Vec<usize> = f.widgets.iter().enumerate().filter(|(_, w)| w.page.is_some() && !w.hidden).map(|(i, _)| i).collect();
    if let Some(pos) = stops.iter().position(|&i| i == widget) {
        let k = if forward { (pos + 1) % stops.len() } else { (pos + stops.len() - 1) % stops.len() };
        if let Some((wi, target)) = stops.get(k).and_then(|&wi| Some((wi, f.widgets.get(wi)?))) {
            session.forms.focus = Some(open_focus(f, wi));
            edit = select_edit(f, wi);
            if let Some(p) = target.page
                && !session.placed.iter().any(|placed| placed.page == p)
            {
                session.pending_page = Some((p, None));
            }
        }
    }
    edit
}

/// Where a field's widget is on screen (tests and automation).
pub fn field_screen_rect(view: &DocView, info: &DocInfo, f: &FormField, widget: usize) -> Option<Rect> {
    let w = f.widgets.get(widget)?;
    let page = w.page?;
    let xf = view.page_xform(page)?;
    Some(widget_rect(&xf, info, page, w.rect))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;
    use std::cell::Cell;

    /// A draft as the editor opens it on a field holding `value`.
    fn draft(value: &[&str]) -> Focus {
        Focus {
            name: "hint".into(),
            obj: None,
            widget: 0,
            text: value.first().map(|s| s.to_string()).unwrap_or_default(),
            picked: value.iter().map(|s| s.to_string()).collect(),
            selection: TextSelection::caret_at(0),
            list_open: false,
            calendar_open: false,
        }
    }

    /// An untouched draft takes the value a script gave the field; typing is kept.
    #[test]
    fn a_draft_follows_a_scripts_change_until_something_is_typed() {
        let before = vec!["Type here".to_string()];
        let mut untouched = draft(&["Type here"]);
        refresh_draft(&mut untouched, ("Type here", &before), (String::new(), &[]));
        assert_eq!((untouched.text.as_str(), untouched.picked.len()), ("", 0));
        let mut typed = draft(&["Type here"]);
        typed.text = "Ann".into();
        refresh_draft(&mut typed, ("Type here", &before), (String::new(), &[]));
        assert_eq!(typed.text, "Ann", "the person's typing stays");
        // A list's choices count as typing too.
        let mut picked = draft(&["a"]);
        picked.picked.push("b".into());
        refresh_draft(&mut picked, ("a", &["a".to_string()]), ("c".into(), &["c".to_string()]));
        assert_eq!(picked.picked, ["a", "b"]);
        // A combo box's editor shows the option's shown text, not its export value: that is
        // what an untouched draft holds, and what it takes.
        let mut combo = draft(&["a"]);
        combo.text = "Apple".into();
        refresh_draft(&mut combo, ("Apple", &["a".to_string()]), ("Banana".into(), &["b".to_string()]));
        assert_eq!((combo.text.as_str(), combo.picked.as_slice()), ("Banana", ["b".to_string()].as_slice()));
    }

    #[test]
    fn a_line_is_never_laid_out_above_the_screen_font_bound() {
        let largest = Cell::new(0.0f32);
        let measure = |at: f32| {
            largest.set(largest.get().max(at));
            at * 3.0
        };
        assert_eq!(bounded_line_width(12_522.0, measure), 12_522.0 * 3.0, "the width scales");
        assert_eq!(largest.get(), MAX_SCREEN_FONT, "laid out at the bound");
        assert_eq!(bounded_line_width(10.0, measure), 30.0, "below it, at the size itself");
        assert_eq!(largest.get(), MAX_SCREEN_FONT);
    }

    fn near(a: egui::Pos2, b: egui::Pos2) -> bool {
        (a - b).length() < 1e-3
    }

    /// #4296: a tall field turned 90 degrees counterclockwise lays its text out in a box as wide
    /// as the field is tall, and that box's top-left corner lands on the field's bottom-left, so
    /// the text runs up the screen.
    #[test]
    fn a_quarter_turned_field_runs_its_text_up_the_screen() {
        let field = Rect::from_min_max(pos2(100.0, 100.0), pos2(120.0, 260.0));
        let frame = TextFrame::new(field, 90, 0, 0);
        assert_eq!(frame.turn, 270);
        assert_eq!(frame.upright.size(), egui::vec2(160.0, 20.0));
        assert!(near(frame.on_screen(frame.upright.left_top()), field.left_bottom()), "{:?}", frame.on_screen(frame.upright.left_top()));
        assert!(near(frame.on_screen(frame.upright.right_top()), field.left_top()));
        for p in [pos2(105.0, 110.0), pos2(119.0, 250.0)] {
            assert!(near(frame.on_screen(frame.in_box(p)), p));
        }
        // 270 runs down the screen, from the top-right corner.
        let down = TextFrame::new(field, 270, 0, 0);
        assert_eq!(down.turn, 90);
        assert!(near(down.on_screen(down.upright.left_top()), field.right_top()));
    }

    /// The page's /Rotate and the view's rotation turn the page clockwise, against /MK /R: a field
    /// turned 90 on a page shown a quarter clockwise reads upright, in its own rectangle.
    #[test]
    fn page_and_view_rotation_turn_against_the_field() {
        let field = Rect::from_min_max(pos2(0.0, 0.0), pos2(80.0, 20.0));
        assert_eq!(TextFrame::new(field, 90, 90, 0), TextFrame { upright: field, turn: 0 });
        assert_eq!(TextFrame::new(field, 90, 0, 90), TextFrame { upright: field, turn: 0 });
        assert_eq!(TextFrame::new(field, 0, 0, 0), TextFrame { upright: field, turn: 0 });
        assert_eq!(TextFrame::new(field, 180, 270, 270).turn, 0);
        assert_eq!(TextFrame::new(field, 0, 180, 0).turn, 180);
        // Out-of-range input stays a quarter turn.
        assert_eq!(TextFrame::new(field, i64::MIN, u16::MAX, u16::MAX).turn % 90, 0);
    }
}
