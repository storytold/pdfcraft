use super::tools::row_label;
use super::{
    Align2, Availability, Color32, CornerRadius, LeftPanel, PdfCraftApp, Rect, Sense, Stroke, Tokens, ToolGroup, ToolItem, catalog, icons, theme,
    vec2, widgets,
};

/// The page operations Edit shows as a row of icon buttons, as Acrobat's Edit panel does.
const EDIT_PAGE_BUTTONS: [&str; 4] = ["page.rotate", "page.insert", "page.delete", "page.extract"];

/// Add content items shown before More.
const EDIT_CONTENT_SHOWN: usize = 5;

/// Other options: the tools Acrobat's Edit panel links to.
const EDIT_OTHER_TOOLS: [&str; 3] = ["combine", "redact", "form"];

/// Edit a PDF, laid out as Acrobat's Edit panel: the title with Preferences and Close; Format text
/// while text is being edited or added; Modify page as a row of icon buttons and Organize pages;
/// Add content with the rarer items under More; and Other options, the neighbouring tools.
/// Edit text & images itself is on the floating toolbar, and on from the moment Edit opens.
/// Every item comes from the catalogue, so its label, icon and availability have one source.
pub(super) fn edit_panel(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, g: &'static ToolGroup) {
    let items = || g.sections.iter().flat_map(|s| s.items.iter());
    let item = |command: &str| items().find(|i| i.command == command);
    let mut run = None;
    let mut tool = None;
    // Back to All tools only when Edit was opened from there; the Edit tab is its own way back.
    let from_all_tools = app.mode != crate::Mode::Edit;
    ui.horizontal(|ui| {
        if from_all_tools && icons::button(ui, "chevron-left", 26.0, false, tl!("Back to all tools")).clicked() {
            app.left = LeftPanel::AllTools;
            app.enter_mode(crate::Mode::AllTools);
        }
        // "Edit" under the Edit tab, as Acrobat titles it; the tool's own name from All tools.
        let title = if from_all_tools { tl!(g.label) } else { tl!("Edit") };
        let title_width = (ui.available_width() - 2.0 * (26.0 + ui.spacing().item_spacing.x)).max(1.0);
        ui.add_sized([title_width, 26.0], egui::Label::new(egui::RichText::new(title).font(theme::semibold(15.5)).color(t.text)).truncate())
            .on_hover_text(title);
        if icons::button(ui, "settings", 26.0, false, tl!("Preferences")).clicked() {
            run = Some("app.preferences");
        }
        if icons::button(ui, "x", 26.0, false, tl!("Close panel")).clicked() {
            app.left_open = false;
        }
    });
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(6.0);
    // Several objects selected: how many, and how to move them. Acrobat's panel has no idle hint.
    let count = app.active_ids().filter(|_| app.quick_tool == crate::QuickTool::EditText).map_or(0, |(i, _)| app.views[i].objects.count());
    if count > 0 {
        ui.label(crate::i18n::trn(crate::i18n::current(), count as u64, "{n} object selected", "{n} objects selected"));
        ui.label(tl!("Drag the selection to move it. Esc clears the selection."));
        ui.add_space(6.0);
    }
    // Format text at the top while text is selected or being added.
    format_section(app, ui, t);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        widgets::section_title(ui, tl!("Modify page"));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for i in EDIT_PAGE_BUTTONS.iter().filter_map(|c| item(c)) {
                // Enabled as the command itself is: a document that forbids page assembly refuses these.
                let enabled = pdfcraft_engine::commands::command(i.command).is_some_and(|spec| app.command_enabled(spec));
                let resp = ui.add_enabled_ui(enabled, |ui| icons::button(ui, i.icon, 34.0, false, tl!(i.label))).inner;
                if resp.clicked() {
                    run = Some(i.command);
                }
            }
        });
        ui.add_space(2.0);
        // Organize pages is a workspace of its own: its panel replaces this one, with the grid.
        if let Some(i) = item("page.organize")
            && edit_row(ui, t, i.icon, tl!(i.label), i.availability).clicked()
        {
            // With no document there is no grid to open: the command asks for a file first.
            if app.active.is_some() {
                tool = Some("organize");
            } else {
                run = Some("page.organize");
            }
        }

        widgets::section_title(ui, tl!("Add content"));
        let content: Vec<_> =
            g.sections.iter().filter(|s| s.title == "Add content").flat_map(|s| s.items.iter()).filter(|i| i.command != "edit.edit_text").collect();
        let more_id = egui::Id::unique("edit_panel_more");
        let more = ui.data(|d| d.get_temp::<bool>(more_id)).unwrap_or(false);
        let shown = if more { content.len() } else { EDIT_CONTENT_SHOWN.min(content.len()) };
        for i in content.iter().take(shown) {
            let resp = edit_row(ui, t, i.icon, tl!(i.label), i.availability);
            // Header and footer, Watermark and Background open their Add / Update / Remove menu.
            if i.actions.is_empty() {
                if resp.clicked() {
                    run = Some(i.command);
                }
            } else if let Some(cmd) = actions_menu(app, ui, &resp, i, resp.rect.right_center() - vec2(14.0, 0.0), t.text) {
                run = Some(cmd);
            }
        }
        if content.len() > EDIT_CONTENT_SHOWN {
            ui.add_space(4.0);
            let label = if more { tl!("Less") } else { tl!("More") };
            let resp = ui
                .add(
                    egui::Label::new(egui::RichText::new(label).color(t.accent_text).font(theme::medium(13.0)))
                        .selectable(false)
                        .sense(Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.clicked() {
                ui.data_mut(|d| d.insert_temp(more_id, !more));
            }
        }

        widgets::section_title(ui, tl!("Other options"));
        for other in EDIT_OTHER_TOOLS.iter().filter_map(|id| catalog::group(id)) {
            if edit_row(ui, t, other.icon, tl!(other.label), other.availability).clicked() {
                tool = Some(other.id);
            }
        }
    });
    if let Some(id) = tool {
        app.select_tool(id);
    }
    if let Some(cmd) = run {
        app.run_command(cmd);
    }
}

