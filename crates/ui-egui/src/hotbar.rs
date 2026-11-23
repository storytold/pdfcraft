//! The toolbar: a tool rail in the dock, with a searchable drawer that edits which commands sit on it.
use craft::behavior::{DockSide, IconId};
use craft::keymap::CommandRef;
use craft::theme::Srgba;
use craft::ui::buttons::IconButton;
use craft::ui::choice::Swatch;
use craft::ui::contract::{Bind, Entry, Props as _, Show};
use craft::ui::docking::PanelSpec;
use craft::ui::form::SliderField;
use craft::ui::pickers::Select;
use craft::ui::rails::{DrawerGroup, DrawerKind, LastUsed, RailTool, SlotId, ToolRail, ToolSlot, ToolbarDrawer, apply_toolbar_edit};
use craft::ui::runtime::env::UnitKind;
use craft::ui::value_drag::SliderScale;
use pdfcraft_engine::commands::{COMMANDS, command};
use pdfcraft_engine::{Edit, Rgb};
use serde::{Deserialize, Serialize};

use crate::fill_sign::{FILL_TOOLS, FillTool};
use crate::{Mode, PdfCraftApp, QuickTool, comments};

/// The tool ids of the tools that are not registry commands; the rail's current tool and its slots are keyed by
/// command id.
const TOOL_SELECT: &str = "view.select_tool";
const TOOL_HAND: &str = "view.hand_tool";
const TOOL_CROP: &str = "page.crop";
const TOOL_EDIT_TEXT: &str = "edit.edit_text";

const SLOT_SELECT: SlotId = 0;
const SLOT_HAND: SlotId = 1;
const SLOT_CROP: SlotId = 2;
/// The first comment group's slot; group `g` is `SLOT_COMMENT + g`.
const SLOT_COMMENT: SlotId = 10;
const SLOT_EDIT_TEXT: SlotId = 20;
const SLOT_FILL: SlotId = 30;
/// The first user slot; the user's `i`-th command is `SLOT_USER + i`.
const SLOT_USER: SlotId = 100;

/// Persistent state and UI state for the toolbar.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Hotbar {
    /// The commands the user put on the toolbar, in order.
    pub tools: Vec<String>,
    /// The tool each slot showed last.
    #[serde(skip)]
    last_used: LastUsed<String>,
    /// Whether the drawer that edits the toolbar is open.
    #[serde(skip)]
    drawer_open: bool,
}

impl Hotbar {
    /// Drops tool ids the command registry no longer has, and duplicates.
    pub fn normalize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.tools.retain(|id| command(id).is_some() && seen.insert(id.clone()));
    }

    /// Parse and validate a comma-separated list of command identifiers for the hotbar.
    pub fn set_tools(&mut self, value: &str) -> Result<(), String> {
        let tools: Vec<String> = value.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
        if let Some(id) = tools.iter().find(|id| command(id).is_none()) {
            return Err(format!("Unknown hotbar command: {id}"));
        }
        self.tools = tools;
        self.normalize();
        Ok(())
    }
}

/// Reads a `hotbar` object saved before the toolbar became a dock panel: its tool list (filtered as `normalize`
/// does) and the side it was docked to. Position, size, full-side and placement are dropped. The old physical
/// `"left"` maps to `Start`, as `DockSide`'s own aliases do, so a right-to-left window docks it on the right.
pub(crate) fn migrate_legacy(value: &serde_json::Value) -> (Vec<String>, Option<DockSide>) {
    let mut bar = Hotbar {
        tools: value
            .get("tools")
            .and_then(|tools| tools.as_array())
            .map(|tools| tools.iter().filter_map(|id| id.as_str().map(str::to_owned)).collect())
            .unwrap_or_default(),
        ..Hotbar::default()
    };
    bar.normalize();
    let side = match value.get("dock").and_then(|dock| dock.as_str()) {
        Some("left") => Some(DockSide::Start),
        Some("right") => Some(DockSide::End),
        Some("top") => Some(DockSide::Top),
        Some("bottom") => Some(DockSide::Bottom),
        _ => None,
    };
    (bar.tools, side)
}

