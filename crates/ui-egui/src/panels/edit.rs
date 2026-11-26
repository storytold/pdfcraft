use craft::behavior::{IconId, Selection};
use craft::ui::buttons::IconButton;
use craft::ui::contract::{
    Bind, Entry, Outcome as _, Props as _, Scroll, SelectMode, Show as _, ShowCollection as _, ShowContainer as _, Trigger, clicked,
};
use craft::ui::display::{Shade, Text};
use craft::ui::layout_components::{Row, Stack};
use craft::ui::list::{ListRow, RowInk, ThumbAspect};
use craft::ui::menu::Menu;
use craft::ui::primitives::IconSpec;
use craft::ui::runtime::env::Label;
use craft::ui::sections::{HeaderAction, HeaderCommand, Panel, Section};
use craft::ui::tile_grid::{CaptionPlace, Tile, TileGrid, TileGroup};

use super::{Availability, PdfCraftApp, ToolGroup, ToolItem, catalog};

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
pub(super) fn edit_panel(app: &mut PdfCraftApp, ui: &mut egui::Ui, g: &'static ToolGroup) {
    let items = || g.sections.iter().flat_map(|s| s.items.iter());
    let item = |command: &str| items().find(|i| i.command == command);
    let mut run = None;
    let mut tool = None;
    // Back to All tools only when Edit was opened from there; the Edit tab is its own way back.
    let from_all_tools = app.mode != crate::Mode::Edit;
    // "Edit" under the Edit tab, as Acrobat titles it; the tool's own name from All tools.
    let title = if from_all_tools { tl!(g.label) } else { tl!("Edit") }.to_string();
    let mut commands = Vec::new();
    if from_all_tools {
        commands.push(HeaderCommand::new(IconId::new("chevron-left"), "tools.all"));
    }
    commands.push(HeaderCommand::new(IconId::new("settings"), "app.preferences"));
    let shown = Panel::new(title).titled().commands(commands).actions([HeaderAction::Close]).show(ui, |ui, _| {
        // Several objects selected: how many, and how to move them. Acrobat's panel has no idle hint.
        let count =
            app.active_ids().filter(|_| app.quick_tool == crate::QuickTool::EditText).map_or(0, |(i, _)| app.views[i].session.objects.count());
        if count > 0 {
            Text::new(crate::i18n::trn(crate::i18n::current(), count as u64, "{n} object selected", "{n} objects selected")).show(ui);
            Text::new(tl!("Drag the selection to move it. Esc clears the selection.").to_string()).show(ui);
        }
        // Format text at the top while text is selected or being added.
        format_section(app, ui);

        Section::fixed(tl!("Modify page").to_string()).show(ui, |ui, _| {
            Row::new().show(ui, |ui, _| {
                for i in EDIT_PAGE_BUTTONS.iter().filter_map(|c| item(c)) {
                    // Enabled as the command itself is: a document that forbids page assembly refuses these.
                    IconButton::command(IconSpec::svg(IconId::new(i.icon)), i.command).show(ui);
                }
            });
            // Organize pages is a workspace of its own: its panel replaces this one, with the grid.
            if let Some(i) = item("page.organize")
                && clicked(&tool_row(ui, i.icon, tl!(i.label), i.availability, false))
            {
                // With no document there is no grid to open: the command asks for a file first.
                if app.active.is_some() {
                    tool = Some("organize");
                } else {
                    run = Some("page.organize");
                }
            }
        });

        Section::fixed(tl!("Add content").to_string()).show(ui, |ui, _| {
            let content: Vec<_> = g
                .sections
                .iter()
                .filter(|s| s.title == "Add content")
                .flat_map(|s| s.items.iter())
                .filter(|i| i.command != "edit.edit_text")
                .collect();
            let shown = if app.edit_more { content.len() } else { EDIT_CONTENT_SHOWN.min(content.len()) };
            for i in content.iter().take(shown) {
                let resp = tool_row(ui, i.icon, tl!(i.label), i.availability, !i.actions.is_empty());
                // Header and footer, Watermark and Background open their Add / Update / Remove menu.
                if i.actions.is_empty() {
                    if clicked(&resp) {
                        run = Some(i.command);
                    }
                } else if let Some(cmd) = actions_menu(app, ui, &resp, i) {
                    run = Some(cmd);
                }
            }
            if content.len() > EDIT_CONTENT_SHOWN {
                let label = if app.edit_more { tl!("Less") } else { tl!("More") };
                if clicked(craft::ui::buttons::Link::new(label.to_string()).show(ui).response()) {
                    app.edit_more = !app.edit_more;
                }
            }
        });

        Section::fixed(tl!("Other options").to_string()).show(ui, |ui, _| {
            for other in EDIT_OTHER_TOOLS.iter().filter_map(|id| catalog::group(id)) {
                if clicked(&tool_row(ui, other.icon, tl!(other.label), other.availability, false)) {
                    tool = Some(other.id);
                }
            }
        });
    });
    if shown.output.closed.is_some() || shown.output.action == Some(HeaderAction::Close) {
        app.set_left_open(false);
    }
    if let Some(id) = tool {
        app.select_tool(ui.ctx(), id);
    }
    if let Some(cmd) = run {
        app.run_command(&ui.ctx().clone(), cmd);
    }
}