/// A tool entry with further commands (Add / Update / Remove): a caret at `caret` says it opens a
/// menu, and each command is enabled by its own needs. Returns the command chosen.
pub(super) fn actions_menu(
    app: &PdfCraftApp,
    ui: &egui::Ui,
    resp: &egui::Response,
    item: &ToolItem,
    caret: egui::Pos2,
    color: Color32,
) -> Option<&'static str> {
    ui.painter().add(egui::Shape::convex_polygon(
        vec![caret + vec2(-3.5, -2.0), caret + vec2(3.5, -2.0), caret + vec2(0.0, 2.5)],
        color,
        egui::Stroke::NONE,
    ));
    let mut run = None;
    egui::Popup::menu(resp).show(|ui| {
        ui.set_min_width(180.0);
        for (n, id) in item.actions.iter().enumerate() {
            let Some(spec) = pdfcraft_engine::commands::command(id) else { continue };
            // Add stays usable with no document open: it asks for a file first.
            let enabled = (n == 0 && app.active.is_none()) || app.command_enabled(spec);
            let label = crate::i18n::menu_label(spec.id, spec.label);
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                run = Some(spec.id);
                ui.close();
            }
        }
    });
    run
}

/// One row of the Edit panel: a monochrome icon and the label, as Acrobat draws them. A planned
/// item is muted and says when it ships when hovered.
fn edit_row(ui: &mut egui::Ui, t: &Tokens, icon: &str, label: &str, availability: Availability) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let press = theme::Press::track(ui, &resp);
    press.wash(ui, rect, 6, false);
    let body = rect.translate(press.offset());
    let ready = availability == Availability::Ready;
    let fg = if ready { t.text } else { t.text_muted };
    icons::paint(ui, Rect::from_min_size(body.min + vec2(6.0, 8.0), vec2(18.0, 18.0)), icon, 17.0, if ready { t.text } else { t.text_faint });
    row_label(ui, label, theme::regular(13.5), fg, body.left_center() + vec2(34.0, 0.0), rect.width() - 42.0);
    let resp = theme::hand(resp);
    match availability {
        Availability::Ready => resp,
        Availability::Planned(m) => resp.on_hover_text(format!("{} {m}", tl!("Planned for milestone"))),
        Availability::Provider => resp.on_hover_text(tl!("Optional: needs an AI provider you configure")),
    }
}

