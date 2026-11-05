//! Comment Properties (Acrobat: right-click a comment ▸ Properties…; audit "Comment
//! properties"): Appearance (colour, fill, opacity, line thickness, note icon), General (author,
//! subject, modified) and Review History (status changes), with Acrobat's Locked box.

use egui::{Align, Layout};
use pdfcraft_engine::{CommentProps, Edit, LineEnding, NoteIcon};

use crate::comments::swatch_grid;
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropsTab {
    Appearance,
    General,
    ReviewHistory,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PropsDraft {
    pub page: usize,
    pub index: usize,
    pub tab: PropsTab,
    pub original: CommentProps,
    pub edited: CommentProps,
    /// Acrobat's Make Properties Default box: on OK, new comments of this kind take the edited
    /// colour, opacity and line width too.
    pub make_default: bool,
    /// The other selected comments, each with its own properties: OK makes the same changes to
    /// them where they apply (a colour to every one, a font size to text boxes, …).
    pub others: Vec<(usize, usize, CommentProps)>,
}

const ICONS: [NoteIcon; 14] = NoteIcon::ALL;

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

impl PdfCraftApp {
    /// Attach file: ask for a file (or take `attach_override`) and attach it at `at`.
    pub fn attach_file_comment(&mut self, page: usize, at: [f64; 2]) {
        match self.attach_override.take() {
            Some((file, data)) => self.add_attachment_comment(page, at, file, data),
            #[cfg(not(target_arch = "wasm32"))]
            None => {
                // Attached on a later frame, and only to the document it was started on.
                let target = self.active_ids().map(|(_, id)| id);
                let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Attach a file"));
                self.ask_one(crate::pickers::Ask::File(dialog), target, move |app, p| match std::fs::read(&p) {
                    Ok(data) => {
                        app.add_attachment_comment(page, at, p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), data)
                    }
                    Err(e) => app.notify_fmt("Couldn't read {name}: {e}", &[("name", &p.display().to_string()), ("e", &e.to_string())]),
                });
            }
            #[cfg(target_arch = "wasm32")]
            None => {}
        }
    }

    /// Add a file attachment comment at `at` on `page` of the active document.
    fn add_attachment_comment(&mut self, page: usize, at: [f64; 2], file: String, data: Vec<u8>) {
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
            self.views[i].comments.tool_done = true;
        }
        self.apply_edit(edit);
    }

    /// Make Current Properties Default: new comments of this kind take this one's colour,
    /// opacity and line width.
    pub fn make_comment_default(&mut self, page: usize, index: usize) {
        let Some(p) = self.active_ids().and_then(|(_, id)| self.session.get(id)).and_then(|doc| doc.comment_props(page, index)) else { return };
        self.make_props_default(page, index, &p);
    }

    /// New comments of the kind at `(page, index)` take the look in `p`: its colour, opacity,
    /// line width and (for shapes) fill. `p` may be a Comment Properties draft not applied yet.
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
            self.fill_text_default = crate::fill_sign::TextBorder { width, color: p.border_color };
            (self.fill_text_border, self.fill_text_border_color) = (width, p.border_color);
            self.notify_fmt("New {tool} comments will look like this one", &[("tool", &kind)]);
            return;
        }
        let Some(tool) = crate::comments::tool_for_default(a, p.endings.as_deref()) else { return };
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
        self.notify_fmt("New {tool} comments will look like this one", &[("tool", &crate::i18n::in_sentence(tl!(tool.label())))]);
    }

    /// Give every selected comment of the active document `color`, as one undo step.
    pub fn colour_selected_comments(&mut self, color: pdfcraft_engine::Rgb) {
        self.colour_selection(color, |_| true);
    }

    /// Give the selected comments that `only` picks `color`, as one undo step: those whose look can
    /// change and that aren't locked (a locked one would refuse the whole step).
    pub(crate) fn colour_selection(&mut self, color: pdfcraft_engine::Rgb, only: impl Fn(&pdfcraft_render::Annotation) -> bool) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let keys = self.views.get(i).map(|v| v.comments.selection()).unwrap_or_default();
        let mut edits: Vec<Edit> = keys
            .into_iter()
            .filter(|(p, ix)| doc.info.annotations.iter().any(|a| a.page == *p && a.index == *ix && !a.is_reply() && only(a)))
            .filter(|(p, ix)| doc.comment_props(*p, *ix).is_some_and(|c| c.restylable && !c.locked))
            .map(|(page, index)| Edit::StyleAnnotation { page, index, color: Some(color), opacity: None, width: None, endings: None })
            .collect();
        match edits.len() {
            0 => {}
            1 => {
                self.apply_edit(edits.remove(0));
            }
            _ => {
                self.apply_edit(Edit::Batch { label: "Change comment properties".into(), edits });
            }
        }
    }

    /// Open Comment Properties for the comment at `(page, index)` of the active document. When it
    /// is one of several selected comments, the changes go to all of them.
    pub fn open_comment_props(&mut self, page: usize, index: usize) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let Some(p) = doc.comment_props(page, index) else { return };
        let selection = self.views.get(i).map(|v| &v.comments).filter(|c| c.is_selected((page, index))).map(|c| c.selection()).unwrap_or_default();
        let others =
            selection.into_iter().filter(|k| *k != (page, index)).filter_map(|(pg, ix)| doc.comment_props(pg, ix).map(|c| (pg, ix, c))).collect();
        self.comment_props = Some(PropsDraft { page, index, tab: PropsTab::Appearance, original: p.clone(), edited: p, make_default: false, others });
        self.dialog = Some(crate::Dialog::CommentProps);
    }
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