/// A tool entry with further commands (Add / Update / Remove): pressing the row `opener` opens a
/// menu (a click or the activation key), and each command is enabled by its own needs. Returns the command chosen.
pub(super) fn actions_menu(app: &PdfCraftApp, ui: &mut egui::Ui, opener: &egui::Response, item: &ToolItem) -> Option<&'static str> {
    let entries: Vec<Entry<usize>> = item
        .actions
        .iter()
        .enumerate()
        .filter_map(|(n, id)| {
            let spec = pdfcraft_engine::commands::command(id)?;
            // Add stays usable with no document open: it asks for a file first.
            let enabled = (n == 0 && app.active.is_none()) || app.command_enabled(spec);
            Some(Entry::new(n, Label::from(crate::i18n::menu_label(spec.id, spec.label))).enabled(enabled))
        })
        .collect();
    let picked = Menu::new(tl!(item.label).to_string()).opened_by(opener, Trigger::Click).pick(ui, &entries).picked?;
    item.actions.get(picked).and_then(|id| pdfcraft_engine::commands::command(id)).map(|spec| spec.id)
}

/// One row of the Edit panel: a monochrome icon and the label, as Acrobat draws them. A planned
/// item is dimmed and says when it ships when hovered; `menu` adds the row's More button.
fn tool_row(ui: &mut egui::Ui, icon: &'static str, label: &str, availability: Availability, menu: bool) -> egui::Response {
    let mut row = ListRow::new(label.to_string()).icon(IconSpec::svg(IconId::new(icon)));
    row = match availability {
        Availability::Ready => row,
        Availability::Planned(m) => row.ink(RowInk::Dimmed).tooltip(format!("{} {m}", tl!("Planned for milestone"))),
        Availability::Provider => row.ink(RowInk::Dimmed).tooltip(tl!("Optional: needs an AI provider you configure").to_string()),
    };
    if menu {
        // The disclosure arrow production shows at the end of Header and footer and Watermark; the whole row
        // opens the menu.
        row = row.trailing_named("\u{25BE}".to_owned(), tl!("Options").to_owned());
    }
    row.show(ui).response
}

/// The stamps a [`TileGrid`] lists: one tile per kind, in the order of their groups.
struct Stamps(usize);

impl craft::behavior::DisplayOrder<usize> for Stamps {
    fn len(&self) -> usize {
        self.0
    }

    fn key(&self, at: usize) -> usize {
        at
    }

    fn index_of(&self, key: &usize) -> Option<usize> {
        (*key < self.0).then_some(*key)
    }
}

