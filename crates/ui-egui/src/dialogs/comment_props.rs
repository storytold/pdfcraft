//! Comment Properties (Acrobat: right-click a comment ▸ Properties…; audit "Comment
//! properties"): Appearance (colour, fill, opacity, line thickness, note icon), General (author,
//! subject, modified) and Review History (status changes), with Acrobat's Locked box. The dialog
//! is a tabbed parameter dialog the app's dialog host shows; [`edited`] maps its values back to
//! the comment's properties.

use craft::host::{PickRequest, Picked};
use craft::theme::Srgba;
use craft::ui::contract::Bind;
use craft::ui::dialog::DialogId;
use craft::ui::dialog::DialogResult;
use craft::ui::patterns::form_dialogs::{
    DialogFields, FieldValue, HostedParamDialog, Param, ParamDialog, ParamKind, PropertiesDialog, PropertiesTab, PropertyRow, SharedRows,
};
use craft::ui::runtime::env::UnitKind;
use craft::ui::value_drag::SliderScale;
use pdfcraft_engine::{BorderShape, CommentProps, Edit, LineEnding, NoteIcon};

use crate::PdfCraftApp;
use crate::colour_picker::PickerHost;
use crate::comments::{RecentColours, border_choice, fill_choice, palette};

/// A comment colour as the dialog's field value.
fn field_colour(c: pdfcraft_engine::Rgb) -> FieldValue {
    FieldValue::Color(c.map(|v| v as f32))
}

/// A dialog field's colour as the comment's. Each channel is the decimal the `f32` shows (its shortest form), not
/// its binary widening: a colour the dialog opened with or was given (0.18) comes back as itself, not 0.18000000715.
fn rgb(c: [f32; 3]) -> pdfcraft_engine::Rgb {
    c.map(|v| v.to_string().parse().unwrap_or(f64::from(v)))
}

/// Where the Comment Properties dialog stands: the id the dialog host returned for it, and what it
/// edits. The dialog itself lives in the host, which shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct CommentPropsDialog {
    /// The host's id of the open dialog.
    pub id: DialogId,
    /// The comment the dialog edits and the others selected with it.
    pub draft: PropsDraft,
}

/// The comment a Comment Properties dialog edits, as it was when the dialog opened.
#[derive(Clone, Debug, PartialEq)]
pub struct PropsDraft {
    pub page: usize,
    pub index: usize,
    pub original: CommentProps,
    /// The other selected comments, each with its own properties: OK makes the same changes to
    /// them where they apply (a colour to every one, a font size to text boxes, …).
    pub others: Vec<(usize, usize, CommentProps)>,
}

const KEY: &str = "comment.properties";
const COLOUR: &str = "comment.props.colour";
const BORDER: &str = "comment.props.border";
const FILL: &str = "comment.props.fill";
const MAKE_DEFAULT: &str = "make_default";

/// The name shown in the line-ending combo. [`LineEnding::name`] stays the PDF `/LE` token.
fn ending_label(ending: LineEnding) -> &'static str {
    match ending {
        LineEnding::None => tl!("None"),
        LineEnding::Square => tl!("Square"),
        LineEnding::Circle => tl!("Circle"),
        LineEnding::Diamond => tl!("Diamond"),
        LineEnding::OpenArrow => tl!("Open arrow"),
        LineEnding::ClosedArrow => tl!("Closed arrow"),
        LineEnding::Butt => tl!("Butt"),
        LineEnding::ROpenArrow => tl!("Reverse open arrow"),
        LineEnding::RClosedArrow => tl!("Reverse closed arrow"),
        LineEnding::Slash => tl!("Slash"),
    }
}

/// The key of the choice row for the line ending at `i`.
fn ending_key(i: usize) -> String {
    format!("ending.{i}")
}

