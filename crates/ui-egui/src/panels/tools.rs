use craft::behavior::{DisplayOrder, IconId, ItemSource, Selection};
use craft::theme::{
    Variant,
    variant::{Emphasis, Tone},
};
use craft::ui::buttons::Button;
use craft::ui::choice::{Checkbox, Segmented};
use craft::ui::contract::{
    ActivateOn, Bind, CollectionEvent, Entry, Outcome as _, Props as _, SelectMode, Show as _, ShowCollection as _, Sizing, clicked,
};
use craft::ui::display::Badge;
use craft::ui::list::{HueTarget, List, ListRow, RowEvent, RowInk, TrailingFace, TrailingPart};
use craft::ui::primitives::IconSpec;
use craft::ui::style::DataColor;
use egui::Color32;

use super::edit::{actions_menu, edit_panel, stamp_palette};
use super::{Availability, LeftPanel, PdfCraftApp, TOOL_GROUPS, ToolGroup, ToolItem, catalog};

const COLLAPSED_TOOLS: usize = 14;

/// The pin part of a tool row.
const PIN: TrailingPart = TrailingPart::new(0);

/// A display order over `0..len`, keyed by position: the source of a list or grid whose rows the caller builds from
/// its own vector.
pub(super) struct Rows(pub(super) usize);

impl DisplayOrder<usize> for Rows {
    fn len(&self) -> usize {
        self.0
    }

    fn key(&self, at: usize) -> usize {
        at
    }

    fn index_of(&self, key: &usize) -> Option<usize> {
        (*key < self.0).then_some(*key)
    }

    fn text(&self, _: usize) -> Option<std::borrow::Cow<'_, str>> {
        None
    }
}

impl ItemSource<usize> for Rows {}

/// The colour a tool's data gives its icon.
pub(super) fn data_hue(g: &ToolGroup) -> DataColor {
    DataColor::new(Color32::from_rgb(g.hue[0], g.hue[1], g.hue[2]))
}

/// One line of text in the body: a section heading, or a quiet note. It takes its line's height, so the controls
/// and the list after it keep the panel's room.
pub(super) fn caption_row(ui: &mut egui::Ui, text: &str, heading: bool) {
    let label = text.to_owned();
    let list = List::<usize>::new(label.clone()).sizing(Sizing::Content).natural_height(true);
    list.selection(Bind::given(Selection::default())).show_items(ui, &Rows(1), &mut |_, _| {
        let row = ListRow::new(label.clone());
        if heading { row.heading().ink(RowInk::Dimmed) } else { row.ink(RowInk::Muted) }
    });
}

pub fn left_panel(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    // Organize closed another way (the grid's Close, or a page opened from it) closes the tool too,
    // as its panel's Close does, back to the workspace's own panel.
    if app.left == LeftPanel::Tool("organize") && app.active.and_then(|i| app.views.get(i)).is_some_and(|v| !v.organize) {
        app.left = crate::organize_panel::workspace_panel(app.mode);
    }
    match app.left {
        // Organize pages has its own panel whenever the grid shows (with a document open).
        _ if app.active.is_some() && crate::organize_panel::showing(app) => crate::organize_panel::panel(app, ui),
        LeftPanel::AllTools => all_tools(app, ui),
        LeftPanel::Tool(id) => match catalog::group(id) {
            Some(g) => tool_detail(app, ui, g),
            None => app.left = LeftPanel::AllTools,
        },
        LeftPanel::CalculationOrder => crate::calc_order_ui::panel(app, ui),
    }
}