impl craft::behavior::ItemSource<usize> for Stamps {
    /// Every stamp is a wide strip, as production draws them: six parts wide to one high.
    fn size_hint(&self, _at: usize) -> craft::behavior::SizeHint {
        craft::behavior::SizeHint::Aspect(6.0)
    }
}

/// Add a stamp: Dynamic, Sign Here and Standard Business stamps; click one, then click on the
/// page to place it.
pub(super) fn stamp_palette(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    use pdfcraft_engine::{StampGroup, StampKind};
    // The dock's group header already names the tool and closes it; a second title here showed "Add a stamp" twice.
    let shown = Panel::new(tl!("Add a stamp").to_string()).show(ui, |ui, _| {
        Stack::new().scroll(Scroll::vertical()).show(ui, |ui, _| {
            Text::new(tl!("Choose a stamp, then click on the page to place it.").to_string()).shade(Shade::Faint).show(ui);
            let titles = [
                (StampGroup::Dynamic, tl!("Dynamic")),
                (StampGroup::SignHere, tl!("Sign Here")),
                (StampGroup::StandardBusiness, tl!("Standard Business")),
            ];
            // The kinds in group order, and the headers' keys after every tile's.
            let mut kinds: Vec<(StampGroup, StampKind)> = Vec::new();
            let mut groups = Vec::new();
            for (n, (group, title)) in titles.iter().enumerate() {
                let start = kinds.len();
                kinds.extend(StampKind::ALL.into_iter().filter(|k| k.group() == *group).map(|k| (*group, k)));
                groups.push(TileGroup::new(1_000_000 + n, start, kinds.len() - start, title.to_uppercase()));
            }
            let mut selection = match app.quick_tool {
                crate::QuickTool::Stamp(current) => kinds.iter().position(|(_, kind)| *kind == current).map(Selection::single).unwrap_or_default(),
                _ => Selection::default(),
            };
            let before = selection.clone();
            let _ = TileGrid::<usize>::new(tl!("Stamps").to_string())
                .natural_height(true)
                .justified()
                // Pictures this high keep every strip, the last of a group included, at one size across the panel.
                .zoom(craft::ui::tile_grid::GridZoom::Fixed(0.22))
                .caption(CaptionPlace::None)
                .select(SelectMode::Single)
                .selection(Bind::bound(&mut selection))
                .groups(&groups)
                .show_items(ui, &Stamps(kinds.len()), &mut |key, _| {
                    let Some((group, kind)) = kinds.get(*key).copied() else { return Tile::new(String::new()) };
                    let [r, g, b] = kind.color().map(|v| (v * 255.0) as u8);
                    let color = egui::Color32::from_rgb(r, g, b);
                    let (label, by) = (kind.label().to_string(), tl!("By name at time, date").to_string());
                    Tile::new(label.clone()).thumb(ThumbAspect::new(6, 1), move |cx, rect| paint_chip(cx, rect, group, &label, &by, color))
                });
            if selection != before
                && let Some(&at) = selection.items.first()
                && let Some((_, kind)) = kinds.get(at)
            {
                app.quick_tool = crate::QuickTool::Stamp(*kind);
            }
            crate::stamps_ui::palette_section(app, ui);
        });
    });
    if shown.output.closed.is_some() || shown.output.action == Some(HeaderAction::Close) {
        app.set_left_open(false);
    }
}