/// Add a stamp: Dynamic, Sign Here and Standard Business stamps; click one, then click on the
/// page to place it.
pub(super) fn stamp_palette(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    use pdfcraft_engine::{StampGroup, StampKind};
    ui.label(egui::RichText::new(tl!("Choose a stamp, then click on the page to place it.")).small().color(t.text_faint));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (group, title) in [
            (StampGroup::Dynamic, tl!("Dynamic")),
            (StampGroup::SignHere, tl!("Sign Here")),
            (StampGroup::StandardBusiness, tl!("Standard Business")),
        ] {
            widgets::section_title(ui, title);
            for kind in StampKind::ALL.into_iter().filter(|k| k.group() == group) {
                let active = app.quick_tool == crate::QuickTool::Stamp(kind);
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, active, kind.label()));
                let press = theme::Press::track(ui, &resp);
                press.wash(ui, rect, 6, active);
                let body = rect.translate(press.offset());
                let [r, g, b] = kind.color().map(|v| (v * 255.0) as u8);
                let col = Color32::from_rgb(r, g, b);
                let chip = Rect::from_min_size(body.min + vec2(8.0, 5.0), vec2((rect.width() - 16.0).min(200.0), 28.0));
                if group == StampGroup::SignHere {
                    let tip = chip.height() * 0.45;
                    let pts = vec![
                        chip.left_center(),
                        chip.left_top() + vec2(tip, 0.0),
                        chip.right_top(),
                        chip.right_bottom(),
                        chip.left_bottom() + vec2(tip, 0.0),
                    ];
                    ui.painter().add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
                    let label = kind.label();
                    let font = theme::semibold(11.0);
                    let shown = crate::widgets::fit_line(label, (chip.width() - 10.0).max(0.0), |s| crate::widgets::text_width(ui, s, &font));
                    ui.painter().with_clip_rect(chip.shrink(3.0)).text(
                        chip.center() + vec2(tip / 2.0, 0.0),
                        Align2::CENTER_CENTER,
                        shown,
                        font,
                        Color32::WHITE,
                    );
                } else {
                    ui.painter().rect(chip, CornerRadius::same(5), col.gamma_multiply(0.1), Stroke::new(1.5, col), egui::StrokeKind::Inside);
                    let y = if group == StampGroup::Dynamic { chip.center().y - 4.0 } else { chip.center().y };
                    let font = theme::semibold(11.0);
                    let shown = crate::widgets::fit_line(kind.label(), (chip.width() - 10.0).max(0.0), |s| crate::widgets::text_width(ui, s, &font));
                    ui.painter().with_clip_rect(chip.shrink(3.0)).text(egui::pos2(chip.center().x, y), Align2::CENTER_CENTER, shown, font, col);
                    if group == StampGroup::Dynamic {
                        let by = "By name at time, date";
                        let by_font = theme::regular(7.5);
                        let by_shown = crate::widgets::fit_line(by, (chip.width() - 8.0).max(0.0), |s| crate::widgets::text_width(ui, s, &by_font));
                        ui.painter().with_clip_rect(chip.shrink(2.0)).text(
                            egui::pos2(chip.center().x, chip.bottom() - 6.0),
                            Align2::CENTER_CENTER,
                            by_shown,
                            by_font,
                            col,
                        );
                    }
                }
                if theme::hand(resp).clicked() {
                    app.quick_tool = crate::QuickTool::Stamp(kind);
                }
            }
        }
        crate::stamps_ui::palette_section(app, ui, t);
    });
}