fn rail_tool(id: &str) -> RailTool<String> {
    RailTool::new(id.to_owned(), CommandRef::new(id))
}

/// The id of the rail tool the app's current tool stands for; empty when the rail has none for it.
fn current_tool(app: &PdfCraftApp) -> String {
    match app.quick_tool {
        QuickTool::Select => TOOL_SELECT.to_owned(),
        QuickTool::Hand => TOOL_HAND.to_owned(),
        QuickTool::Crop => TOOL_CROP.to_owned(),
        QuickTool::EditText => TOOL_EDIT_TEXT.to_owned(),
        QuickTool::Comment(tool) => tool.command().to_owned(),
        QuickTool::Fill(tool) => tool.command().to_owned(),
        _ => String::new(),
    }
}

/// The tool each slot shows while its tools aren't current. The comment groups and Fill & Sign show the app's own last
/// tool of theirs, which every pick sets (a command, a menu, the control channel), with the toolbar shown or not; the
/// other slots show the rail's own memory.
fn last_used(app: &PdfCraftApp) -> LastUsed<String> {
    let mut last = app.hotbar.last_used.clone();
    for (g, tool) in app.comment_prefs.group_tool.iter().enumerate() {
        last.set(SLOT_COMMENT + g as SlotId, tool.command().to_owned());
    }
    last.set(SLOT_FILL, app.fill_tool.command().to_owned());
    last
}

/// The built-in slots, then one slot for each of `user` commands.
fn slots_with(app: &PdfCraftApp, user: &[String]) -> Vec<ToolSlot<String>> {
    let mut slots = vec![
        ToolSlot::new(SLOT_SELECT, vec![rail_tool(TOOL_SELECT)]),
        ToolSlot::new(SLOT_HAND, vec![rail_tool(TOOL_HAND)]),
        ToolSlot::ending_group(SLOT_CROP, vec![rail_tool(TOOL_CROP)]),
    ];
    let groups = comments::GROUPS.len();
    for (g, tools) in comments::GROUPS.iter().enumerate() {
        let tools = tools.iter().map(|tool| rail_tool(tool.command())).collect();
        let id = SLOT_COMMENT + g as SlotId;
        slots.push(if g + 1 == groups { ToolSlot::ending_group(id, tools) } else { ToolSlot::new(id, tools) });
    }
    if app.mode == Mode::Edit || app.quick_tool == QuickTool::EditText {
        slots.push(ToolSlot::new(SLOT_EDIT_TEXT, vec![rail_tool(TOOL_EDIT_TEXT)]));
    }
    slots.push(ToolSlot::ending_group(SLOT_FILL, FILL_TOOLS.iter().map(|tool| rail_tool(tool.command())).collect()));
    for (i, id) in user.iter().enumerate() {
        slots.push(ToolSlot::new(SLOT_USER + i as SlotId, vec![rail_tool(id)]));
    }
    slots
}

/// The toolbar's slots: the built-in tools and the user's commands.
pub(crate) fn slots(app: &PdfCraftApp) -> Vec<ToolSlot<String>> {
    slots_with(app, &app.hotbar.tools)
}

/// The registry's commands grouped by family (the id up to its first dot) for the drawer.
fn drawer_groups() -> Vec<DrawerGroup<String>> {
    let mut families: Vec<(&str, Vec<RailTool<String>>)> = Vec::new();
    for spec in COMMANDS {
        let family = spec.id.split('.').next().unwrap_or(spec.id);
        match families.iter_mut().find(|(name, _)| *name == family) {
            Some((_, tools)) => tools.push(rail_tool(spec.id)),
            None => families.push((family, vec![rail_tool(spec.id)])),
        }
    }
    families
        .into_iter()
        .map(|(family, tools)| {
            let mut name = family.to_owned();
            if let Some(first) = name.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            DrawerGroup::new(name, tools)
        })
        .collect()
}