/// A stamp's chip: its colour as a tinted frame (Sign Here as a pentagon pointing left, with the label
/// in white), the label, and the Dynamic stamps' second line.
fn paint_chip(
    cx: &craft::ui::primitives::ContentPainterCx<'_>,
    chip: egui::Rect,
    group: pdfcraft_engine::StampGroup,
    label: &str,
    by: &str,
    col: egui::Color32,
) {
    use craft::ui::primitives::{PaintShape, StrokeWidth};
    use craft::ui::style::DataColor;
    use egui::vec2;
    use pdfcraft_engine::StampGroup;
    // Sizes follow the chip's height, so the chip scales with its tile.
    let height = chip.height();
    let line = cx.stroke_width(StrokeWidth::Thin);
    let chip = chip.shrink2(vec2(0.0, line));
    if group == StampGroup::SignHere {
        let tip = height * 0.45;
        let pts =
            vec![chip.left_center(), chip.left_top() + vec2(tip, 0.0), chip.right_top(), chip.right_bottom(), chip.left_bottom() + vec2(tip, 0.0)];
        cx.with_draw(|draw| draw.data_fill(&PaintShape::Polygon(pts), DataColor::new(col)));
        centred_text(cx, chip.center() + vec2(tip / 2.0, 0.0), label, height * 0.28, egui::Color32::WHITE);
    } else {
        let shape = PaintShape::Rect(chip, egui::CornerRadius::same((height * 0.12).round().clamp(0.0, 255.0) as u8));
        cx.with_draw(|draw| {
            draw.data_fill(&shape, DataColor::new(col.gamma_multiply(0.1)));
            draw.stroke(&shape, StrokeWidth::Thin, DataColor::new(col));
        });
        let y = if group == StampGroup::Dynamic { chip.center().y - height * 0.1 } else { chip.center().y };
        centred_text(cx, egui::pos2(chip.center().x, y), label, height * 0.28, col);
        if group == StampGroup::Dynamic {
            centred_text(cx, egui::pos2(chip.center().x, chip.bottom() - height * 0.15), by, height * 0.19, col);
        }
    }
}

/// `text` in the proportional face, centred on `at`, in a colour from the stamp's data.
fn centred_text(cx: &craft::ui::primitives::ContentPainterCx<'_>, at: egui::Pos2, text: &str, size: f32, color: egui::Color32) {
    use craft::ui::fonts::FontRef;
    use craft::ui::primitives::{DocRun, DocText};
    use craft::ui::style::DataColor;
    let runs = [DocRun::new(text, FontRef::proportional(), size, DataColor::new(color))];
    let layout = cx.layout(&DocText::new(&runs));
    let pos = at - layout.galley.size() / 2.0;
    cx.with_draw(|draw| draw.galley(pos, &layout));
}

/// Edit a PDF ▸ Format text: for the selected added text (one undoable change), or the style
/// new text gets.
fn format_section(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    use crate::format_text_ui::{Target, panel};
    // Editing a paragraph of existing text: its formatting, applied as it changes.
    if let Some((i, _)) = app.active_ids()
        && let Some(ed) = app.views[i].session.line_editor.clone()
    {
        let mut ed = ed;
        let sources: Vec<(String, String)> = app
            .session
            .get(app.views[i].id)
            .map(|d| d.text_editor_fonts(ed.page))
            .unwrap_or_default()
            .into_iter()
            .map(|f| (f.resource, f.face))
            .collect();
        let fmt = ed.selection_formatting(&sources);
        ed.sync_selection_look(&fmt);
        panel(app, ui, i, Target::Paragraph(Box::new(ed)), Some(&fmt), &sources);
        return;
    }
    // Typing added text: the box's formatting, and alignment for the lines a selection touches.
    if let Some(i) = app.active_ids().map(|(i, _)| i)
        && let Some(draft) = app.views[i].session.content.draft.clone()
    {
        if draft.index.is_none() && app.quick_tool == crate::QuickTool::AddText {
            crate::content_ui::hint(ui);
        }
        panel(app, ui, i, Target::Draft(Box::new(draft)), None, &[]);
        return;
    }
    let image = app.active_ids().and_then(|(i, id)| {
        let (page, index) = app.views[i].session.content.selected?;
        let doc = app.session.get(id)?;
        match &doc.added().iter().filter(|a| a.page == page).nth(index)?.content {
            pdfcraft_engine::AddedContent::Image(img) => Some((page, index, img.clone())),
            _ => None,
        }
    });
    if let Some((page, index, img)) = image {
        match crate::content_ui::image_panel(ui, &img) {
            Some(crate::content_ui::ImageAction::Update(content)) => {
                app.apply_edit(ui.ctx(), pdfcraft_engine::Edit::UpdateContent { page, index, content });
            }
            Some(crate::content_ui::ImageAction::Replace) => app.replace_image_dialog(page, index),
            None => {}
        }
        return;
    }
    let selected = app.active_ids().and_then(|(i, id)| {
        let (page, index) = app.views[i].session.content.selected?;
        let doc = app.session.get(id)?;
        let a = doc.added().iter().filter(|a| a.page == page).nth(index)?;
        match &a.content {
            pdfcraft_engine::AddedContent::Text(text) => Some((i, page, index, text.clone())),
            _ => None,
        }
    });
    match selected {
        Some((i, page, index, text)) => panel(app, ui, i, Target::Added { page, index, text }, None, &[]),
        None if app.quick_tool == crate::QuickTool::AddText => {
            crate::content_ui::hint(ui);
            let armed = app.text_style.clone();
            panel(app, ui, 0, Target::Armed(armed), None, &[]);
        }
        None => {}
    }
}