/// The dialog's opening values for the comment `o`.
fn opening(o: &CommentProps) -> DialogFields {
    let mut f = DialogFields::new();
    if let Some(icon) = o.icon {
        f.set("icon", icon.name());
    }
    if let Some(c) = o.color {
        f.set(COLOUR, field_colour(c));
    }
    if let Some(c) = o.border_color {
        f.set(BORDER, field_colour(c));
    }
    if let Some(c) = o.fill {
        f.set(FILL, field_colour(c));
    }
    if let Some(shape) = o.border_shape {
        f.set("border_shape", shape.name());
    }
    if let Some(size) = o.font_size {
        f.set("font_size", FieldValue::Number(size));
    }
    f.set("opacity", FieldValue::Number(o.opacity * 100.0));
    if let Some(t) = o.text_opacity {
        f.set("text_opacity", FieldValue::Number(t * 100.0));
    }
    if let Some(fo) = o.fill_opacity {
        f.set("fill_opacity", FieldValue::Number(fo * 100.0));
    }
    if let Some(w) = o.width {
        f.set("width", FieldValue::Number(w));
    }
    for (i, ending) in o.endings.iter().flatten().enumerate() {
        f.set(ending_key(i), ending.name());
    }
    f.set("author", o.author.clone());
    f.set("subject", o.subject.clone());
    f.set("locked", FieldValue::Bool(o.locked));
    f.set(MAKE_DEFAULT, FieldValue::Bool(false));
    f
}

/// The comment's properties after the dialog's `fields`, starting from `original`. The Text opacity
/// row, when the text is not drawn separately (see [`parts_shown`]), shows the text's opacity (else
/// the whole comment's) and a change of it sets the whole comment's opacity and the text's
/// together; with the text drawn separately it writes the text's opacity and Border opacity writes
/// the comment's.
pub fn edited(original: &CommentProps, fields: &DialogFields) -> CommentProps {
    let mut e = original.clone();
    let text = |key: &str| fields.text(key).map(str::to_string);
    let percent = |key: &str| fields.number(key).map(|v| v / 100.0);
    if let Some(name) = text("icon").filter(|_| e.icon.is_some()) {
        e.icon = NoteIcon::ALL.into_iter().find(|i| i.name() == name).or(e.icon);
    }
    if e.color.is_some() || fields.color(COLOUR).is_some() {
        e.color = fields.color(COLOUR).map(rgb).or(e.color);
    }
    if e.font_size.is_some() {
        e.border_color = fields.color(BORDER).map(rgb);
        e.font_size = fields.number("font_size").or(e.font_size);
    }
    if let Some(name) = text("border_shape").filter(|_| e.border_shape.is_some()) {
        e.border_shape = BorderShape::ALL.into_iter().find(|s| s.name() == name).or(e.border_shape);
    }
    if e.fillable {
        e.fill = fields.color(FILL).map(rgb);
    }
    if let Some(w) = fields.number("width").filter(|_| e.width.is_some()) {
        e.width = Some(w);
    }
    if let Some(fo) = percent("fill_opacity").filter(|_| e.fill_opacity.is_some()) {
        e.fill_opacity = Some(fo);
    }
    let whole = percent("opacity").unwrap_or(original.opacity);
    match original.text_opacity {
        Some(_) if parts_shown(original).1 => {
            e.text_opacity = percent("text_opacity").or(original.text_opacity);
            e.opacity = whole;
        }
        Some(own) => {
            let before = own;
            let shown = percent("text_opacity").unwrap_or(before);
            if (shown - before).abs() > 1e-6 {
                e.opacity = shown;
                e.text_opacity = Some(shown);
            }
        }
        None => e.opacity = whole,
    }
    if let Some(ends) = e.endings.as_mut() {
        for (i, ending) in ends.iter_mut().enumerate() {
            if let Some(name) = text(&ending_key(i)) {
                *ending = LineEnding::ALL.into_iter().find(|l| l.name() == name).unwrap_or(*ending);
            }
        }
    }
    e.author = text("author").unwrap_or(e.author);
    e.subject = text("subject").unwrap_or(e.subject);
    e.locked = fields.bool("locked").unwrap_or(e.locked);
    e
}