/// The toolbar's dock panel: its column is whole tool columns wide, and its tab shows the marker of the command that
/// toggles panels.
pub(crate) fn panel_spec(app: &PdfCraftApp, ctx: &egui::Context) -> PanelSpec {
    // PLAN: assumed the toolbar's `PanelId` serialises as "toolbar" in `view.panel {panel}` (pdf-chrome-w13).
    PanelSpec::new(tl!("Toolbar").to_string())
        .grid(ToolRail::<String>::dock_grid(ctx, slots(app).len()))
        .command(CommandRef::with_params("view.panel", serde_json::json!({ "panel": "toolbar" })))
}

/// Measures the toolbar and active tool controls so the dock gives the rail its full height.
pub(crate) fn measure_rail(app: &mut PdfCraftApp, ui: &egui::Ui, width: f32) -> f32 {
    app.hotbar.normalize();
    let toolbar = slots(app);
    let current = current_tool(app);
    let last_used = last_used(app);
    let mut rail = ToolRail::new(tl!("Toolbar").to_string(), &toolbar, &current, &last_used).edit_toolbar(tl!("Edit toolbar").to_string()).edit_icon(IconId::new("settings"));
    if app.hotbar.drawer_open {
        rail = rail.editing();
    }
    rail.measure_rail(ui, width, |ui, _footer| footer_controls(ui, app)).y
}

// Every conversion between the engine's `f64` colour and the framework's goes through these two.
fn to_srgba(color: Rgb) -> Srgba {
    Srgba::rgb(color[0] as f32, color[1] as f32, color[2] as f32)
}

fn from_srgba(color: Srgba) -> Rgb {
    let [r, g, b] = color.to_rgb_array();
    [f64::from(r), f64::from(g), f64::from(b)]
}

/// Opens the colour picker in the main window, the window the rail's footer lives in, and runs `then` with the answer.
fn open_picker(
    app: &mut PdfCraftApp,
    request: craft::ui::runtime::requests::ColorRequest,
    then: impl FnOnce(&mut PdfCraftApp, &egui::Context, Srgba) + 'static,
) {
    app.open_colour_picker(crate::colour_picker::PickerHost::App, request, then);
}

/// Shows the toolbar's rail in the width `ui` has left, with the active tool's controls under it, and applies what
/// the user did: a drawer pick runs its command, a drawer edit changes the tool list.
pub(crate) fn show_rail(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    app.hotbar.normalize();
    let toolbar = slots(app);
    let current = current_tool(app);
    let last_used = last_used(app);
    let editing = app.hotbar.drawer_open;
    let mut rail = ToolRail::new(tl!("Toolbar").to_string(), &toolbar, &current, &last_used).edit_toolbar(tl!("Edit toolbar").to_string()).edit_icon(IconId::new("settings"));
    if editing {
        rail = rail.editing();
    }
    let output = rail.show_rail(ui, |ui, _footer| footer_controls(ui, app));
    if let Some(next) = output.last_used {
        app.hotbar.last_used = next;
    }
    // A click by the pointer or by keyboard activation.
    if output.edit_toolbar.as_ref().is_some_and(craft::ui::contract::clicked) {
        app.hotbar.drawer_open = true;
    }
    if !app.hotbar.drawer_open {
        return;
    }
    let Some(opener) = output.edit_toolbar.as_ref() else {
        app.hotbar.drawer_open = false;
        return;
    };
    let groups = drawer_groups();
    let drawer = ToolbarDrawer::new(output.response.rect, &groups, &toolbar, output.dropped).kind(DrawerKind::Searchable).opened_by(opener).show(ui);
    let defaults = slots_with(app, &[]);
    let mut edited: Option<Vec<ToolSlot<String>>> = None;
    for edit in &drawer.edits {
        let base = edited.as_deref().unwrap_or(&toolbar);
        edited = Some(apply_toolbar_edit(base, edit, &defaults));
    }
    if let Some(edited) = edited {
        // Built-in slots are not the user's list; only commands the registry has are kept.
        app.hotbar.tools = edited
            .iter()
            .filter(|slot| !defaults.iter().any(|built_in| built_in.id == slot.id))
            .flat_map(|slot| slot.tools.iter())
            .map(|tool| tool.id.clone())
            .collect();
        app.hotbar.normalize();
    }
    if let Some(id) = drawer.select {
        app.execute(ui.ctx(), &id);
    }
    if drawer.closed {
        app.hotbar.drawer_open = false;
    }
}