/// The edits that make the comment match the draft (empty when nothing changed), and make the
/// same changes to the other selected comments where they apply. Locks come off first and go on
/// last, as a locked comment refuses other changes; another comment that stays locked is left
/// as it is.
pub fn edits(d: &PropsDraft) -> Vec<Edit> {
    let (o, e) = (&d.original, &d.edited);
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
    let author = (e.author != o.author).then(|| e.author.clone());
    let subject = (e.subject != o.subject).then(|| e.subject.clone());
    let icon = (e.icon != o.icon).then_some(e.icon).flatten().filter(|_| t.icon.is_some());
    if author.is_some() || subject.is_some() || icon.is_some() {
        out.push(Edit::SetAnnotationInfo { page, index, author, subject, icon });
    }
}

/// Draw the dialog; returns (apply, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let history: Vec<(String, String, String)> = app
        .active_ids()
        .and_then(|(_, id)| app.session.get(id))
        .zip(app.comment_props.as_ref())
        .map(|(doc, d)| {
            let me = doc.info.annotations.iter().find(|a| a.page == d.page && a.index == d.index && !a.is_reply());
            doc.info
                .annotations
                .iter()
                .filter(|r| r.state.is_some() && me.is_some_and(|a| r.replies_to(a)))
                .map(|r| (r.state.clone().unwrap_or_default(), r.author.clone().unwrap_or_default(), r.modified.clone().unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default();
    let has_default = app.comment_props.as_ref().is_some_and(|d| {
        let doc = app.active_ids().and_then(|(_, id)| app.session.get(id));
        doc.and_then(|doc| doc.info.annotations.iter().find(|a| a.page == d.page && a.index == d.index && !a.is_reply()))
            .is_some_and(|a| crate::comments::tool_for_default(a, d.edited.endings.as_deref()).is_some() || crate::comments::typed_text(a))
    });
    let Some(d) = app.comment_props.as_mut() else { return (false, true) };
    let kind = tl!(crate::comments_panel::subtype_label(&d.edited.subtype)).to_string();
    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("{kind} Properties"), &[("kind", &kind)])).font(theme::semibold(18.0)));
    if !d.others.is_empty() {
        let n = (d.others.len() + 1).to_string();
        ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Changes here apply to all {n} selected comments"), &[("n", &n)])).color(t.text_muted));
    }
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        for (tab, label) in
            [(PropsTab::Appearance, tl!("Appearance")), (PropsTab::General, tl!("General")), (PropsTab::ReviewHistory, tl!("Review History"))]
        {
            if widgets::mode_tab(ui, label, d.tab == tab).clicked() {
                d.tab = tab;
            }
        }
    });
    ui.separator();
    ui.add_space(6.0);
    let e = &mut d.edited;
    match d.tab {
        PropsTab::Appearance => {
            if !e.restylable {
                ui.label(egui::RichText::new(tl!("This comment's appearance can't be changed yet.")).color(t.text_muted));
            }
            ui.add_enabled_ui(e.restylable, |ui| {
                egui::Grid::new("comment-props").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                    if let Some(icon) = e.icon.as_mut() {
                        ui.label(tl!("Icon"));
                        craft_ui::ComboBox::from_id_salt("note-icon").selected_text(icon.name()).show_ui(ui, |ui| {
                            for i in ICONS {
                                ui.selectable_value(icon, i, i.name());
                            }
                        });
                        ui.end_row();
                    }
                    ui.label(tl!("Colour"));
                    if let Some(c) = swatch_grid(ui, e.color) {
                        e.color = Some(c);
                    }
                    ui.end_row();
                    // Text boxes, typewriter text and callouts: the border's own colour.
                    if e.font_size.is_some() {
                        ui.label(tl!("Border colour"));
                        let text = e.color.unwrap_or([0.0; 3]);
                        ui.horizontal(|ui| {
                            if let Some(c) = crate::comments::border_colour_button(ui, e.border_color, text) {
                                e.border_color = c;
                            }
                        });
                        ui.end_row();
                    }
                    if e.fillable {
                        ui.label(tl!("Fill colour"));
                        if let Some(f) = crate::comments::fill_picker(ui, e.fill) {
                            e.fill = f;
                        }
                        ui.end_row();
                    }
                    if let Some(size) = e.font_size.as_mut() {
                        ui.label(tl!("Font size"));
                        ui.add(egui::DragValue::new(size).range(1.0..=400.0).speed(0.5).suffix(" pt"));
                        ui.end_row();
                    }
                    // One opacity for the whole comment, or each part's of its own: a text box's
                    // text, border and fill, a shape's line and fill. The rows stay put; a part
                    // not drawn (no fill, no border) is greyed.
                    let percent = |ui: &mut egui::Ui, label: &str, value: &mut f64, enabled: bool| {
                        let name = ui.add_enabled(enabled, egui::Label::new(label));
                        let mut pct = *value * 100.0;
                        if ui.add_enabled(enabled, egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%")).labelled_by(name.id).changed() {
                            *value = pct / 100.0;
                        }
                        ui.end_row();
                    };
                    let (fill, text) = parts_shown(e);
                    let border = has_border(e);
                    match (e.text_opacity.is_some(), e.fill_opacity.is_some()) {
                        (true, _) => {
                            // With neither a border nor a fill, the text is the whole comment:
                            // shown at the opacity it is drawn at, a change sets the whole
                            // comment's (which its parts then take).
                            match e.text_opacity.as_mut() {
                                Some(t) if text => percent(ui, tl!("Text opacity"), t, true),
                                drawn => {
                                    let mut shown = drawn.as_deref().copied().unwrap_or(e.opacity);
                                    let before = shown;
                                    percent(ui, tl!("Text opacity"), &mut shown, true);
                                    if shown != before {
                                        e.opacity = shown;
                                        if let Some(t) = drawn {
                                            *t = shown;
                                        }
                                    }
                                }
                            }
                            if text {
                                percent(ui, tl!("Border opacity"), &mut e.opacity, border);
                            } else {
                                let mut whole = e.opacity;
                                percent(ui, tl!("Border opacity"), &mut whole, false);
                            }
                        }
                        (false, true) => percent(ui, tl!("Line opacity"), &mut e.opacity, true),
                        (false, false) => percent(ui, tl!("Opacity"), &mut e.opacity, true),
                    }
                    if let Some(f) = e.fill_opacity.as_mut() {
                        percent(ui, tl!("Fill opacity"), f, fill);
                    }
                    let min_width = if e.font_size.is_some() { 0.0 } else { 0.5 };
                    if let Some(w) = e.width.as_mut() {
                        ui.label(tl!("Thickness"));
                        ui.add(egui::Slider::new(w, min_width..=crate::comments::MAX_LINE_WIDTH).step_by(0.5).suffix(" pt"));
                        ui.end_row();
                    }
                    if let Some(ends) = e.endings.as_mut() {
                        ui.label(tl!("Line ending"));
                        ui.horizontal(|ui| {
                            for (i, ending) in ends.iter_mut().enumerate() {
                                craft_ui::ComboBox::from_id_salt(("line-ending", i)).selected_text(ending_label(*ending)).width(188.0).show_ui(
                                    ui,
                                    |ui| {
                                        for style in LineEnding::ALL {
                                            ui.selectable_value(ending, style, ending_label(style));
                                        }
                                    },
                                );
                            }
                        });
                        ui.end_row();
                    }
                });
            });
        }
        PropsTab::General => {
            egui::Grid::new("comment-general").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                let l = ui.label(tl!("Author"));
                ui.add(crate::widgets::line(&mut e.author).desired_width(280.0)).labelled_by(l.id);
                ui.end_row();
                let l = ui.label(tl!("Subject"));
                ui.add(crate::widgets::line(&mut e.subject).desired_width(280.0)).labelled_by(l.id);
                ui.end_row();
                ui.label(tl!("Modified"));
                ui.label(e.modified.as_deref().map(pdfcraft_render::pretty_date).unwrap_or_else(|| "—".into()));
                ui.end_row();
            });
        }
        PropsTab::ReviewHistory => {
            if history.is_empty() {
                ui.label(egui::RichText::new(tl!("No status has been set.")).color(t.text_muted));
            }
            for (state, who, when) in &history {
                ui.label(format!("{} — {who}  {when}", tl!(state)));
            }
        }
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.add(widgets::checkbox(&mut d.edited.locked, tl!("Locked")));
        ui.add_space(16.0);
        // Acrobat's box beside Locked. Only a comment whose look can change, made by a tool (or
        // typed text, for Fill & Sign).
        let defaultable = d.edited.restylable && has_default;
        ui.add_enabled(defaultable, widgets::checkbox(&mut d.make_default, tl!("Make Properties Default")));
    });
    let (mut apply, mut cancel) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("OK"), true).clicked() {
            apply = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (apply, cancel)
}