impl PdfCraftApp {
    /// Format text changed while added text is being typed in view `index`: the box takes the new
    /// style at once and stays open. Text already on the page is updated with what has been typed,
    /// as one undoable change; new text waits until it is finished. Text typed after this styling
    /// starts from it too.
    pub(crate) fn restyle_draft(&mut self, ctx: &egui::Context, index: usize, mut style: pdfcraft_engine::AddedText) {
        // A gray or CMYK ink the colour no longer shows goes now (#6440): kept, the draft would
        // differ from the item the page reads back without it, and a later pick of the ink's
        // look would bring it back.
        style.drop_stale_ink();
        let Some(view) = self.views.get_mut(index) else { return };
        // Text the standard fonts can't draw stays in the box, unwritten, until it is corrected (#125).
        let blocked = view.session.content.blocked().is_some();
        let Some(d) = view.session.content.draft.as_mut() else { return };
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
        if self.apply_edit(
            ctx,
            pdfcraft_engine::Edit::UpdateContent { page, index: item, content: pdfcraft_engine::AddedContent::Text(updated.clone()) },
        ) && let Some(d) = self.views.get_mut(index).and_then(|v| v.session.content.draft.as_mut())
        {
            // What the page has now. The aligns stay those of the text in the box.
            d.style = pdfcraft_engine::AddedText { aligns: std::mem::take(&mut d.style.aligns), ..updated };
            // The page has the box's text: its typing history restarts from it, even when the
            // restyle left the text as it was, so a Redo can't put undone typing back.
            let now = crate::edit_text_ui::Typed::of(d.text.clone(), d.selection, d.style.aligns.clone());
            d.history.restart(now);
        }
    }

    /// Apply the open paragraph's formatting as one change; the paragraph stays open.
    pub(crate) fn apply_line_format(&mut self, ctx: &egui::Context, index: usize, mut ed: crate::edit_text_ui::LineEditor) {
        let edit = pdfcraft_engine::Edit::EditTextBlock { page: ed.page, block: ed.block, text: ed.text.clone(), style: ed.style() };
        if self.apply_edit(ctx, edit) {
            ed.applied();
            if let Some(doc) = self.session.get(self.views[index].id)
                && let Some(block) = doc.text_blocks(ed.page).get(ed.block)
            {
                ed.refresh_source(block, doc.text_lines(ed.page));
                // The page has the paragraph's text: its typing history restarts from it.
                let now = crate::edit_text_ui::Typed::of(ed.text.clone(), ed.selection, ed.look.aligns.clone());
                ed.history.restart(now);
            }
        }
        self.views[index].session.line_editor = Some(ed);
    }
}