fn selected_fill_text(app: &PdfCraftApp) -> Option<(usize, usize)> {
    let (index, id) = app.active_ids()?;
    let (page, annotation) = app.views.get(index)?.session.comments.primary()?;
    let doc = app.session.get(id).filter(|doc| doc.allows_annotation())?;
    doc.info
        .annotations
        .iter()
        .any(|a| a.page == page && a.index == annotation && a.fill_sign && a.subtype == "FreeText" && !a.locked)
        .then_some((page, annotation))
}

/// The controls of the active tool under the rail's slots.
fn footer_controls(ui: &mut egui::Ui, app: &mut PdfCraftApp) {
    match app.quick_tool {
        QuickTool::Fill(FillTool::Text | FillTool::Date) => fill_text_controls(ui, app),
        QuickTool::Comment(tool) => comment_controls(ui, app, tool),
        _ => {}
    }
}

/// The colour of new Fill & Sign text, and of the selected text.
fn set_fill_text_colour(app: &mut PdfCraftApp, ctx: &egui::Context, color: Rgb) {
    app.fill_text_color = color;
    if let Some((page, index)) = selected_fill_text(app) {
        app.apply_edit(ctx, Edit::StyleAnnotation { page, index, color: Some(color), opacity: None, width: None, endings: None });
    }
}

/// Fill & Sign's text controls: its colour, font size and border, for the selected text, or for new text when none
/// is selected. Each opens a popover from the bar, as the comment tools' controls do.
fn fill_text_controls(ui: &mut egui::Ui, app: &mut PdfCraftApp) {
    let picked = selected_fill_text(app);
    let mut colour = comments::colours::PalettePick::default();
    let current = app.fill_text_color;
    comments::popover_from(
        ui,
        tl!("Colour"),
        |ui| Swatch::new(tl!("Colour").to_string(), to_srgba(current)).show(ui).response,
        |ui| {
            let pick = comments::palette(ui, tl!("Colour"), Some(current), "hotbar.fill-text-colour", &app.recent_colours);
            let done = pick.picked.is_some();
            colour.picked = colour.picked.take().or(pick.picked);
            colour.request = colour.request.take().or(pick.request);
            done
        },
    );
    if let Some(color) = colour.picked {
        set_fill_text_colour(app, ui.ctx(), color);
    }
    if let Some(request) = colour.request {
        open_picker(app, request, move |app: &mut PdfCraftApp, ctx: &egui::Context, picked_colour: Srgba| {
            set_fill_text_colour(app, ctx, from_srgba(picked_colour));
        });
    }

    let (min, max) = (f64::from(crate::fill_sign::MIN_TEXT_SIZE), f64::from(crate::fill_sign::MAX_TEXT_SIZE));
    let mut size = app.fill_text_size;
    // #1274: the size also applies to the Fill & Sign text that is selected, once per committed change (a typed
    // value, or the end of a drag), so a drag adds one undo step and not one per frame.
    let mut committed = false;
    comments::popover_from(
        ui,
        tl!("Font size"),
        |ui| IconButton::new(IconId::new("a-large-small")).label(tl!("Font size")).show(ui).response,
        |ui| {
            let field = SliderField::new(tl!("Font size"), Bind::bound(&mut size), min..=max)
                .scale(SliderScale::Log)
                .typed_decimals(12)
                .unit_kind(UnitKind::Points)
                .show(ui);
            committed |= field.edit.events.committed().is_some();
            false
        },
    );
    app.fill_text_size = size;
    if committed
        && let Some((page, index)) = picked
        && app
            .active_ids()
            .and_then(|(_, id)| app.session.get(id)?.comment_props(page, index)?.font_size)
            .is_none_or(|stored| format!("{stored:.3}") != format!("{size:.3}"))
    {
        app.apply_edit(ui.ctx(), Edit::FontSizeAnnotation { page, index, size });
    }

    comments::popover_from(
        ui,
        tl!("Border"),
        |ui| IconButton::new(IconId::new("square")).label(tl!("Border")).show(ui).response,
        |ui| {
            fill_text_border_controls(ui, app, picked);
            false
        },
    );
}

