//! Help ▸ Rulers, guides and grid: a short guide to the rulers, guides, grid and guide sets,
//! shown in the app. The set file format is described in full in `persist/guide_sets.rs`.

use crate::PdfCraftApp;

/// The example set file shown in the help, to copy into the guide-set folder.
pub const EXAMPLE: &str = r##"format = "pdfcraft-guides"
version = 1
name = "A4 type area"
unit = "mm"
shortcut = 1

[style]
color = "#E6005C"
width = 1.0

[[guide]]
axis = "horizontal"
at = 20
from = "top"
label = "Header"

[[guide]]
axis = "vertical"
at = 20
from = "right"
pages = "all"

[grid]
width = 10
height = 10
subdivisions = 2
"##;

/// One section of the help: a heading and its paragraphs (English, translated through the catalog).
const SECTIONS: &[(&str, &[&str])] = &[
    (
        "Rulers",
        &[
            "View ▸ Rulers ({rulers}) shows a ruler above and beside the pages. Click the corner between them to step to the next unit, or right-click a ruler to choose one.",
        ],
    ),
    (
        "Guides",
        &[
            "Drag from a ruler onto a page to place a guide: from the top ruler for a horizontal one, from the left ruler for a vertical one. Hold Shift as you drop it to put it on every page.",
            "Drag a guide to move it. Drag it back onto a ruler, or off the page, to remove it.",
            "Right-click a guide for a colour, Properties… (colour, thickness, label, every page) and Delete. Double-click opens Properties too.",
            "View ▸ Guides ({guides}) hides and shows them; View ▸ Clear guides removes this document's guides. Guides are never saved in the PDF.",
        ],
    ),
    (
        "Grid",
        &[
            "View ▸ Grid ({grid}) draws a grid over the pages; View ▸ Snap to grid ({snap}) makes a dragged guide snap to it.",
            "Spacing, offset, subdivisions, colour and thickness are in Preferences ▸ Units and guides.",
        ],
    ),
    (
        "Guide sets",
        &[
            "Right-click a ruler ▸ Save guides as a set… keeps the guides, the unit and, if you like, the grid under a name and a shortcut. {first} … {last} shows and hides a set over any document; Edit in document turns it back into guides you can move.",
            "Each set is a text file in the guide-set folder (Preferences ▸ Units and guides ▸ Open folder). Edit it in any text editor, send it to others, or drop theirs into the folder, then click Reload. Distances are measured from the edge you name, so a set fits other page sizes.",
        ],
    ),
    (
        "For scripts and AI agents",
        &[
            "With the control channel on, ui.state lists every guide (in the set format, and where it lies on screen) and the sets with their files and errors. ui.set document-guides <set as JSON or TOML> replaces the document's guides, guide-set <name>:on|off|toggle shows a set, guide-set-file <set> saves one, and guides, grid and snap-to-grid take on or off.",
        ],
    ),
];

/// The shortcuts the help names, as written on this platform: `{rulers}`, `{guides}`, `{grid}`,
/// `{snap}`, and `{first}` … `{last}` for the guide sets.
pub fn shortcut_args(ctx: &egui::Context) -> [(&'static str, String); 6] {
    let key = |id| crate::commands::command_shortcut_label(ctx, id);
    [
        ("rulers", key("view.rulers")),
        ("guides", key("view.guides")),
        ("grid", key("view.grid")),
        ("snap", key("view.snap_to_grid")),
        ("first", crate::guides::set_shortcut_label(1, ctx)),
        ("last", crate::guides::set_shortcut_label(9, ctx)),
    ]
}

/// The help window, while it is open.
pub fn window(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if !app.guides_help_open {
        return;
    }
    let mut open = true;
    egui::Window::new(tl!("Rulers, guides and grid"))
        .id(egui::Id::unique("pdfcraft-guides-help"))
        .open(&mut open)
        .collapsible(false)
        .default_width(520.0)
        .default_height(560.0)
        .show(ctx, |ui| {
            let keys = shortcut_args(ctx);
            let args: Vec<(&str, &str)> = keys.iter().map(|(k, v)| (*k, v.as_str())).collect();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (heading, paragraphs) in SECTIONS {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(tl!(heading)).strong().size(15.0));
                    for p in *paragraphs {
                        ui.add(egui::Label::new(crate::i18n::fmt(tl!(p), &args)).wrap());
                    }
                }
                ui.add_space(8.0);
                ui.label(egui::RichText::new(tl!("Example set file")).strong());
                let mut text = EXAMPLE.to_string();
                ui.add(egui::TextEdit::multiline(&mut text).code_editor().desired_rows(12).interactive(false).desired_width(f32::INFINITY));
                ui.horizontal(|ui| {
                    if ui.button(tl!("Copy example")).clicked() {
                        ui.ctx().copy_text(EXAMPLE.to_string());
                    }
                    if ui.button(tl!("Open Units and guides")).clicked() {
                        app.preferences_page = crate::prefs::PrefsPage::UnitsGuides;
                        app.dialog = Some(crate::Dialog::Preferences);
                    }
                });
            });
        });
    if !open {
        app.guides_help_open = false;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_example_in_the_help_is_a_valid_set() {
        let set = crate::guide_sets::read_toml(super::EXAMPLE).unwrap();
        assert_eq!((set.guides.len(), set.shortcut), (2, Some(1)));
        assert!(set.grid.is_some());
    }
}