/// A colour row: the swatch palette over the colour kept under `key`, which the host's colour
/// picker answers into.
fn colour_row(key: &'static str, label: &'static str, optional: Optional, recent: RecentColours) -> Param {
    Param::custom(key, label, move |ui, fields, cx| {
        let current = fields.color(key).map(rgb);
        let request = match optional {
            Optional::No => {
                let pick = palette(ui, label, current, key, &recent);
                if let Some(c) = pick.picked {
                    fields.set(key, field_colour(c));
                }
                pick.request
            }
            Optional::Border | Optional::Fill => {
                let shown =
                    if optional == Optional::Border { border_choice(ui, current, key, &recent) } else { fill_choice(ui, current, key, &recent) };
                if let Some(choice) = shown.choice {
                    set_optional(fields, key, choice);
                }
                shown.request
            }
        };
        if request.is_some() {
            cx.pick_color(key, Srgba::from_rgb_array(fields.color(key).unwrap_or([0.0; 3])));
        }
        ui.response()
    })
}

/// Which colour row a [`colour_row`] is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Optional {
    No,
    Border,
    Fill,
}

/// Keeps `choice` under `key`; no colour removes the value.
fn set_optional(fields: &mut DialogFields, key: &str, choice: Option<pdfcraft_engine::Rgb>) {
    match choice {
        Some(c) => fields.set(key, field_colour(c)),
        None => {
            fields.remove(key);
        }
    }
}

/// An opacity row in percent; `enabled` false shows a part that is not drawn.
fn opacity_row(key: &str, label: &'static str, enabled: bool) -> Param {
    let row = Param::new(key, label, ParamKind::range(0.0, 100.0, 100.0, SliderScale::Linear)).unit(UnitKind::Percent);
    if enabled { row } else { row.disabled() }
}