/// Edit a PDF ▸ Format text: for the selected added text (one undoable change), or the style
/// new text gets.
fn format_section(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // Editing a paragraph of existing text: its formatting, applied as it changes.
    if let Some((i, _)) = app.active_ids()
        && let Some(ed) = app.views[i].line_editor.clone()
    {
        use crate::format_text_ui::{FontChoice, Fonts};
        let mut ed = ed;
        let sources: Vec<(String, String)> = app
            .session
            .get(app.views[i].id)
            .map(|d| d.text_editor_fonts(ed.page))
            .unwrap_or_default()
            .into_iter()
            .map(|f| (f.resource, f.face))
            .collect();
        let shown = ed.font_shown(&sources);
        // A selection aligns the rows it touches; the caret alone, the whole paragraph.
        let paragraphs = crate::format_text_ui::Paragraphs { text: &ed.text, selection: ed.selection.clone(), tidy: false };
        let out = crate::format_text_ui::panel(ui, t, &mut ed.look, Some(&mut ed.extras), Fonts { sources: Some(&sources), shown }, Some(paragraphs));
        let mut changed = out.changed;
        match out.font {
            Some(FontChoice::KeepSource) => {
                ed.keep_source_font();
                changed = true;
            }
            Some(FontChoice::Resource(resource)) => {
                ed.font_resource = Some(resource);
                changed = true;
            }
            Some(FontChoice::Family(family)) => {
                ed.choose_family(family);
                changed = true;
            }
            Some(FontChoice::Load) => app.pick_files(crate::files::FilePurpose::TextFont { page: ed.page }, false),
            #[cfg(not(target_arch = "wasm32"))]
            Some(FontChoice::Installed(path)) => app.use_paths(crate::files::FilePurpose::TextFont { page: ed.page }, &[path]),
            #[cfg(target_arch = "wasm32")]
            Some(FontChoice::Installed(_)) => {}
            None => {}
        }
        if changed {
            app.apply_line_format(i, ed, Some(ui.ctx()));
        }
        return;
    }
    // Typing added text: the box's formatting, and alignment for the lines a selection touches.
    if let Some(i) = app.active_ids().map(|(i, _)| i)
        && let Some(draft) = app.views[i].content.draft.clone()
    {
        if draft.index.is_none() && app.quick_tool == crate::QuickTool::AddText {
            crate::content_ui::hint(ui, t);
        }
        let mut style = draft.style.clone();
        let paragraphs = crate::format_text_ui::Paragraphs { text: &draft.text, selection: draft.selection.clone(), tidy: true };
        added_text_format(ui, t, &mut style, Some(paragraphs));
        if style != draft.style {
            app.restyle_draft(i, style);
        }
        return;
    }
    let image = app.active_ids().and_then(|(i, id)| {
        let (page, index) = app.views[i].content.selected?;
        let doc = app.session.get(id)?;
        match &doc.added().iter().filter(|a| a.page == page).nth(index)?.content {
            pdfcraft_engine::AddedContent::Image(img) => Some((page, index, img.clone())),
            _ => None,
        }
    });
    if let Some((page, index, img)) = image {
        match crate::content_ui::image_panel(ui, t, &img) {
            Some(crate::content_ui::ImageAction::Update(content)) => {
                app.apply_edit(pdfcraft_engine::Edit::UpdateContent { page, index, content });
            }
            Some(crate::content_ui::ImageAction::Replace) => app.replace_image_dialog(page, index),
            None => {}
        }
        ui.add_space(6.0);
        ui.separator();
        return;
    }
    let selected = app.active_ids().and_then(|(i, id)| {
        let (page, index) = app.views[i].content.selected?;
        let doc = app.session.get(id)?;
        let a = doc.added().iter().filter(|a| a.page == page).nth(index)?;
        match &a.content {
            pdfcraft_engine::AddedContent::Text(text) => Some((page, index, text.clone())),
            _ => None,
        }
    });
    match selected {
        Some((page, index, text)) => {
            let mut style = text.clone();
            added_text_format(ui, t, &mut style, None);
            if style != text {
                // The reusable style follows the selection only when the edit took, so a refused one adds nothing odd later.
                if app.apply_edit(pdfcraft_engine::Edit::UpdateContent { page, index, content: pdfcraft_engine::AddedContent::Text(style.clone()) }) {
                    app.text_style = pdfcraft_engine::AddedText { text: String::new(), rect: [0.0; 4], aligns: Vec::new(), ..style };
                }
            }
        }
        None if app.quick_tool == crate::QuickTool::AddText => {
            crate::content_ui::hint(ui, t);
            let mut style = app.text_style.clone();
            added_text_format(ui, t, &mut style, None);
            app.text_style = style;
        }
        None => {}
    }
}