/// Fill & Sign's Border controls for typed text: its width, colour and shape, for the selected text (`picked`), or
/// for new text when none is selected.
fn fill_text_border_controls(ui: &mut egui::Ui, app: &mut PdfCraftApp, picked: Option<(usize, usize)>) {
    // The selected text's own border (Comment Properties may have changed it since): the controls below show it and
    // change only it. Without one they set what new text gets until PdfCraft quits; Make Current Properties Default
    // on typed text keeps it.
    let selected = picked.and_then(|(page, annotation)| {
        let p = app.active_ids().and_then(|(_, id)| app.session.get(id))?.comment_props(page, annotation)?;
        let own = crate::fill_sign::TextBorder { width: p.width.unwrap_or(0.0), color: p.border_color, shape: p.border_shape.unwrap_or_default() };
        Some((page, annotation, own))
    });
    let was = selected.map_or(
        crate::fill_sign::TextBorder { width: app.fill_text_border, color: app.fill_text_border_color, shape: app.fill_text_border_shape },
        |(_, _, own)| own,
    );
    let mut border = was;
    let border_label = |w: f64| if w > 0.0 { format!("{w} pt") } else { tl!("None").to_string() };
    let widths: Vec<Entry<usize>> = crate::fill_sign::TEXT_BORDERS.iter().enumerate().map(|(i, w)| Entry::new(i, border_label(*w))).collect();
    let mut width_at = crate::fill_sign::TEXT_BORDERS.iter().position(|w| *w == was.width).unwrap_or(0);
    Select::new(tl!("Border:").to_string(), Bind::bound(&mut width_at), &widths).show(ui);
    if let Some(width) = crate::fill_sign::TEXT_BORDERS.get(width_at) {
        border.width = *width;
    }
    // The border's own colour (the text's unless picked).
    if was.width > 0.0 {
        let text_colour = selected
            .and_then(|(page, index, _)| app.active_ids().and_then(|(_, id)| app.session.get(id))?.comment_props(page, index)?.color)
            .unwrap_or(app.fill_text_color);
        let pick = border_colour_control(ui, was.color, text_colour, "hotbar.fill-border-colour", &app.recent_colours);
        if let Some(color) = pick.choice {
            border.color = color;
        }
        if let Some(request) = pick.request {
            let target = selected.map(|(page, index, _)| (page, index));
            open_picker(app, request, move |app: &mut PdfCraftApp, ctx: &egui::Context, picked_colour: Srgba| {
                let color = from_srgba(picked_colour);
                match target {
                    Some((page, index)) => {
                        app.apply_edit(ctx, Edit::BorderColorAnnotation { page, index, color: Some(color) });
                    }
                    None => app.fill_text_border_color = Some(color),
                }
            });
        }
        // Its shape: an oval or a circle goes around the text, which keeps its place.
        let shapes: Vec<Entry<usize>> =
            pdfcraft_engine::BorderShape::ALL.iter().enumerate().map(|(i, shape)| Entry::new(i, tl!(shape.name()).to_string())).collect();
        let mut shape_at = pdfcraft_engine::BorderShape::ALL.iter().position(|shape| *shape == was.shape).unwrap_or(0);
        Select::new(tl!("Border shape").to_string(), Bind::bound(&mut shape_at), &shapes).show(ui);
        if let Some(shape) = pdfcraft_engine::BorderShape::ALL.get(shape_at) {
            border.shape = *shape;
        }
    }
    // No border, no shape: an oval or a circle left without its line would keep its box grown around a shape that
    // isn't drawn.
    if border.width <= 0.0 {
        border.shape = pdfcraft_engine::BorderShape::Rectangle;
    }
    match selected {
        Some((page, index, _)) => {
            // One undo step for what one choice changed (Border ▸ None takes a circle's line and its shape).
            let mut edits = Vec::new();
            if border.width != was.width {
                edits.push(Edit::StyleAnnotation { page, index, color: None, opacity: None, width: Some(border.width), endings: None });
            }
            if border.color != was.color {
                edits.push(Edit::BorderColorAnnotation { page, index, color: border.color });
            }
            if border.shape != was.shape {
                edits.push(Edit::BorderShapeAnnotation { page, index, shape: border.shape });
            }
            if edits.len() > 1 {
                app.apply_edit(ui.ctx(), Edit::Batch { label: "Change comment properties".into(), edits });
            } else if let Some(edit) = edits.pop() {
                app.apply_edit(ui.ctx(), edit);
            }
        }
        None => {
            app.fill_text_border = border.width;
            app.fill_text_border_color = border.color;
            app.fill_text_border_shape = border.shape;
        }
    }
}

