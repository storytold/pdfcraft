//! Home ▸ Pin a tool…: pick the tool groups shown on the Home page.

use std::cell::Cell;

use craft::behavior::{DisplayOrder, IconId, ItemSource};
use craft::ui::buttons::ToggleButton;
use craft::ui::contract::{Bind, Props as _, SelectMode, Show as _, ShowCollection as _, ShowContainer as _, Sizing};
use craft::ui::dialog::{Dialog, DialogId, DialogResult, DialogStack, StackDialog, dialog_id};
use craft::ui::display::Text;
use craft::ui::layout_components::Stack;
use craft::ui::list::{HueTarget, List, ListRow};
use craft::ui::primitives::IconSpec;
use craft::ui::runtime::env::Label;
use craft::ui::sections::{DialogRole, FooterButton};
use craft::ui::style::DataColor;
use craft::ui::text::TextField;
use egui::{Color32, Ui};

use crate::PdfCraftApp;

/// One tool group as the dialog lists it.
struct Row {
    id: &'static str,
    icon: &'static str,
    hue: [u8; 3],
    label: String,
    /// The first four items of the group's first section.
    detail: String,
}

/// The rows as a collection source: position is the key, the label is the row's text.
struct Listed<'a>(&'a [Row]);

impl DisplayOrder<usize> for Listed<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn key(&self, at: usize) -> usize {
        at
    }

    fn index_of(&self, key: &usize) -> Option<usize> {
        (*key < self.0.len()).then_some(*key)
    }

    fn text(&self, at: usize) -> Option<std::borrow::Cow<'_, str>> {
        self.0.get(at).map(|row| std::borrow::Cow::Borrowed(row.label.as_str()))
    }
}

impl ItemSource<usize> for Listed<'_> {}

/// The Pin tools dialog on the dialog host. It cannot see the app: a press on Pin or Pinned is left in `toggled`
/// for [`PdfCraftApp::pin_tools_pass`], which applies it and writes the app's answer back into `pinned`.
pub(crate) struct PinTools {
    id: DialogId,
    rows: Vec<Row>,
    query: String,
    /// The ids of the pinned tool groups.
    pinned: Vec<String>,
    /// The group whose button was pressed since the last pass.
    toggled: Option<String>,
}

impl PinTools {
    fn new(pinned: Vec<String>) -> Self {
        let rows = pdfcraft_engine::catalog::TOOL_GROUPS
            .iter()
            .map(|g| Row {
                id: g.id,
                icon: g.icon,
                hue: g.hue,
                label: tl!(g.label).to_string(),
                detail: g.sections.first().map(|s| s.items.iter().take(4).map(|i| tl!(i.label)).collect::<Vec<_>>().join(" • ")).unwrap_or_default(),
            })
            .collect();
        PinTools { id: dialog_id("pin-tool"), rows, query: String::new(), pinned, toggled: None }
    }
}

impl StackDialog for PinTools {
    type Result = ();

    fn id(&self) -> DialogId {
        self.id
    }

    fn key(&self) -> Option<&str> {
        Some("pin-tool")
    }

    fn show(&mut self, ui: &mut Ui, _stack: &DialogStack) -> Option<DialogResult<()>> {
        let pressed = Cell::new(None);
        let Self { rows, query, pinned, .. } = self;
        let shown = Dialog::new(tl!("Pin tools to Home")).buttons([FooterButton::new(tl!("Done"), DialogRole::Accept)]).show(ui, |ui, _| {
            Stack::new().show(ui, |ui, _| {
                Text::new(tl!("Select tools to display on your Home page for quick access.")).show(ui);
                TextField::search(tl!("Search tools…"), Bind::bound(&mut *query)).show(ui);
                let texts = |i: &usize| {
                    rows.get(*i).map(|r| vec![Label::from(r.label.clone()), Label::from(r.detail.clone()), Label::from(r.id)]).unwrap_or_default()
                };
                List::<usize>::new(tl!("Tools"))
                    .select(SelectMode::Single)
                    .sizing(Sizing::Content)
                    .max_rows(8)
                    .filter(query.trim(), texts)
                    .show_items(ui, &Listed(rows), &mut |key, _| {
                        let Some(row) = rows.get(*key) else {
                            return ListRow::new("");
                        };
                        let on = pinned.iter().any(|p| p == row.id);
                        let button = if on { tl!("Pinned") } else { tl!("Pin") };
                        let [r, g, b] = row.hue;
                        let at = *key;
                        let pressed = &pressed;
                        ListRow::new(row.label.clone())
                            .icon(IconSpec::svg(IconId::new(row.icon)))
                            .hue(DataColor::new(Color32::from_rgb(r, g, b)), HueTarget::Icon)
                            .detail(row.detail.clone())
                            .trailing_field(ToggleButton::new(button, Bind::given(on)), move |out| {
                                if out.events.committed().is_some() {
                                    pressed.set(Some(at));
                                }
                            })
                    });
            });
        });
        if let Some(at) = pressed.get() {
            self.toggled = self.rows.get(at).map(|r| r.id.to_owned());
        }
        shown.output.result(|| ())
    }
}

impl PdfCraftApp {
    /// Opens the Pin tools dialog (Home ▸ Pin a tool…).
    pub(crate) fn open_pin_tools(&mut self) {
        let pinned = pdfcraft_engine::catalog::TOOL_GROUPS.iter().filter(|g| self.is_tool_pinned(g.id)).map(|g| g.id.to_owned()).collect();
        crate::dialogs::host::open(self, PinTools::new(pinned));
    }

    /// Applies the pin the user pressed and shows the app's answer in the dialog.
    pub(crate) fn pin_tools_pass(&mut self, _ctx: &egui::Context) {
        let Some(id) = self.dialogs.get_mut::<PinTools>().and_then(|d| d.toggled.take()) else {
            return;
        };
        if let Some(group) = pdfcraft_engine::catalog::TOOL_GROUPS.iter().find(|g| g.id == id) {
            self.toggle_pin_tool(group.id);
        }
        let pinned: Vec<String> =
            pdfcraft_engine::catalog::TOOL_GROUPS.iter().filter(|g| self.is_tool_pinned(g.id)).map(|g| g.id.to_owned()).collect();
        if let Some(dialog) = self.dialogs.get_mut::<PinTools>() {
            dialog.pinned = pinned;
        }
    }
}