/// Format text for added text: the standard families, size, colour, style and alignment
/// (of the lines `paragraphs` selects, while the text is being typed).
fn added_text_format(ui: &mut egui::Ui, t: &Tokens, style: &mut pdfcraft_engine::AddedText, paragraphs: Option<crate::format_text_ui::Paragraphs>) {
    let shown = style.family.label().to_string();
    let out = crate::format_text_ui::panel(ui, t, style, None, crate::format_text_ui::Fonts { sources: None, shown }, paragraphs);
    if let Some(crate::format_text_ui::FontChoice::Family(family)) = out.font {
        style.family = family;
    }
}

impl PdfCraftApp {
    /// Format text changed while added text is being typed in view `index`: the box takes the new
    /// style at once and stays open. Text already on the page is updated with what has been typed,
    /// as one undoable change; new text waits until it is finished. Text typed after this styling
    /// starts from it too.
    pub(crate) fn restyle_draft(&mut self, index: usize, style: pdfcraft_engine::AddedText) {
        let Some(view) = self.views.get_mut(index) else { return };
        // Text the standard fonts can't draw stays in the box, unwritten, until it is corrected (#125).
        let blocked = view.content.blocked().is_some();
        let Some(d) = view.content.draft.as_mut() else { return };
        let committed = d.style.clone();
        // `text` and `rect` stay what the page has; the rest is the new style.
        d.style = pdfcraft_engine::AddedText { text: committed.text.clone(), rect: committed.rect, ..style };
        d.focus = true;
        self.text_style = pdfcraft_engine::AddedText { text: String::new(), rect: [0.0; 4], aligns: Vec::new(), ..d.style.clone() };
        let (page, item) = (d.page, d.index);
        let text = d.text.trim_end().to_string();
        let Some(item) = item.filter(|_| !text.trim().is_empty() && !blocked) else { return };
        let aligns = pdfcraft_engine::paragraph_align::remap(&d.text, &text, &d.style.aligns);
        let updated = pdfcraft_engine::AddedText { text, rect: d.rect, aligns, ..d.style.clone() };
        if updated == committed {
            return;
        }
        if self.apply_edit(pdfcraft_engine::Edit::UpdateContent { page, index: item, content: pdfcraft_engine::AddedContent::Text(updated.clone()) })
            && let Some(d) = self.views.get_mut(index).and_then(|v| v.content.draft.as_mut())
        {
            // What the page has now. The aligns stay those of the text in the box.
            d.style = pdfcraft_engine::AddedText { aligns: std::mem::take(&mut d.style.aligns), ..updated };
            // The page has the box's text: its typing history restarts at the next claim, even
            // when the restyle left the text as it was, so a Redo can't put undone typing back.
            d.typing_from = None;
        }
    }

    /// Apply the open paragraph's formatting as one change; the paragraph stays open.
    pub(crate) fn apply_line_format(&mut self, index: usize, mut ed: crate::edit_text_ui::LineEditor, ctx: Option<&egui::Context>) {
        let edit = pdfcraft_engine::Edit::EditTextBlock { page: ed.page, block: ed.block, text: ed.text.clone(), style: ed.style() };
        if self.apply_edit(edit) {
            ed.applied();
            if let Some(doc) = self.session.get(self.views[index].id)
                && let Some(block) = doc.text_blocks(ed.page).get(ed.block)
            {
                ed.refresh_source(block, doc.text_lines(ed.page));
                if let Some(ctx) = ctx {
                    ed.fonts = crate::edit_text_ui::install_source_fonts(ctx, doc, ed.page);
                }
            }
        }
        self.views[index].line_editor = Some(ed);
    }
}