/// One tool-group row: its icon, label, badge and the pin part.
fn tool_row(g: &'static ToolGroup, pinned: bool) -> ListRow<'static> {
    let mut row = ListRow::new(tl!(g.label).to_owned()).icon(IconSpec::svg(IconId::new(g.icon)));
    row = row.hue(data_hue(g), HueTarget::Icon);
    if let Some(b) = g.badge {
        row = row.trailing_named(tl!(b).to_owned(), tl!(b).to_owned());
    }
    let pin_name = if pinned { tl!("Unpin from Home") } else { tl!("Pin to Home") }.to_owned();
    let face = TrailingFace::Icon(IconSpec::svg(IconId::new("pin")));
    row = if pinned { row.trailing_part(PIN, face, pin_name) } else { row.trailing_part_revealed(PIN, face, pin_name) };
    let tip = match g.availability {
        Availability::Ready => tl!("Available").to_string(),
        Availability::Planned(m) => format!("{} {m} — {}", tl!("Planned for milestone"), tl!("open to see what it will include")),
        Availability::Provider => tl!("Optional: needs an AI provider you configure").into(),
    };
    row.tooltip(tip)
}

/// The All tools sidebar body: the tool list, the first few tools until View more shows them all. Its Expand and
/// Close belong to the dock's group header.
fn all_tools(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let shown = if app.all_tools_expanded { TOOL_GROUPS.len() } else { COLLAPSED_TOOLS.min(TOOL_GROUPS.len()) };
    let pins: Vec<bool> = TOOL_GROUPS[..shown].iter().map(|g| app.is_tool_pinned(g.id)).collect();
    let out = List::<usize>::new(tl!("All tools").to_owned())
        .natural_height(true)
        .select(SelectMode::Single)
        .activate(ActivateOn::Click)
        .selection(Bind::given(Selection::default()))
        .show_items(ui, &Rows(shown), &mut |at, _| match (TOOL_GROUPS.get(*at), pins.get(*at)) {
            (Some(g), Some(pinned)) => tool_row(g, *pinned),
            _ => ListRow::spacer(),
        });
    for event in out.events {
        match event {
            CollectionEvent::Activate(at) => {
                if let Some(g) = TOOL_GROUPS.get(at) {
                    app.select_tool(ui.ctx(), g.id);
                }
            }
            CollectionEvent::Item(at, RowEvent::Trailing { part, .. }) if part == PIN => {
                if let Some(g) = TOOL_GROUPS.get(at) {
                    app.toggle_pin_tool(g.id);
                }
            }
            _ => {}
        }
    }
    let more = if app.all_tools_expanded { "View less" } else { "View more" };
    if clicked(craft::ui::buttons::Link::new(tl!(more).to_owned()).show(ui).response()) {
        app.all_tools_expanded = !app.all_tools_expanded;
    }
}

/// A line of a tool's detail list.
#[derive(Clone, Copy)]
enum Line {
    Heading(&'static str),
    Item(&'static ToolItem),
}

/// One item row: its icon, label and the availability chip.
fn item_row(g: &'static ToolGroup, item: &'static ToolItem) -> ListRow<'static> {
    let ready = item.availability == Availability::Ready;
    let chip = match item.availability {
        Availability::Ready => tl!("Ready"),
        Availability::Planned(m) => m,
        Availability::Provider => "AI",
    };
    let badge = Badge::new(tl!(chip).to_owned()).variant(Variant::look(Emphasis::Soft, if ready { Tone::Good } else { Tone::Neutral }));
    let row = ListRow::new(tl!(item.label).to_owned()).icon(IconSpec::svg(IconId::new(item.icon))).trailing_field(badge, |_| {});
    if ready {
        row.hue(data_hue(g), HueTarget::Icon).tooltip(format!("{} — {}", tl!(item.label), item.command))
    } else {
        row.ink(RowInk::Dimmed).tooltip(tl!("Planned").to_owned())
    }
}

/// The detail view for one tool group, dispatching its commands and tool-specific panels. Its Back and Close belong
/// to the dock's group header.
fn tool_detail(app: &mut PdfCraftApp, ui: &mut egui::Ui, g: &'static ToolGroup) {
    if g.id == "edit" {
        edit_panel(app, ui, g);
        return;
    }
    if let Availability::Planned(m) = g.availability {
        caption_row(ui, &format!("{} {m}. {}", tl!("Coming in milestone"), tl!("Items marked Ready work today.")), false);
    }
    if g.id == "measure" {
        crate::measure_ui::panel(app, ui);
        return;
    }
    // Add a stamp: the palette.
    if g.id == "stamp" {
        stamp_palette(app, ui);
        return;
    }
    // Prepare a form: Preview fills the form as a reader would; Edit goes back.
    if g.id == "form" {
        if let Some(view) = app.active.and_then(|i| app.views.get_mut(i)) {
            Checkbox::new(Bind::bound(&mut view.session.prepare.show_tab_numbers)).label(tl!("Show tab numbers").to_owned()).show(ui);
        }
        let entries = [Entry::new(false, tl!("Edit fields").to_owned()), Entry::new(true, tl!("Preview").to_owned())];
        let landed = Segmented::new(tl!("Form mode").to_owned(), Bind::given(app.form_preview), &entries)
            .tooltip(tl!("Try the form as people filling it in will see it").to_owned())
            .show(ui)
            .events
            .landed()
            .cloned();
        if let Some(preview) = landed {
            app.form_preview = preview;
            if preview {
                app.quick_tool = crate::QuickTool::Select;
            }
        }
    }
    let mut run = None;
    let mut lines = Vec::new();
    for s in g.sections {
        lines.push(Line::Heading(s.title));
        for item in s.items {
            if g.id == "fill_sign" && (item.command.starts_with("sign.fill.signature") || item.command.starts_with("sign.fill.initials")) {
                continue;
            }
            lines.push(Line::Item(item));
        }
    }
    let mut list = List::<usize>::new(tl!(g.label).to_owned());
    if g.id == "fill_sign" {
        list = list.sizing(Sizing::Content).natural_height(true);
    }
    let out = list.select(SelectMode::Single).activate(ActivateOn::Click).selection(Bind::given(Selection::default())).show_items(
        ui,
        &Rows(lines.len()),
        &mut |at, _| match lines.get(*at) {
            Some(Line::Heading(title)) => ListRow::new(tl!(title).to_owned()).heading().ink(RowInk::Dimmed),
            Some(Line::Item(item)) => item_row(g, item),
            None => ListRow::spacer(),
        },
    );
    let opener = out.response.clone();
    let mut menu = None;
    for event in out.events {
        if let CollectionEvent::Activate(at) = event
            && let Some(Line::Item(item)) = lines.get(at)
        {
            if item.actions.is_empty() {
                run = Some(item.command);
            } else {
                menu = Some(*item);
            }
        }
    }
    // An entry with further commands opens a menu from its row.
    // PLAN: assumed actions_menu(app, ui, opener: &Response, item: &'static ToolItem) -> Option<&'static str> (pdf-edit-w6); it keeps its own open state.
    if let Some(item) = menu
        && let Some(cmd) = actions_menu(app, ui, &opener, item)
    {
        run = Some(cmd);
    }
    if g.id == "fill_sign" {
        caption_row(ui, tl!("Sign yourself"), true);
        if let Some(cmd) = crate::fill_sign::signature_entries(ui, app) {
            run = Some(cmd);
        }
        // Where the signing happens, not only in Preferences, and once: not also in the Sign
        // menu over the page, where it was easy to miss (#2149).
        Checkbox::new(Bind::bound(&mut app.flatten_fill_sign_on_save)).label(tl!("Lock into the page when saving").to_owned()).show(ui);
    }
    if let Some(cmd) = run {
        app.run_command(ui.ctx(), cmd);
    }
}

/// Whether the Tools panel shows a tool whose actions sit in a footer pinned under the body: Redact a PDF's.
pub(crate) fn has_footer(app: &PdfCraftApp) -> bool {
    app.left == LeftPanel::Tool("redact") && !(app.active.is_some() && crate::organize_panel::showing(app))
}

/// The Tools panel's pinned footer: Redact a PDF's mark count, Clear all and Redact all, in view while the body
/// scrolls, as production pinned them.
pub(crate) fn footer(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    if !has_footer(app) {
        return;
    }
    let marks = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(0, |d| d.redaction_marks());
    if marks > 0 {
        caption_row(
            ui,
            &if marks == 1 { crate::i18n::fmt(tl!("1 mark"), &[]) } else { crate::i18n::fmt(tl!("{n} marks"), &[("n", &marks.to_string())]) },
            false,
        );
    }
    let mut run = None;
    if Button::new(tl!("Clear all").to_owned()).enabled(marks > 0).show(ui).clicked() {
        run = Some("redact.clear");
    }
    if Button::new(tl!("Redact all").to_owned()).enabled(marks > 0).show(ui).clicked() {
        run = Some("redact.apply");
    }
    if let Some(cmd) = run {
        app.run_command(ui.ctx(), cmd);
    }
}