/// What the dialog shows of the comment: the three tabs over `fields`.
fn comment_props_dialog<'b>(
    fields: &'b mut DialogFields,
    o: &CommentProps,
    history: &[(String, String)],
    has_default: bool,
    selected: usize,
    recent: &RecentColours,
) -> ParamDialog<'b> {
    let kind = tl!(crate::comments_panel::subtype_label(&o.subtype)).to_string();
    let title = crate::i18n::fmt(tl!("{kind} Properties"), &[("kind", &kind)]);
    let mut rows = Vec::new();
    if o.icon.is_some() {
        let options = NoteIcon::ALL.into_iter().map(|i| (i.name().to_string(), i.name().into())).collect();
        rows.push(PropertyRow::field(Param::new("icon", tl!("Icon"), ParamKind::choice(options, None))));
    }
    rows.push(PropertyRow::field(colour_row(COLOUR, tl!("Colour"), Optional::No, recent.clone())));
    if o.font_size.is_some() {
        rows.push(PropertyRow::field(colour_row(BORDER, tl!("Border colour"), Optional::Border, recent.clone())));
    }
    if o.border_shape.is_some() {
        let options = BorderShape::ALL.into_iter().map(|s| (s.name().to_string(), tl!(s.name()).into())).collect();
        rows.push(PropertyRow::field(Param::new("border_shape", tl!("Border shape"), ParamKind::choice(options, None))));
    }
    if o.fillable {
        rows.push(PropertyRow::field(colour_row(FILL, tl!("Fill colour"), Optional::Fill, recent.clone())));
    }
    if o.font_size.is_some() {
        rows.push(PropertyRow::field(
            Param::number("font_size", tl!("Font size"), 12.0, 0.5, 1).bounds(Some(1.0), Some(400.0)).unit(UnitKind::Points),
        ));
    }
    // One opacity for the whole comment, or each part's of its own; a part not drawn is disabled.
    let (fill, text) = parts_shown(o);
    match (o.text_opacity.is_some(), o.fill_opacity.is_some()) {
        (true, _) => {
            rows.push(PropertyRow::field(opacity_row("text_opacity", tl!("Text opacity"), true)));
            rows.push(PropertyRow::field(opacity_row("opacity", tl!("Border opacity"), text && has_border(o))));
        }
        (false, true) => rows.push(PropertyRow::field(opacity_row("opacity", tl!("Line opacity"), true))),
        (false, false) => rows.push(PropertyRow::field(opacity_row("opacity", tl!("Opacity"), true))),
    }
    if o.fill_opacity.is_some() {
        rows.push(PropertyRow::field(opacity_row("fill_opacity", tl!("Fill opacity"), fill)));
    }
    if o.width.is_some() {
        let min_width = if o.font_size.is_some() { 0.0 } else { 0.5 };
        rows.push(PropertyRow::field(
            Param::new("width", tl!("Thickness"), ParamKind::range(min_width, crate::comments::MAX_LINE_WIDTH, 1.0, SliderScale::Linear))
                .decimals(1)
                .unit(UnitKind::Points),
        ));
    }
    for i in 0..o.endings.as_ref().map_or(0, Vec::len) {
        let options = LineEnding::ALL.into_iter().map(|l| (l.name().to_string(), ending_label(l).into())).collect();
        let label = if i == 0 { tl!("Line ending").to_string() } else { crate::i18n::fmt(tl!("Line ending {n}"), &[("n", &(i + 1).to_string())]) };
        rows.push(PropertyRow::field(Param::new(ending_key(i), label, ParamKind::choice(options, None))));
    }
    let appearance = PropertiesTab::new(tl!("Appearance"), rows);
    let appearance =
        if o.restylable { appearance } else { appearance.read_only(Some(tl!("This comment's appearance can't be changed yet.").into())) };

    let modified = o.modified.as_deref().map(pdfcraft_render::pretty_date).unwrap_or_else(|| "—".into());
    let general = PropertiesTab::new(
        tl!("General"),
        vec![
            PropertyRow::field(Param::new("author", tl!("Author"), ParamKind::text(""))),
            PropertyRow::field(Param::new("subject", tl!("Subject"), ParamKind::text(""))),
            PropertyRow::field(Param::read_only_text("modified", tl!("Modified"), modified)),
        ],
    );
    let review = if history.is_empty() {
        vec![PropertyRow::value(tl!("Status"), tl!("No status has been set."))]
    } else {
        history.iter().map(|(state, line)| PropertyRow::value(tl!(state).to_string(), line.clone())).collect()
    };
    let review = PropertiesTab::new(tl!("Review History"), review);

    // Acrobat's box beside Locked: only a comment whose look can change, made by a tool (or typed
    // text, for Fill & Sign).
    let default_row = Param::new(MAKE_DEFAULT, tl!("Make Properties Default"), ParamKind::bool(false));
    let default_row = if o.restylable && has_default { default_row } else { default_row.disabled() };
    let shared = vec![PropertyRow::field(Param::new("locked", tl!("Locked"), ParamKind::bool(false))), PropertyRow::field(default_row)];
    let dialog = PropertiesDialog::new(KEY, title, vec![appearance, general, review], Bind::bound(fields))
        .shared(shared)
        .shared_rows(SharedRows::Below)
        .into_param_dialog();
    if selected > 1 {
        let n = selected.to_string();
        dialog.title_note(crate::i18n::fmt(tl!("Changes here apply to all {n} selected comments"), &[("n", &n)]))
    } else {
        dialog
    }
}