/// The Border colour control: a swatch of the border as drawn (`own`, else the text's colour `text`) that opens
/// "Same as text" over the palette. `choice` is `Some(None)` for Same as text, `Some(Some(c))` for a colour; a choice
/// closes the popover.
fn border_colour_control(
    ui: &mut egui::Ui,
    own: Option<Rgb>,
    text: Rgb,
    key: &str,
    recent: &comments::RecentColours,
) -> comments::colours::ColourChoice {
    let mut pick = comments::colours::ColourChoice::default();
    comments::popover_from(
        ui,
        tl!("Border colour"),
        |ui| Swatch::new(tl!("Border colour").to_string(), to_srgba(own.unwrap_or(text))).show(ui).response,
        |ui| {
            let this = comments::border_choice(ui, own, key, recent);
            let done = this.choice.is_some();
            pick.choice = pick.choice.take().or(this.choice);
            pick.request = pick.request.take().or(this.request);
            done
        },
    );
    pick
}

/// The controls of the active comment tool: its border colour (text boxes and callouts), and the quick bar's.
fn comment_controls(ui: &mut egui::Ui, app: &mut PdfCraftApp, tool: comments::CommentTool) {
    // Text boxes and callouts: the border's own colour, for new ones and the selected one this tool made (not a
    // locked one). With one selected the well shows its colours (Comment Properties may have changed them), else the
    // tool's.
    if tool.has_border_color() {
        let style = app.comment_prefs.style(tool);
        let selected = app.active_ids().and_then(|(index, id)| {
            let (page, annotation) = app.views.get(index)?.session.comments.primary()?;
            let doc = app.session.get(id)?;
            let made = doc.allows_annotation()
                && doc.info.annotations.iter().any(|a| a.page == page && a.index == annotation && !a.locked && comments::made_by(a, tool));
            made.then(|| (page, annotation, doc.comment_props(page, annotation)))
        });
        let (own, text) = match selected.as_ref().and_then(|(.., p)| p.as_ref()) {
            Some(p) => (p.border_color, p.color.unwrap_or(style.color)),
            None => (style.border_color, style.color),
        };
        let pick = border_colour_control(ui, own, text, "hotbar.comment-border-colour", &app.recent_colours);
        if let Some(color) = pick.choice {
            app.comment_prefs.set_border_color(tool, color);
            if let Some((page, annotation, _)) = &selected {
                app.apply_edit(ui.ctx(), Edit::BorderColorAnnotation { page: *page, index: *annotation, color });
            }
        }
        if let Some(request) = pick.request {
            let target = selected.map(|(page, annotation, _)| (page, annotation));
            open_picker(app, request, move |app: &mut PdfCraftApp, ctx: &egui::Context, picked_colour: Srgba| {
                let color = from_srgba(picked_colour);
                app.comment_prefs.set_border_color(tool, Some(color));
                if let Some((page, annotation)) = target {
                    app.apply_edit(ctx, Edit::BorderColorAnnotation { page, index: annotation, color: Some(color) });
                }
            });
        }
    }
    // The selected comments this tool made take the colour too (locked ones keep theirs).
    let pick = comments::quick_bar_controls(ui, tool, &mut app.comment_prefs, &app.recent_colours);
    if let Some(color) = pick.colour
        && let Some((_, id)) = app.active_ids()
        && app.session.get(id).is_some_and(|doc| doc.allows_annotation())
    {
        app.colour_selection(ui.ctx(), color, |a| comments::made_by(a, tool));
    }
    if let Some(request) = pick.request {
        open_picker(app, request, move |app: &mut PdfCraftApp, ctx: &egui::Context, picked_colour: Srgba| {
            let color = from_srgba(picked_colour);
            if app.active_ids().and_then(|(_, id)| app.session.get(id)).is_some_and(|doc| doc.allows_annotation()) {
                app.colour_selection(ctx, color, |a| comments::made_by(a, tool));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An old `hotbar` object keeps its tools and the side it was docked to, drops what the dock workspace owns now
    /// (position, size, full-side, placement), and a side the dock has no name for docks nowhere.
    #[test]
    fn hotbar_preferences_round_trip() {
        let old = serde_json::json!({
            "tools": ["comment.freehand_highlight", "comment.ink"],
            "position": [0.0, 0.5], "vertical": true, "button_size": 48.0, "dock": "left", "full_side": true, "placed": true,
        });
        let (tools, side) = migrate_legacy(&old);
        assert_eq!(tools, ["comment.freehand_highlight", "comment.ink"]);
        assert_eq!(side, Some(DockSide::Start));
        assert_eq!(migrate_legacy(&serde_json::json!({ "dock": "top" })).1, Some(DockSide::Top));
        assert_eq!(migrate_legacy(&serde_json::json!({ "dock": "float" })).1, None);

        // The list survives a save and a restore, and nothing but the list is saved.
        let mut bar = Hotbar::default();
        bar.set_tools("comment.freehand_highlight,comment.ink").unwrap();
        let saved = serde_json::to_value(&bar).unwrap();
        assert_eq!(saved, serde_json::json!({ "tools": ["comment.freehand_highlight", "comment.ink"] }));
        let restored: Hotbar = serde_json::from_value(old).unwrap();
        assert_eq!(restored.tools, bar.tools, "an old object with the dropped keys still deserializes");

        // The floating bar's options are gone from the control channel.
        let ctx = egui::Context::default();
        let mut app = PdfCraftApp::for_unit_tests();
        for key in ["hotbar-size", "hotbar-side", "hotbar-full-side"] {
            assert!(app.set_option(&ctx, key, "left").is_err(), "{key} is removed");
        }
        app.set_option(&ctx, "hotbar-tools", "comment.freehand_highlight,comment.ink").unwrap();
        let saved = app.settings_snapshot();
        let mut fresh = PdfCraftApp::for_unit_tests();
        fresh.apply_settings(&saved);
        assert_eq!(fresh.hotbar.tools, app.hotbar.tools);
    }

    /// Ids the registry no longer has, and duplicates, are dropped on restore; an unknown id given to the control
    /// channel is refused and leaves the list as it was.
    #[test]
    fn hotbar_restore_filters_unknown_tools_and_offscreen_positions() {
        let (tools, _) = migrate_legacy(&serde_json::json!({ "tools": ["invalid", "comment.ink", "comment.ink"] }));
        assert_eq!(tools, ["comment.ink"]);

        let mut app = PdfCraftApp::for_unit_tests();
        app.hotbar.set_tools("comment.ink").unwrap();
        assert!(app.set_option(&egui::Context::default(), "hotbar-tools", "does.not.exist").is_err());
        assert_eq!(app.hotbar.tools, ["comment.ink"]);
    }
}