impl PdfCraftApp {
    /// Attach file: ask for a file and attach it at `at`, on the document the pick started on.
    pub fn attach_file_comment(&mut self, page: usize, at: [f64; 2]) {
        let target = self.pick_target(self.active_ids().map(|(_, id)| id));
        self.pick_file(PickRequest::open("attach").title(tl!("Attach a file")), target, move |app, ctx, picked| match picked {
            Picked::Paths(paths) => {
                let Some(p) = paths.into_iter().next() else { return };
                match std::fs::read(&p) {
                    Ok(data) => {
                        app.add_attachment_comment(ctx, page, at, p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), data)
                    }
                    Err(e) => app.notify_fmt("Couldn't read {name}: {e}", &[("name", &p.display().to_string()), ("e", &e.to_string())]),
                }
            }

            Picked::Contents(files) => {
                if let Some((file, data)) = files.into_iter().next() {
                    app.add_attachment_comment(ctx, page, at, file, data);
                }
            }
            _ => {}
        });
    }

    /// Add a file attachment comment at `at` on `page` of the active document.
    fn add_attachment_comment(&mut self, ctx: &egui::Context, page: usize, at: [f64; 2], file: String, data: Vec<u8>) {
        let tool = crate::comments::CommentTool::Attach;
        let shape = pdfcraft_engine::Shape::Attachment { at, icon: pdfcraft_engine::AttachIcon::PushPin, file, data };
        let edit = Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
            page,
            shape,
            style: self.comment_prefs.style(tool),
            contents: String::new(),
            author: self.comment_prefs.author.clone(),
        });
        // A one-shot tool: back to Select unless pinned (apply_edit acts on this).
        if let Some((i, _)) = self.active_ids() {
            self.views[i].session.comments.tool_done = true;
        }
        self.apply_edit(ctx, edit);
    }

    /// Make Current Properties Default: new comments of this kind take this one's colour,
    /// opacity and line width.
    pub fn make_comment_default(&mut self, page: usize, index: usize) {
        let Some(p) = self.active_ids().and_then(|(_, id)| self.session.get(id)).and_then(|doc| doc.comment_props(page, index)) else { return };
        self.make_props_default(page, index, &p);
    }

    /// New comments of the kind at `(page, index)` take the look in `p`: its colour, opacity,
    /// line width, (for shapes) fill and (for text boxes and callouts) font size. `p` may be a Comment Properties draft not applied yet.
    pub fn make_props_default(&mut self, page: usize, index: usize, p: &CommentProps) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let Some(a) = doc.info.annotations.iter().find(|a| a.page == page && a.index == index && !a.is_reply()) else { return };
        // Typed text has no comment tool: Fill & Sign gives new text its size, colour and border,
        // kept across launches.
        if crate::comments::typed_text(a) {
            let kind = crate::i18n::in_sentence(tl!(crate::comments_panel::kind_label(a))).to_string();
            if let Some(size) = p.font_size.and_then(crate::fill_sign::valid_text_size) {
                self.fill_text_size = size;
            }
            if let Some(color) = p.color.and_then(crate::fill_sign::valid_text_color) {
                self.fill_text_color = color;
            }
            let width = p.width.and_then(crate::fill_sign::valid_text_border).unwrap_or(0.0);
            let border = crate::fill_sign::TextBorder { width, color: p.border_color, shape: drawn_border_shape(p).unwrap_or_default() };
            self.fill_text_default = border;
            (self.fill_text_border, self.fill_text_border_color, self.fill_text_border_shape) = (border.width, border.color, border.shape);
            self.notify_fmt("New {tool} comments will look like this one", &[("tool", &kind)]);
            return;
        }
        let Some(tool) = crate::comments::tool_for_default(&doc.info.annotations, a, p.endings.as_deref()) else { return };
        let mut style = self.comment_prefs.style(tool);
        if let Some(c) = p.color {
            style.color = c;
        }
        if p.fillable && tool.has_fill() {
            style.fill = p.fill;
        }
        self.comment_prefs.set_style(tool, style);
        // A text box's or callout's own border colour (cleared for tools that have none).
        self.comment_prefs.set_border_color(tool, p.border_color);
        if let Some(shape) = drawn_border_shape(p) {
            self.comment_prefs.set_border_shape(tool, shape);
        }
        // The fill's and text's own opacities where they show (else the opacity is the whole
        // comment's). With neither a border nor a fill the text is the whole comment: its drawn
        // opacity, which Comment Properties shows, is the whole comment's default.
        let (fill, text) = parts_shown(p);
        // Keep invisible defaults out of tools where a zero stroke would make them disappear.
        self.comment_prefs.set_opacity(tool, p.text_opacity.filter(|_| !text).unwrap_or(p.opacity));
        self.comment_prefs.set_part_opacity(tool, p.fill_opacity.filter(|_| fill), p.text_opacity.filter(|_| text));
        if let Some(w) = p.width {
            self.comment_prefs.set_width(tool, w);
        }
        if let Some(size) = p.font_size {
            self.comment_prefs.set_font_size(tool, size);
        }
        self.notify_fmt("New {tool} comments will look like this one", &[("tool", &crate::i18n::in_sentence(tl!(tool.label())))]);
    }

    /// Give every selected comment of the active document `color`, as one undo step.
    pub fn colour_selected_comments(&mut self, ctx: &egui::Context, color: pdfcraft_engine::Rgb) {
        self.colour_selection(ctx, color, |_| true);
    }

    /// Give the selected comments that `only` picks `color`, as one undo step: those whose look can
    /// change and that aren't locked (a locked one would refuse the whole step).
    pub(crate) fn colour_selection(&mut self, ctx: &egui::Context, color: pdfcraft_engine::Rgb, only: impl Fn(&pdfcraft_render::Annotation) -> bool) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let keys = self.views.get(i).map(|v| v.session.comments.selection()).unwrap_or_default();
        let mut edits: Vec<Edit> = keys
            .into_iter()
            .filter(|(p, ix)| doc.info.annotations.iter().any(|a| a.page == *p && a.index == *ix && !a.is_reply() && only(a)))
            .filter(|(p, ix)| doc.comment_props(*p, *ix).is_some_and(|c| c.restylable && !c.locked))
            .map(|(page, index)| Edit::StyleAnnotation { page, index, color: Some(color), opacity: None, width: None, endings: None })
            .collect();
        match edits.len() {
            0 => {}
            1 => {
                self.apply_edit(ctx, edits.remove(0));
            }
            _ => {
                self.apply_edit(ctx, Edit::Batch { label: "Change comment properties".into(), edits });
            }
        }
    }

    /// Open Comment Properties for the comment at `(page, index)` of the active document. When it
    /// is one of several selected comments, the changes go to all of them.
    pub fn open_comment_props(&mut self, ctx: &egui::Context, page: usize, index: usize) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let Some(p) = doc.comment_props(page, index) else { return };
        let selection =
            self.views.get(i).map(|v| &v.session.comments).filter(|c| c.is_selected((page, index))).map(|c| c.selection()).unwrap_or_default();
        let others: Vec<(usize, usize, CommentProps)> =
            selection.into_iter().filter(|k| *k != (page, index)).filter_map(|(pg, ix)| doc.comment_props(pg, ix).map(|c| (pg, ix, c))).collect();
        let me = doc.info.annotations.iter().find(|a| a.page == page && a.index == index && !a.is_reply());
        // Every state change set on it, including a reviewer's later ones, which reply to their
        // earlier (ISO 32000-2 §12.5.6.3).
        let states = pdfcraft_render::ReviewStates::new(&doc.info.annotations);
        let history: Vec<(String, String)> = me
            .map(|a| states.history(a))
            .unwrap_or_default()
            .iter()
            .map(|r| {
                (r.state.clone().unwrap_or_default(), format!("{}  {}", r.author.clone().unwrap_or_default(), r.modified.clone().unwrap_or_default()))
            })
            .collect();
        let has_default = me.is_some_and(|a| {
            crate::comments::tool_for_default(&doc.info.annotations, a, p.endings.as_deref()).is_some() || crate::comments::typed_text(a)
        });
        let selected = others.len() + 1;
        let original = p.clone();
        let recent = self.recent_colours.clone();
        let dialog = HostedParamDialog::new(ctx, KEY, opening(&p), move |fields| {
            comment_props_dialog(fields, &original, &history, has_default, selected, &recent)
        })
        .tab(0);
        let id = crate::dialogs::host::open(self, dialog);
        self.comment_props = Some(CommentPropsDialog { id, draft: PropsDraft { page, index, original: p, others } });
    }

    /// One pass over the open Comment Properties dialog: forwards a colour row's request to the
    /// colour picker, and on OK applies the edits (one undo step) and Make Properties Default.
    pub fn comment_props_pass(&mut self, ctx: &egui::Context) {
        let Some(id) = self.comment_props.as_ref().map(|d| d.id) else { return };
        let requested = self.dialogs.get_mut_by_id::<HostedParamDialog>(id).and_then(HostedParamDialog::pick_color);
        if let Some(request) = requested {
            let key = request.key.clone();
            self.open_colour_picker(PickerHost::App, request, move |app, _: &egui::Context, color: Srgba| {
                if let Some(d) = app.dialogs.get_mut_by_id::<HostedParamDialog>(id) {
                    d.set_field(key, FieldValue::Color([color.r, color.g, color.b]));
                }
            });
        }
        match self.dialogs.take_by_id::<HostedParamDialog>(id) {
            None => {}
            Some(DialogResult::Ok(result)) => {
                let Some(open) = self.comment_props.take() else { return };
                let draft = open.draft;
                let e = edited(&draft.original, &result.fields);
                let mut edits = edits(&draft, &e);
                match edits.len() {
                    0 => {}
                    1 => {
                        self.apply_edit(ctx, edits.remove(0));
                    }
                    _ => {
                        self.apply_edit(ctx, Edit::Batch { label: "Change comment properties".into(), edits });
                    }
                }
                // Make Properties Default: the look chosen here, even when it was already the comment's.
                if result.fields.bool(MAKE_DEFAULT) == Some(true) && e.restylable {
                    self.make_props_default(draft.page, draft.index, &e);
                }
            }
            Some(_) => self.comment_props = None,
        }
    }
}

/// The border shape `p` draws: a Thickness of 0 takes an oval or a circle away with the line, as
/// the toolbar's Border ▸ None does.
fn drawn_border_shape(p: &CommentProps) -> Option<pdfcraft_engine::BorderShape> {
    p.border_shape.map(|s| if p.width.is_some_and(|w| w <= 0.0) { pdfcraft_engine::BorderShape::Rectangle } else { s })
}

/// Which of the comment's parts show an opacity of their own: (its fill, its text). A text box's
/// text, border and fill each do once it has a border or a fill; a filled shape's line and fill
/// do; anything else has one opacity for the whole comment.
pub fn parts_shown(p: &CommentProps) -> (bool, bool) {
    let fill = p.fill_opacity.is_some() && p.fill.is_some();
    let text = p.text_opacity.is_some() && (fill || has_border(p));
    (fill, text)
}

/// A text box's, typewriter text's or callout's border is drawn.
fn has_border(p: &CommentProps) -> bool {
    p.font_size.is_some() && p.width.is_some_and(|w| w > 0.0)
}

/// The edits that make the comment match `e` (empty when nothing changed), and make the
/// same changes to the other selected comments where they apply. Locks come off first and go on
/// last, as a locked comment refuses other changes; another comment that stays locked is left
/// as it is.
pub fn edits(d: &PropsDraft, e: &CommentProps) -> Vec<Edit> {
    let o = &d.original;
    let targets: Vec<(usize, usize, &CommentProps)> =
        std::iter::once((d.page, d.index, o)).chain(d.others.iter().map(|(p, i, c)| (*p, *i, c))).collect();
    let mut out = Vec::new();
    // Unticking Locked unlocks every selected comment.
    let unlocking = o.locked && !e.locked;
    for &(page, index, t) in &targets {
        if unlocking && t.locked {
            out.push(Edit::LockAnnotation { page, index, locked: false });
        }
    }
    for (n, &(page, index, t)) in targets.iter().enumerate() {
        if n > 0 && t.locked && !unlocking {
            continue;
        }
        changes(o, e, page, index, t, &mut out);
    }
    for &(page, index, t) in &targets {
        if !o.locked && e.locked && !t.locked {
            out.push(Edit::LockAnnotation { page, index, locked: true });
        }
    }
    out
}

/// The changes from `o` to `e` (the draft) for the comment at `(page, index)`, whose own
/// properties are `t`: only those its kind has (a line width for shapes and lines, a fill for
/// shapes, a font size for text, …).
fn changes(o: &CommentProps, e: &CommentProps, page: usize, index: usize, t: &CommentProps, out: &mut Vec<Edit>) {
    let color = (e.color != o.color).then_some(e.color).flatten();
    let opacity = ((e.opacity - o.opacity).abs() > 1e-6).then_some(e.opacity);
    let width = (e.width != o.width).then_some(e.width).flatten().filter(|_| t.width.is_some());
    let endings =
        (e.endings != o.endings).then(|| e.endings.clone()).flatten().filter(|ends| t.endings.as_ref().is_some_and(|own| own.len() == ends.len()));
    if t.restylable && (color.is_some() || opacity.is_some() || width.is_some() || endings.is_some()) {
        out.push(Edit::StyleAnnotation { page, index, color, opacity, width, endings });
    }
    if t.restylable && t.fillable && e.fill != o.fill {
        out.push(Edit::FillAnnotation { page, index, fill: e.fill });
    }
    // After the opacity, which is the whole comment's: the parts shown are set again. Which
    // parts show is the comment's own once the draft's fill and line width are on it, so another
    // selected comment without a border or a fill keeps one opacity for the whole comment.
    let mut after = t.clone();
    if t.fillable && e.fill != o.fill {
        after.fill = e.fill;
    }
    if let Some(w) = width {
        after.width = Some(w);
    }
    let (fill, text) = parts_shown(&after);
    let moved = |a: Option<f64>, b: Option<f64>| a.zip(b).is_some_and(|(a, b)| (a - b).abs() > 1e-6);
    if t.restylable
        && (fill || text)
        && (opacity.is_some() || fill && moved(e.fill_opacity, o.fill_opacity) || text && moved(e.text_opacity, o.text_opacity))
    {
        out.push(Edit::PartOpacity { page, index, fill: e.fill_opacity.filter(|_| fill), text: e.text_opacity.filter(|_| text) });
    }
    if t.restylable
        && t.font_size.is_some()
        && let Some(size) = e.font_size.filter(|s| Some(*s) != o.font_size)
    {
        out.push(Edit::FontSizeAnnotation { page, index, size });
    }
    if t.restylable && t.font_size.is_some() && e.border_color != o.border_color {
        out.push(Edit::BorderColorAnnotation { page, index, color: e.border_color });
    }
    // After the width (StyleAnnotation), whose border the shape goes around: each selected
    // text keeps its own shape unless the draft's changed, and loses it with its border.
    let mut shaped = after;
    if e.border_shape != o.border_shape {
        shaped.border_shape = e.border_shape;
    }
    if t.restylable
        && t.border_shape.is_some()
        && (e.border_shape != o.border_shape || width.is_some())
        && let Some(shape) = drawn_border_shape(&shaped).filter(|s| Some(*s) != t.border_shape)
    {
        out.push(Edit::BorderShapeAnnotation { page, index, shape });
    }
    let author = (e.author != o.author).then(|| e.author.clone());
    let subject = (e.subject != o.subject).then(|| e.subject.clone());
    let icon = (e.icon != o.icon).then_some(e.icon).flatten().filter(|_| t.icon.is_some());
    if author.is_some() || subject.is_some() || icon.is_some() {
        out.push(Edit::SetAnnotationInfo { page, index, author, subject, icon });
    }
}
