//! The Print dialog's rows: the app's sections for `HostedPrint`, and the conversion between the
//! draft and the dialog's field values.
//!
//! The draft is the model; the fields are what the rows edit. The shared job settings (printer,
//! copies, collate, grayscale, two-sided) live in the dialog's `State`, not in the fields.

use craft::print::ui::PrintSection;
use craft_ui::buttons::Button;
use craft_ui::choice::Segmented;
use craft_ui::contract::{Bind, Entry, Outcome as _, Props as _, Show as _, ShowContainer as _, clicked};
use craft_ui::layout_components::Row;
use craft_ui::patterns::form_dialogs::{DialogFields, Param, ParamKind};
use craft_ui::runtime::env::{Label, UnitKind};
use pdfcraft_engine::print::{self, Binding, BookletSubset, Content, Orientation, PageOrder, Subset, spool};

use super::draft::{CUSTOM_SCALE, Handling, IMAGE_DPI, PrintDraft, Which};
use crate::print_paper::CustomPaper;
use crate::units::Unit;

const WHICH: [(Which, &str); 5] =
    [(Which::All, "all"), (Which::Current, "current"), (Which::Range, "range"), (Which::Area, "area"), (Which::Selected, "selected")];
const HANDLING: [(Handling, &str); 5] = [
    (Handling::Size, "size"),
    (Handling::Poster, "poster"),
    (Handling::Multiple, "multiple"),
    (Handling::Booklet, "booklet"),
    (Handling::Label, "label"),
];
const SUBSET: [(Subset, &str); 3] = [(Subset::All, "all"), (Subset::Odd, "odd"), (Subset::Even, "even")];
const ORIENTATION: [(Orientation, &str); 3] =
    [(Orientation::Auto, "auto"), (Orientation::Portrait, "portrait"), (Orientation::Landscape, "landscape")];
const ORDER: [(PageOrder, &str); 5] = [
    (PageOrder::Horizontal, "horizontal"),
    (PageOrder::HorizontalReversed, "horizontal_reversed"),
    (PageOrder::Vertical, "vertical"),
    (PageOrder::VerticalReversed, "vertical_reversed"),
    (PageOrder::CutStack, "cut_stack"),
];
const BOOKLET: [(BookletSubset, &str); 3] =
    [(BookletSubset::BothSides, "both"), (BookletSubset::FrontOnly, "front"), (BookletSubset::BackOnly, "back")];
const BINDING: [(Binding, &str); 2] = [(Binding::Left, "left"), (Binding::Right, "right")];
const CONTENT: [(Content, &str); 4] = [
    (Content::Document, "document"),
    (Content::DocumentAndMarkups, "document_and_markups"),
    (Content::DocumentAndStamps, "document_and_stamps"),
    (Content::FormFieldsOnly, "form_fields_only"),
];
/// The pages-per-sheet counts the dialog offers.
const PER_SHEET: [usize; 5] = [2, 4, 6, 9, 16];
/// The id of the typed columns by rows grid among the pages-per-sheet choices.
const CUSTOM_GRID: &str = "custom";

fn id_of<T: Copy + PartialEq>(table: &[(T, &'static str)], value: T) -> &'static str {
    table.iter().find(|(v, _)| *v == value).map_or("", |(_, id)| id)
}

fn value_of<T: Copy>(table: &[(T, &'static str)], id: &str) -> Option<T> {
    table.iter().find(|(_, i)| *i == id).map(|(v, _)| *v)
}

/// The sizing tab a `handling.<id>` action names.
pub(super) fn handling_from_id(id: &str) -> Option<Handling> {
    value_of(&HANDLING, id)
}

fn lbl(text: impl AsRef<str>) -> Label {
    Label::from(text.as_ref().to_owned())
}

/// A row label: the dialog's captions end in a colon, a row's label does not.
fn row_label(text: impl AsRef<str>) -> Label {
    lbl(text.as_ref().trim_end_matches(':'))
}

/// The options of a choice row: `labelled` pairs each value with its caption, the ids come from `table`.
fn options<T: Copy + PartialEq>(table: &[(T, &'static str)], labelled: Vec<(T, Label)>) -> Vec<(String, Label)> {
    labelled.into_iter().map(|(v, l)| (id_of(table, v).to_owned(), l)).collect()
}

fn choice(key: &str, label: Label, options: Vec<(String, Label)>, current: &str) -> Param {
    Param::new(key, label, ParamKind::choice(options, Some(current.to_owned())))
}

fn radio(key: &str, label: Label, options: Vec<(String, Label)>, current: &str) -> Param {
    Param::new(key, label, ParamKind::radio(options, Some(current.to_owned())))
}

fn check(key: &str, label: Label, on: bool) -> Param {
    Param::new(key, label, ParamKind::bool(on))
}

fn number(key: &str, label: Label, (lo, hi): (f64, f64), value: f64, step: f64, decimals: u32, unit: UnitKind) -> Param {
    Param::new(key, label, ParamKind::number(Some(lo), Some(hi), value, step, decimals)).unit(unit)
}

fn note(key: &str, text: impl AsRef<str>) -> Param {
    Param::read_only_text(key, Label::default(), lbl(text))
}

fn unit_kind(unit: Unit) -> UnitKind {
    match unit {
        Unit::Inches => UnitKind::Inches,
        Unit::Millimetres => UnitKind::Millimetres,
        Unit::Centimetres => UnitKind::Centimetres,
        Unit::Points => UnitKind::Points,
    }
}

/// A row of push buttons; a press files its id as the dialog's action. Each button is the id, its caption, whether it is
/// enabled and its tooltip.
fn button_row(key: &str, label: Label, buttons: Vec<(&'static str, Label, bool, Option<Label>)>) -> Param {
    Param::custom(key, label, move |ui, _, cx| {
        let mut last = None;
        Row::new().show(ui, |ui, _| {
            for (id, text, enabled, tip) in &buttons {
                let mut button = Button::new(text.clone()).enabled(*enabled).id_salt(*id);
                if let Some(tip) = tip {
                    button = button.tooltip(tip.clone());
                }
                let shown = button.show(ui);
                if clicked(&shown) {
                    cx.request_action(*id);
                }
                last = Some(shown);
            }
        });
        last.map_or_else(|| ui.response(), |b| b.response)
    })
    .full_width()
}

/// The Size, Poster, Multiple, Booklet and Label tabs. A press writes the `handling` field, which `draft_from_fields`
/// applies through the draft's `set_handling`; every tab but Size is off while an area is chosen.
fn handling_tabs(d: &PrintDraft) -> Param {
    let area = d.which == Which::Area;
    let tabs = vec![
        (Handling::Size, lbl(tl!("Size"))),
        (Handling::Poster, lbl(tl!("Poster"))),
        (Handling::Multiple, lbl(tl!("Multiple"))),
        (Handling::Booklet, lbl(tl!("Booklet"))),
        (Handling::Label, lbl(tl!("Label"))),
    ];
    let name = lbl(tl!("Page Sizing & Handling"));
    Param::custom("handling", name.clone(), move |ui, fields, _| {
        let now = fields.text("handling").unwrap_or("size").to_owned();
        let entries: Vec<Entry<String>> =
            tabs.iter().map(|(h, text)| Entry::new(id_of(&HANDLING, *h).to_owned(), text.clone()).enabled(!area || *h == Handling::Size)).collect();
        let shown = Segmented::new(name.clone(), Bind::given(now.clone()), &entries).show(ui);
        if let Some(picked) = shown.value().cloned()
            && picked != now
        {
            fields.set("handling", picked);
        }
        shown.response
    })
    .full_width()
}

/// The dialog's sections for this draft. `has_area` offers the selected area; `no_printers` says the spooler listed
/// none.
pub(super) fn build(d: &PrintDraft, sizes: &[(f64, f64)], units: Unit, has_area: bool, no_printers: bool) -> Vec<PrintSection> {
    let mut sections = vec![paper_section(d, sizes, units, no_printers)];
    if d.show_driver_options
        && let Some(section) = driver_section(d)
    {
        sections.push(section);
    }
    sections.push(pages_section(d, sizes.len(), units, has_area));
    sections.push(sizing_section(d));
    sections.push(PrintSection::new(lbl(tl!("Comments & Forms")), vec![content_row(d)]));
    sections.push(advanced_section(d));
    sections
}

fn content_row(d: &PrintDraft) -> Param {
    let opts = options(
        &CONTENT,
        vec![
            (Content::Document, lbl(tl!("Document"))),
            (Content::DocumentAndMarkups, lbl(tl!("Document and markups"))),
            (Content::DocumentAndStamps, lbl(tl!("Document and stamps"))),
            (Content::FormFieldsOnly, lbl(tl!("Form fields only"))),
        ],
    );
    choice("content", lbl(tl!("Comments & Forms")), opts, id_of(&CONTENT, d.content))
}

fn paper_section(d: &PrintDraft, sizes: &[(f64, f64)], units: Unit, no_printers: bool) -> PrintSection {
    let detecting = d.papers_loading();
    // Each paper is keyed by its name, which no two papers share: a list that changes under the
    // dialog (a printer's own sizes arriving) leaves a pick on the paper it named.
    let mut papers: Vec<(String, Label)> = d.papers.iter().map(|(name, _)| (name.clone(), lbl(tl!(name)))).collect();
    // The line between the standard sizes and the custom ones falls before the first custom size.
    let mut separator = papers.len();
    papers.extend(d.custom_papers.iter().map(|p| (p.name.clone(), lbl(format!("{} ({})", p.name, units.size(p.width, p.height))))));
    if detecting {
        papers.insert(0, ("detecting".to_owned(), lbl(tl!("Detecting paper…"))));
        separator += 1;
    }
    let mut paper = Param::new("paper", row_label(tl!("Paper:")), ParamKind::choice(papers, Some(d.paper_name())));
    if !d.custom_papers.is_empty() {
        paper = paper.separated_at(separator);
    }
    if detecting {
        paper = paper.disabled_option("detecting", lbl(tl!("The printer's paper sizes are still being read")));
    }
    let browser = d.printer.as_deref() == Some(crate::web_print::BROWSER_PRINTER);
    let has_printer = d.printer.is_some() && !browser;
    let page_setup_tip = if spool::HAS_PRINTER_PREFERENCES {
        tl!("Selects paper size and orientation from the printer driver. For paper tray, print quality or other driver defaults, use Properties…")
    } else {
        tl!("Page setup dialog is not available on this system.")
    };
    let mut buttons = vec![("custom", lbl(tl!("Custom…")), true, Some(lbl(tl!("Add a paper size of your own to the list"))))];
    if d.paper_is_custom() {
        buttons.push(("paper.remove", lbl(tl!("Remove")), true, Some(lbl(tl!("Take this paper size off the list")))));
    }
    buttons.push(("paper.setup", lbl(tl!("Page Setup…")), has_printer && spool::HAS_PRINTER_PREFERENCES, Some(lbl(page_setup_tip))));
    let source = sizes.get(d.current_page).copied();
    if let Some(source) = source {
        buttons.push(("paper.match", lbl(tl!("Match page at 100%")), d.matching_paper(source).is_some(), None));
    }
    let mut params = vec![paper, button_row("paper_actions", Label::default(), buttons)];

    let sides = (0.0, crate::print_paper::MAX_SIDE * units.per_point());
    let kind = unit_kind(units);
    let decimals = units.decimals() as u32;
    params.push(Param::new("custom_name", lbl(tl!("Name")), ParamKind::text(String::new())).visible_when("custom_open", true));
    params.push(number("custom_width", lbl(tl!("Width")), sides, 0.0, units.speed(), decimals, kind).visible_when("custom_open", true));
    params.push(number("custom_height", lbl(tl!("Height")), sides, 0.0, units.speed(), decimals, kind).visible_when("custom_open", true));
    let accepted = typed_custom_paper(d, units).is_some();
    params.push(
        button_row(
            "custom_buttons",
            Label::default(),
            vec![("custom.add", lbl(tl!("Add")), accepted, None), ("custom.cancel", lbl(tl!("Cancel")), true, None)],
        )
        .visible_when("custom_open", true),
    );

    if let Some(source) = source {
        params.push(note("note.pdf_page", crate::i18n::fmt(tl!("PDF page: {size}"), &[("size", &units.size(source.0, source.1))])));
    }
    // Windows says which drivers can't print two-sided as they're asked; they keep their own setting.
    if d.printer_two_sided() == Some(false) {
        params.push(note(
            "note.duplex",
            tl!("This printer's driver doesn't take two-sided settings from other programs, so its own setting applies. Change it in Properties…"),
        ));
    }
    if let Some(e) = &d.driver_error {
        params.push(note("note.error", crate::i18n::fmt(tl!("Couldn't open the printer's preferences: {e}"), &[("e", e)])));
    }
    // The spooler answered with no printer at all (#160): say so, rather than leaving Save as PDF looking like the only
    // thing PdfCraft can do.
    if no_printers {
        params.push(note(
            "note.no_printers",
            tl!("No printers were found. Check that the printer is installed and the print spooler is running, then open Print again."),
        ));
    }
    PrintSection::new(lbl(tl!("Paper")), params).untitled()
}

/// The paper the custom row has typed, when the paper list would take it.
pub(super) fn typed_custom_paper(d: &PrintDraft, units: Unit) -> Option<CustomPaper> {
    let (name, width, height) = d.custom_new.as_ref()?;
    let side = |s: &str| s.trim().replace(',', ".").parse::<f64>().ok().map(|v| v / units.per_point());
    CustomPaper::new(name, side(width)?, side(height)?)
}

/// Printer properties: the driver's options grouped as the driver groups them, as their own section while Properties…
/// is open. `None` while the driver's options have not been asked for.
fn driver_section(d: &PrintDraft) -> Option<PrintSection> {
    let title = lbl(tl!("Printer properties"));
    let loading = d.driver_options.is_none() && d.driver_options_loading.is_some();
    let section = |params: Vec<Param>| {
        PrintSection::new(title.clone(), params).button("driver.done", lbl(tl!("Done")), true).button(
            "driver.reset",
            lbl(tl!("Reset to printer defaults")),
            !d.driver_choices.is_empty(),
        )
    };
    if loading {
        return Some(section(vec![note("note.driver_loading", tl!("Reading the printer's settings…"))]));
    }
    let (_, driver) = d.driver_options.as_ref()?;
    if driver.is_empty() {
        return Some(section(vec![note("note.driver_none", tl!("This printer offers no settings of its own."))]));
    }
    let mut params = Vec::new();
    for chunk in driver.chunk_by(|a, b| a.group == b.group) {
        let group = chunk.first().map_or("", |o| o.group.as_str());
        if !group.is_empty() {
            // Option and group names are the driver's own text.
            params.push(Param::section(lbl(group)));
        }
        for o in chunk {
            let current = d.driver_choices.get(&o.key).unwrap_or(&o.default);
            let opts = o.choices.iter().map(|(k, l)| (k.clone(), lbl(l))).collect();
            params.push(choice(&driver_key(&o.key), lbl(&o.label), opts, current));
        }
    }
    Some(section(params))
}

/// The field key of a driver option.
fn driver_key(option: &str) -> String {
    format!("driver.{option}")
}

fn pages_section(d: &PrintDraft, count: usize, units: Unit, has_area: bool) -> PrintSection {
    let mut which = vec![(Which::All, lbl(tl!("All"))), (Which::Current, lbl(tl!("Current page"))), (Which::Range, lbl(tl!("Pages")))];
    // Offered only when an area was chosen with Snapshot (its menu's Print area…).
    if let (true, Some((page, a))) = (has_area, d.area) {
        let text = crate::i18n::fmt(
            tl!("Selected area (page {page}, {size})"),
            &[("page", &(page + 1).to_string()), ("size", &units.size(a[2] - a[0], a[3] - a[1]))],
        );
        which.push((Which::Area, lbl(text)));
    }
    // Offered only when pages were picked before the dialog opened.
    if !d.selected.is_empty() {
        which.push((Which::Selected, lbl(format!("{} ({})", tl!("Selected pages"), d.selected.len()))));
    }
    let subset = options(
        &SUBSET,
        vec![(Subset::All, lbl(tl!("All pages in range"))), (Subset::Odd, lbl(tl!("Odd pages only"))), (Subset::Even, lbl(tl!("Even pages only")))],
    );
    PrintSection::new(
        lbl(tl!("Pages to Print")),
        vec![
            radio("which", lbl(tl!("Pages to Print")), options(&WHICH, which), id_of(&WHICH, d.which)),
            // Typed text, not a from-to range: it reads page labels ("iii", "1-4").
            Param::new("range", lbl(tl!("Pages")), ParamKind::text(d.range.clone())).hint(lbl(format!("1-{count}"))).enabled_when("which", "range"),
            choice("subset", row_label(tl!("More options:")), subset, id_of(&SUBSET, d.subset)),
            check("reverse", lbl(tl!("Reverse pages")), d.reverse),
        ],
    )
}

fn sizing_section(d: &PrintDraft) -> PrintSection {
    let on = |tab: &'static str| move |p: Param| p.visible_when("handling", tab);
    let (size, poster, multiple, booklet, label) = (on("size"), on("poster"), on("multiple"), on("booklet"), on("label"));
    let mut params = vec![handling_tabs(d)];

    let sizing = vec![
        ("fit".to_owned(), lbl(tl!("Fit"))),
        ("actual".to_owned(), lbl(tl!("Actual size"))),
        ("shrink".to_owned(), lbl(tl!("Shrink oversized pages"))),
        ("custom".to_owned(), row_label(tl!("Custom scale:"))),
    ];
    params.push(size(radio("size", lbl(tl!("Size")), sizing, print::names::size_mode(d.size))));
    let scale = (*CUSTOM_SCALE.start(), *CUSTOM_SCALE.end());
    params.push(size(
        number("custom_scale", lbl(tl!("Custom scale")), scale, d.custom_scale, 1.0, 0, UnitKind::Percent).enabled_when("size", "custom"),
    ));

    params.push(label(note(
        "note.label",
        tl!("Cuts each page to its printed area, turns it upright and fits it to the sheet without stretching. One label per sheet."),
    )));
    params.push(label(check("label_flip", lbl(tl!("Turn upside down (180°)")), d.label_flip)));

    params.push(poster(number("poster_scale", row_label(tl!("Tile scale:")), (10.0, 1000.0), d.poster_scale, 1.0, 0, UnitKind::Percent)));
    params.push(poster(number("overlap", row_label(tl!("Overlap:")), (0.0, 144.0), d.overlap, 1.0, 0, UnitKind::Points)));
    params.push(poster(check("cut_marks", lbl(tl!("Cut marks")), d.cut_marks)));

    let mut per_sheet: Vec<(String, Label)> = PER_SHEET.iter().map(|n| (n.to_string(), lbl(n.to_string()))).collect();
    per_sheet.push((CUSTOM_GRID.to_owned(), lbl(tl!("Custom"))));
    let per = if d.custom_grid { CUSTOM_GRID.to_owned() } else { d.per_sheet.to_string() };
    params.push(multiple(choice("per_sheet", row_label(tl!("Pages per sheet:")), per_sheet, &per)));
    let grid = (1.0, print::MAX_GRID as f64);
    params.push(multiple(number("cols", lbl(tl!("Columns")), grid, d.cols as f64, 1.0, 0, UnitKind::Columns)).visible_when("per_sheet", CUSTOM_GRID));
    params.push(multiple(number("rows", lbl(tl!("Rows")), grid, d.rows as f64, 1.0, 0, UnitKind::Columns)).visible_when("per_sheet", CUSTOM_GRID));
    let order = options(
        &ORDER,
        vec![
            (PageOrder::Horizontal, lbl(tl!("Horizontal"))),
            (PageOrder::HorizontalReversed, lbl(tl!("Horizontal reversed"))),
            (PageOrder::Vertical, lbl(tl!("Vertical"))),
            (PageOrder::VerticalReversed, lbl(tl!("Vertical reversed"))),
            (PageOrder::CutStack, lbl(tl!("Cut and stack"))),
        ],
    );
    params.push(multiple(choice("order", row_label(tl!("Page order:")), order, id_of(&ORDER, d.order))));
    params.push(multiple(check("border", lbl(tl!("Print page border")), d.border)));
    params.push(multiple(check("auto_rotate", lbl(tl!("Auto-rotate pages")), d.auto_rotate)));
    params.push(
        multiple(note(
            "note.cut_stack",
            tl!("Print single-sided. Keep the sheets in order, cut at the marks, then stack the piles left to right, top to bottom."),
        ))
        .visible_when("order", "cut_stack"),
    );

    let subset = options(
        &BOOKLET,
        vec![
            (BookletSubset::BothSides, lbl(tl!("Both sides"))),
            (BookletSubset::FrontOnly, lbl(tl!("Front side only"))),
            (BookletSubset::BackOnly, lbl(tl!("Back side only"))),
        ],
    );
    params.push(booklet(choice("booklet_subset", row_label(tl!("Booklet subset:")), subset, id_of(&BOOKLET, d.booklet_subset))));
    let binding = options(&BINDING, vec![(Binding::Left, lbl(tl!("Left"))), (Binding::Right, lbl(tl!("Right")))]);
    params.push(booklet(choice("binding", row_label(tl!("Binding:")), binding, id_of(&BINDING, d.binding))));
    params.push(booklet(
        number("creep", row_label(tl!("Creep:")), (0.0, print::MAX_CREEP), d.creep, 0.05, 2, UnitKind::Points).tooltip(lbl(tl!(
            "Moves each sheet's pages toward the fold by this much more than the sheet outside it, so the margins match once the booklet is folded and trimmed."
        ))),
    ));

    let orientation = options(
        &ORIENTATION,
        vec![
            (Orientation::Auto, lbl(tl!("Auto portrait/landscape"))),
            (Orientation::Portrait, lbl(tl!("Portrait"))),
            (Orientation::Landscape, lbl(tl!("Landscape"))),
        ],
    );
    params.push(radio("orientation", row_label(tl!("Orientation:")), orientation, id_of(&ORIENTATION, d.orientation)));
    PrintSection::new(lbl(tl!("Page Sizing & Handling")), params)
}

fn advanced_section(d: &PrintDraft) -> PrintSection {
    let dpi: Vec<(String, Label)> = IMAGE_DPI.iter().map(|v| ((*v as u32).to_string(), lbl(format!("{} dpi", *v as u32)))).collect();
    PrintSection::new(
        lbl(tl!("Advanced")),
        vec![
            check("as_image", lbl(tl!("Print as image")), d.as_image),
            choice("dpi", row_label(tl!("Resolution:")), dpi, &(d.dpi.round() as u32).to_string()).enabled_when("as_image", true),
            note(
                "note.image",
                tl!("Use it when a printer skips text, shading or transparency. The print is no longer searchable text; with Print in grayscale, the pictures are grey."),
            ),
        ],
    )
    .folded(d.as_image)
}

<<<<<<< HEAD
/// The values of every row the draft decides.
pub(super) fn fields_from_draft(d: &PrintDraft) -> DialogFields {
    let mut f = DialogFields::new();
    f.set("which", id_of(&WHICH, d.which));
    f.set("range", d.range.clone());
    f.set("subset", id_of(&SUBSET, d.subset));
    f.set("reverse", d.reverse);
    f.set("handling", id_of(&HANDLING, d.handling));
    f.set("size", print::names::size_mode(d.size));
    f.set("custom_scale", d.custom_scale);
    f.set("per_sheet", if d.custom_grid { CUSTOM_GRID.to_owned() } else { d.per_sheet.to_string() });
    f.set("cols", d.cols as f64);
    f.set("rows", d.rows as f64);
    f.set("order", id_of(&ORDER, d.order));
    f.set("border", d.border);
    f.set("auto_rotate", d.auto_rotate);
    f.set("booklet_subset", id_of(&BOOKLET, d.booklet_subset));
    f.set("binding", id_of(&BINDING, d.binding));
    f.set("creep", d.creep);
    f.set("poster_scale", d.poster_scale);
    f.set("overlap", d.overlap);
    f.set("cut_marks", d.cut_marks);
    f.set("label_flip", d.label_flip);
    f.set("orientation", id_of(&ORIENTATION, d.orientation));
    f.set("content", id_of(&CONTENT, d.content));
    f.set("as_image", d.as_image);
    f.set("dpi", (d.dpi.round() as u32).to_string());
    f.set("paper", d.paper_name());
    f.set("custom_open", d.custom_new.is_some());
    if let Some((name, width, height)) = &d.custom_new {
        let typed = |s: &str| s.trim().parse::<f64>().unwrap_or(0.0);
        f.set("custom_name", name.clone());
        f.set("custom_width", typed(width));
        f.set("custom_height", typed(height));
    }
    if let Some((_, options)) = &d.driver_options {
        for o in options {
            f.set(driver_key(&o.key), d.driver_choices.get(&o.key).unwrap_or(&o.default).clone());
        }
    }
    f
}

/// Applies the rows the user edited to the draft. `edited` is the keys an edit changed since the last pass: a row nobody
/// touched leaves the draft alone, so what the app sets on the draft between passes is not overwritten by the fields'
/// older values.
pub(super) fn draft_from_fields(d: &mut PrintDraft, f: &DialogFields, edited: &[String]) {
    let on = |key: &str| edited.iter().any(|e| e == key);
    let text = |key: &str| f.text(key);
    let num = |key: &str| f.number(key).filter(|n| n.is_finite());
    if on("which")
        && let Some(which) = text("which").and_then(|t| value_of(&WHICH, t))
    {
        // Leaving Label this way gives back the paper it replaced, as its tabs do.
        if which == Which::Area && d.which != Which::Area {
            d.set_handling(Handling::Size);
        }
        d.which = which;
    }
    if on("range")
        && let Some(range) = text("range")
    {
        d.range = range.to_owned();
        // Typing in the range picks it.
        d.which = Which::Range;
    }
    if on("subset")
        && let Some(v) = text("subset").and_then(|t| value_of(&SUBSET, t))
    {
        d.subset = v;
    }
    if on("reverse")
        && let Some(v) = f.bool("reverse")
    {
        d.reverse = v;
    }
    if on("handling")
        && let Some(h) = text("handling").and_then(handling_from_id)
        && h != d.handling
    {
        d.set_handling(h);
    }
    if on("paper")
        && let Some(p) = text("paper").and_then(|t| d.paper_index(t))
        && p != d.paper
    {
        let before = d.paper;
        d.paper = p;
        d.paper_overridden = true;
        // Label stock gets the label layout (cut, turned upright, fitted).
        if p == print::LABEL_PAPER && d.which != Which::Area {
            d.paper_before_label = Some(before);
            d.handling = Handling::Label;
            d.sheet = 0;
        }
    }
    if on("custom_scale")
        && let Some(v) = num("custom_scale")
    {
        d.custom_scale = v;
    }
    if on("size")
        && let Some(mode) = text("size").and_then(|t| print::names::parse_size_mode(t, d.custom_scale))
    {
        d.size = mode;
    }
    if on("per_sheet")
        && let Some(t) = text("per_sheet")
    {
        if t == CUSTOM_GRID {
            d.custom_grid = true;
        } else if let Ok(n) = t.parse::<usize>() {
            d.custom_grid = false;
            d.per_sheet = n;
        }
    }
    let grid = |v: f64| (v.round().max(1.0) as usize).min(print::MAX_GRID);
    if on("cols")
        && let Some(v) = num("cols")
    {
        d.cols = grid(v);
    }
    if on("rows")
        && let Some(v) = num("rows")
    {
        d.rows = grid(v);
    }
    if on("order")
        && let Some(v) = text("order").and_then(|t| value_of(&ORDER, t))
    {
        d.order = v;
    }
    if on("border")
        && let Some(v) = f.bool("border")
    {
        d.border = v;
    }
    if on("auto_rotate")
        && let Some(v) = f.bool("auto_rotate")
    {
        d.auto_rotate = v;
    }
    if on("booklet_subset")
        && let Some(v) = text("booklet_subset").and_then(|t| value_of(&BOOKLET, t))
    {
        d.booklet_subset = v;
    }
    if on("binding")
        && let Some(v) = text("binding").and_then(|t| value_of(&BINDING, t))
    {
        d.binding = v;
    }
    if on("creep")
        && let Some(v) = num("creep")
    {
        d.creep = v.clamp(0.0, print::MAX_CREEP);
    }
    if on("poster_scale")
        && let Some(v) = num("poster_scale")
    {
        d.poster_scale = v;
    }
    if on("overlap")
        && let Some(v) = num("overlap")
    {
        d.overlap = v;
    }
    if on("cut_marks")
        && let Some(v) = f.bool("cut_marks")
    {
        d.cut_marks = v;
    }
    if on("label_flip")
        && let Some(v) = f.bool("label_flip")
    {
        d.label_flip = v;
    }
    if on("orientation")
        && let Some(v) = text("orientation").and_then(|t| value_of(&ORIENTATION, t))
    {
        d.orientation = v;
    }
    if on("content")
        && let Some(v) = text("content").and_then(|t| value_of(&CONTENT, t))
    {
        d.content = v;
    }
    if on("as_image")
        && let Some(v) = f.bool("as_image")
    {
        d.as_image = v;
    }
    if on("dpi")
        && let Some(v) = text("dpi").and_then(|t| t.parse::<f64>().ok()).filter(|v| IMAGE_DPI.contains(v))
    {
        d.dpi = v;
    }
    if let Some((name, width, height)) = d.custom_new.as_mut() {
        let typed = |n: f64| if n > 0.0 { n.to_string() } else { String::new() };
        if on("custom_name")
            && let Some(t) = text("custom_name")
=======
/// A wide, searchable fleet list; filtering never changes the selected destination.
fn printer_picker(ui: &mut egui::Ui, d: &mut PrintDraft) -> egui::Response {
    let filter_id = ui.make_persistent_id("printer-filter");
    // Keep the search only while the combo box renders its popup contents.
    let previous_filter = ui.data(|data| data.get_temp::<String>(filter_id));
    let opening = previous_filter.is_none();
    let mut filter = previous_filter.unwrap_or_default();
    let shown = match d.printer.as_deref() {
        Some(p) => printer_label(p),
        None if still_looking(d) => tl!("Looking for printers…").to_string(),
        None => tl!("Save as PDF").to_string(),
    };
    let screen = ui.ctx().content_rect();
    let width = 430.0_f32.min((screen.width() - 32.0).max(0.0));
    let height = 420.0_f32.min(screen.height() * 0.6);
    let response = craft_ui::ComboBox::from_id_salt("printer")
        .selected_text(&shown)
        .width(260.0)
        .truncate()
        .height(height)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            ui.set_width(width);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            let search_label = ui.label(tl!("Search printers"));
            let search = ui.add(widgets::line(&mut filter).desired_width(f32::INFINITY).char_limit(256)).labelled_by(search_label.id);
            if opening {
                search.request_focus();
            }
            let query = pdfcraft_render::text::fold_case(filter.trim());
            if d.printers_loading {
                ui.add_enabled(false, egui::Label::new(tl!("Looking for printers…")));
            }
            // Keep the search and Save as PDF visible while the devices scroll.
            // The popup's actual clip can be shorter than the requested height.
            let footer_height = ui.spacing().interact_size.y + 4.0 * ui.spacing().item_spacing.y;
            let list_height = (ui.clip_rect().bottom() - ui.cursor().top() - footer_height).clamp(0.0, height);
            let mut list = egui::ScrollArea::vertical().id_salt("printer-results").max_height(list_height);
            if search.changed() {
                list = list.vertical_scroll_offset(0.0);
            }
            list.show(ui, |ui| {
                let mut matched = false;
                for p in &d.printers {
                    let name = printer_label(&p.name);
                    if !query.is_empty() && !pdfcraft_render::text::fold_case(&name).contains(&query) {
                        continue;
                    }
                    matched = true;
                    let label = if p.default && p.name != crate::web_print::BROWSER_PRINTER {
                        crate::i18n::fmt(tl!("{name} (default)"), &[("name", &name)])
                    } else {
                        name
                    };
                    let selected = d.printer.as_deref() == Some(p.name.as_str());
                    let choice = ui.selectable_value(&mut d.printer, Some(p.name.clone()), label);
                    if opening && selected {
                        choice.scroll_to_me(Some(egui::Align::Center));
                    }
                    if choice.clicked() {
                        d.printer_picked = true;
                        ui.close();
                    }
                }
                if !matched && !query.is_empty() && !d.printers_loading {
                    ui.label(tl!("No printers match."));
                }
            });
            ui.separator();
            // An explicit choice also protects PDF from a late default-printer refresh.
            if ui.selectable_value(&mut d.printer, None, tl!("Save as PDF")).clicked() {
                d.printer_picked = true;
                ui.close();
            }
        });
    ui.data_mut(|data| {
        if response.inner.is_some() {
            data.insert_temp(filter_id, filter);
        } else {
            data.remove::<String>(filter_id);
        }
    });
    response.response.on_hover_text(shown)
}

/// Draw the dialog. `thumb` gives a page's thumbnail texture when there is one. Returns
/// (print, cancel).
pub(crate) fn body(
    ui: &mut egui::Ui,
    d: &mut PrintDraft,
    t: &Tokens,
    sizes: &[(f64, f64)],
    labels: &[String],
    thumb: &mut dyn FnMut(usize) -> Option<egui::TextureId>,
) -> (bool, bool) {
    ui.set_width(820.0);
    ui.label(egui::RichText::new(tl!("Print")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let settings = d.settings(sizes.len(), labels);
    let laid = settings.as_ref().ok().map(|s| print::layout(sizes, s));
    // Creep within its field's range can still be too much for a thick booklet.
    let too_much_creep = d.handling == Handling::Booklet && d.creep > 0.0 && matches!(laid, Some(Err(_)));
    let sheets = laid.and_then(Result::ok).unwrap_or_default();
    d.sheet = d.sheet.min(sheets.len().saturating_sub(1));
    // Cap the settings+preview area to the screen's available height (#4252, #5317):
    // on a short screen, or with the driver's own Properties… panel expanded below, this row can
    // otherwise grow taller than the display and push the Print/Cancel buttons off both edges.
    // The title and the button row stay outside, so they are always reachable.
    // `viewport_rect()` is egui's own idea of the current viewport, kept from the latest
    // `RawInput::screen_rect` it was handed — unlike reading `raw.screen_rect` directly, it
    // does not go blank on a frame where the backend didn't resend it.
    let screen_h = ui.ctx().input(|i| i.viewport_rect().height());
    let max_h = (screen_h - 220.0).max(300.0);
    // The viewport is `max_h` itself, not what the modal offers this frame: when a different
    // dialog was shown before, the shared modal remeasures from a height of zero (#5118) and
    // offers less, which would size this area by the dialog shown before. `auto_shrink` still
    // shrinks it to contents that fit.
    egui::ScrollArea::vertical().max_height(max_h).min_scrolled_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
    ui.horizontal_top(|ui| {
        // Settings.
        ui.vertical(|ui| {
            ui.set_width(470.0);
            egui::Grid::new("print-top").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                let printer_caption = ui.label(tl!("Printer:"));
                let before = d.printer.clone();
                // The browser's print dialog sets copies, two-sided printing and colour itself.
                let browser = d.printer.as_deref() == Some(crate::web_print::BROWSER_PRINTER);
                ui.horizontal(|ui| {
                    printer_picker(ui, d).labelled_by(printer_caption.id);
                    let tip = if spool::HAS_PRINTER_PREFERENCES {
                        tl!("Opens the printer driver's preferences. They are saved for every print from this computer; the copies, two-sided, grayscale and paper chosen here still apply.")
                    } else {
                        tl!("The printer driver's own settings for this print: paper tray, paper type, finishing and more.")
                    };
                    let properties = ui.add_enabled(d.printer.is_some() && !browser, egui::Button::new(tl!("Properties…"))).on_hover_text(tip);
                    if properties.clicked()
                        && let Some(p) = d.printer.clone()
                    {
                        if spool::HAS_PRINTER_PREFERENCES {
                            d.driver_error = spool::open_printer_preferences(&p).err().map(|e| e.to_string());
                        } else {
                            if d.driver_options.as_ref().is_none_or(|(read_for, _)| *read_for != p) && d.driver_options_loading.as_deref() != Some(p.as_str()) {
                                // Read off the GUI thread (#1818): `poll_print` starts it next frame.
                                d.driver_options = None;
                                d.driver_choices.clear();
                                d.driver_options_loading = Some(p.clone());
                                d.read_options_for = Some(p);
                            }
                            d.show_driver_options = !d.show_driver_options;
                        }
                    }
                });
                if d.printer != before {
                    d.printer_picked = true;
                    d.show_driver_options = false;
                    d.driver_error = None;
                    // What this printer was last printed with (#2001).
                    d.apply_setup();
                    d.reset_unoverridden_custom_paper();
                    d.apply_printer_papers(Vec::new());
                    if let Some(p) = d.printer.clone() {
                        d.read_papers_for = Some(p);
                    }
                }
                ui.end_row();
                // The spooler answered with no printer at all (#160): say so, rather than
                // leaving Save as PDF looking like the only thing PdfCraft can do.
                if !d.printers_loading && d.printers.is_empty() {
                    ui.label("");
                    ui.label(
                        egui::RichText::new(tl!("No printers were found. Check that the printer is installed and the print spooler is running, then open Print again."))
                            .small()
                            .color(t.text_muted),
                    );
                    ui.end_row();
                }
                let copies_label = ui.label(tl!("Copies:"));
                ui.add_enabled_ui(!browser, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut d.copies).range(1..=999)).labelled_by(copies_label.id);
                        ui.add(widgets::checkbox(&mut d.collate, tl!("Collate")));
                        ui.add(widgets::checkbox(&mut d.grayscale, tl!("Print in grayscale")));
                    });
                });
                ui.end_row();
                // A booklet decides two-sided itself (#2707), so the choice is hidden meanwhile, as
                // in Acrobat's Print dialog; it is kept for the other layouts.
                if d.handling != Handling::Booklet {
                    ui.label(tl!("Two-sided:"));
                    ui.add_enabled_ui(!browser, |ui| {
                        combo(
                            ui,
                            "duplex",
                            tl!("Two-sided:"),
                            &mut d.duplex,
                            &[
                                (None, tl!("Printer default")),
                                (Some(spool::Duplex::Off), tl!("Off")),
                                (Some(spool::Duplex::LongEdge), tl!("Flip on long edge")),
                                (Some(spool::Duplex::ShortEdge), tl!("Flip on short edge")),
                            ],
                            160.0,
                        );
                    });
                    ui.end_row();
                }
                let paper_label = ui.label(tl!("Paper:"));
                d.paper = d.paper.min(d.paper_count().saturating_sub(1));
                let before = d.paper;
                let mut paper_clicked = false;
                let detecting = d.papers_loading();
                ui.horizontal(|ui| {
                    // A standard size's name is translated; a custom paper keeps the name it was given.
                    let shown = if d.paper_is_custom() { d.paper_name().to_string() } else { tl!(&d.paper_name()).to_string() };
                    craft_ui::ComboBox::from_id_salt("paper").selected_text(shown).width(180.0).show_ui(ui, |ui| {
                        if detecting {
                            ui.add_enabled(false, egui::Label::new(tl!("Detecting paper…")));
                        }
                        for (i, (name, _)) in d.papers.iter().enumerate() {
                            if ui.selectable_value(&mut d.paper, i, tl!(name)).clicked() {
                                paper_clicked = true;
                            }
                        }
                        if !d.custom_papers.is_empty() {
                            ui.separator();
                        }
                        for (i, p) in d.custom_papers.iter().enumerate() {
                            let label = format!("{} ({})", p.name, d.units.size(p.width, p.height));
                            if ui.selectable_value(&mut d.paper, d.papers.len() + i, label).clicked() {
                                paper_clicked = true;
                            }
                        }
                    })
                    .response
                    .labelled_by(paper_label.id);
                    if detecting {
                        ui.label(egui::RichText::new(tl!("Detecting…")).small().color(t.text_muted));
                    }
                    if ui.button(tl!("Custom…")).on_hover_text(tl!("Add a paper size of your own to the list")).clicked() {
                        d.custom_new = match d.custom_new {
                            Some(_) => None,
                            None => Some((String::new(), String::new(), String::new())),
                        };
                    }
                    if d.paper_is_custom() && ui.button(tl!("Remove")).on_hover_text(tl!("Take this paper size off the list")).clicked() {
                        d.remove_custom_paper();
                        paper_clicked = true;
                    }
                    let page_setup_tip = if spool::HAS_PRINTER_PREFERENCES {
                        tl!("Selects paper size and orientation from the printer driver. For paper tray, print quality or other driver defaults, use Properties…")
                    } else {
                        tl!("Page setup dialog is not available on this system.")
                    };
                    let page_setup = ui
                        .add_enabled(d.printer.is_some() && !browser && spool::HAS_PRINTER_PREFERENCES, egui::Button::new(tl!("Page Setup…")))
                        .on_hover_text(page_setup_tip);
                    if page_setup.clicked()
                        && let Some(p) = d.printer.clone()
                    {
                        d.driver_error = None;
                        #[cfg(windows)]
                        let parent = winsafe::HWND::GetActiveWindow().map(|h| h.ptr() as isize);
                        #[cfg(not(windows))]
                        let parent = None;
                        match spool::show_printer_properties(&p, parent) {
                            Ok(Some((name, size))) => {
                                d.apply_driver_paper(name, size);
                            }
                            Ok(None) => {}
                            Err(e) => d.driver_error = Some(e.to_string()),
                        }
                    }
                });
                if paper_clicked || d.paper != before {
                    d.paper_overridden = true;
                }
                // Label stock gets the label layout (cut, turned upright, fitted).
                if paper_clicked && d.paper != before && d.paper == print::LABEL_PAPER && d.which != Which::Area {
                    d.paper_before_label = Some(before);
                    d.handling = Handling::Label;
                    d.sheet = 0;
                }
                ui.end_row();
                if d.custom_new.is_some() {
                    ui.label("");
                    custom_paper_row(ui, d, t);
                    ui.end_row();
                }
            });
            // Windows says which drivers can't print two-sided as they're asked; they keep their
            // own setting, even when Two-sided is Off, and for a booklet's both sides.
            if d.printer_two_sided() == Some(false) {
                ui.label(egui::RichText::new(tl!("This printer's driver doesn't take two-sided settings from other programs, so its own setting applies. Change it in Properties…")).small().color(t.text_muted));
            }
            if d.printer.as_deref() == Some(crate::web_print::BROWSER_PRINTER) {
                ui.label(egui::RichText::new(tl!("Copies, two-sided printing and colour are chosen in the browser's print dialog.")).small().color(t.text_muted));
            }
            if let Some(e) = &d.driver_error {
                ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Couldn't open the printer's preferences: {e}"), &[("e", e)])).small().color(t.text_muted));
            }
            if d.show_driver_options {
                if d.driver_options.is_none() && d.driver_options_loading.is_some() {
                    ui.label(egui::RichText::new(tl!("Reading the printer's settings…")).small().color(t.text_muted));
                } else {
                    driver_options_panel(ui, d, t);
                }
            }
            if let Some(&source) = sizes.get(d.current_page) {
                ui.horizontal_wrapped(|ui| {
                    ui.label(crate::i18n::fmt(tl!("PDF page: {size}"), &[("size", &d.units.size(source.0, source.1))]));
                    let matched = d.matching_paper(source);
                    if ui.add_enabled(matched.is_some(), egui::Button::new(tl!("Match page at 100%"))).clicked()
                        && let Some(paper) = matched
                    {
                        d.paper = paper;
                        d.paper_overridden = true;
                        // A paper picked outright: nothing for leaving Label to give back.
                        d.paper_before_label = None;
                        d.handling = Handling::Size;
                        d.size = SizeMode::Actual;
                        d.orientation = Orientation::Auto;
                        d.sheet = 0;
                    }
                });
            }
            ui.add_space(6.0);
            widgets::section_title(ui, tl!("Pages to Print"));
            ui.horizontal(|ui| {
                widgets::radio_value(ui, &mut d.which, Which::All, tl!("All"));
                widgets::radio_value(ui, &mut d.which, Which::Current, tl!("Current page"));
                let pages = widgets::radio_value(ui, &mut d.which, Which::Range, tl!("Pages"));
                let r = ui.add_enabled(
                    d.which == Which::Range,
                    crate::widgets::line(&mut d.range).hint_text(format!("1-{}", sizes.len())).desired_width(110.0),
                ).labelled_by(pages.id);
                if r.gained_focus() {
                    d.which = Which::Range;
                }
            });
            // Offered only when an area was chosen with Snapshot (its menu's Print area…).
            if let Some((page, a)) = d.area {
                let label = crate::i18n::fmt(
                    tl!("Selected area (page {page}, {size})"),
                    &[("page", &(page + 1).to_string()), ("size", &d.units.size(a[2] - a[0], a[3] - a[1]))],
                );
                if widgets::radio_value(ui, &mut d.which, Which::Area, label).clicked() {
                    // Leaving Label this way gives back the paper it replaced, as its tabs do.
                    d.set_handling(Handling::Size);
                }
            }
            // Offered only when pages were picked before the dialog opened.
            if !d.selected.is_empty() {
                let label = format!("{} ({})", tl!("Selected pages"), d.selected.len());
                widgets::radio_value(ui, &mut d.which, Which::Selected, label);
            }
            ui.horizontal(|ui| {
                ui.label(tl!("More options:"));
                combo(
                    ui,
                    "subset",
                    tl!("More options:"),
                    &mut d.subset,
                    &[(Subset::All, tl!("All pages in range")), (Subset::Odd, tl!("Odd pages only")), (Subset::Even, tl!("Even pages only"))],
                    150.0,
                );
                ui.add(widgets::checkbox(&mut d.reverse, tl!("Reverse pages")));
            });
            ui.add_space(6.0);
            widgets::section_title(ui, tl!("Page Sizing & Handling"));
            ui.horizontal_wrapped(|ui| {
                for (h, label) in [
                    (Handling::Size, tl!("Size")),
                    (Handling::Poster, tl!("Poster")),
                    (Handling::Multiple, tl!("Multiple")),
                    (Handling::Booklet, tl!("Booklet")),
                    (Handling::Label, tl!("Label")),
                ] {
                    // A selected area prints only with Size.
                    let offered = d.which != Which::Area || h == Handling::Size;
                    if ui.add_enabled_ui(offered, |ui| widgets::mode_tab(ui, label, d.handling == h)).inner.clicked() {
                        d.set_handling(h);
                    }
                }
            });
            ui.add_space(4.0);
            match d.handling {
                Handling::Size => {
                    ui.horizontal(|ui| {
                        widgets::radio_value(ui, &mut d.size, SizeMode::Fit, tl!("Fit"));
                        widgets::radio_value(ui, &mut d.size, SizeMode::Actual, tl!("Actual size"));
                        widgets::radio_value(ui, &mut d.size, SizeMode::Shrink, tl!("Shrink oversized pages"));
                    });
                    ui.horizontal(|ui| {
                        let custom = matches!(d.size, SizeMode::Custom(_));
                        if ui.add(widgets::radio(custom, tl!("Custom scale:"))).clicked() {
                            d.size = SizeMode::Custom(d.custom_scale);
                        }
                        ui.add_enabled(custom, egui::DragValue::new(&mut d.custom_scale).range(CUSTOM_SCALE).suffix(" %"))
                            .accessible_name(tl!("Custom scale"));
                    });
                }
                Handling::Label => {
                    ui.add(widgets::checkbox(&mut d.label_flip, tl!("Turn upside down (180°)")));
                    ui.label(
                        egui::RichText::new(tl!("Cuts each page to its printed area, turns it upright and fits it to the sheet without stretching. One label per sheet."))
                            .small()
                            .color(t.text_muted),
                    );
                }
                Handling::Poster => {
                    ui.horizontal(|ui| {
                        let scale_label = ui.label(tl!("Tile scale:"));
                        ui.add(egui::DragValue::new(&mut d.poster_scale).range(10.0..=1000.0).suffix(" %")).labelled_by(scale_label.id);
                        let overlap_label = ui.label(tl!("Overlap:"));
                        ui.add(egui::DragValue::new(&mut d.overlap).range(0.0..=144.0).suffix(" pt")).labelled_by(overlap_label.id);
                        ui.add(widgets::checkbox(&mut d.cut_marks, tl!("Cut marks")));
                    });
                }
                Handling::Multiple => {
                    let page_order = |ui: &mut egui::Ui, order: &mut PageOrder| {
                        ui.label(tl!("Page order:"));
                        combo(
                            ui,
                            "order",
                            tl!("Page order:"),
                            order,
                            &[
                                (PageOrder::Horizontal, tl!("Horizontal")),
                                (PageOrder::HorizontalReversed, tl!("Horizontal reversed")),
                                (PageOrder::Vertical, tl!("Vertical")),
                                (PageOrder::VerticalReversed, tl!("Vertical reversed")),
                                (PageOrder::CutStack, tl!("Cut and stack")),
                            ],
                            150.0,
                        );
                    };
                    ui.horizontal(|ui| {
                        ui.label(tl!("Pages per sheet:"));
                        // `None` is Custom: columns by rows, typed in.
                        let mut per_sheet = (!d.custom_grid).then_some(d.per_sheet);
                        combo(
                            ui,
                            "per-sheet",
                            tl!("Pages per sheet:"),
                            &mut per_sheet,
                            &[(Some(2), "2"), (Some(4), "4"), (Some(6), "6"), (Some(9), "9"), (Some(16), "16"), (None, tl!("Custom"))],
                            70.0,
                        );
                        d.custom_grid = per_sheet.is_none();
                        if let Some(n) = per_sheet {
                            d.per_sheet = n;
                            page_order(ui, &mut d.order);
                        } else {
                            let max = print::MAX_GRID;
                            ui.add(egui::DragValue::new(&mut d.cols).range(1..=max)).accessible_name(tl!("Columns"));
                            ui.label(tl!("by"));
                            ui.add(egui::DragValue::new(&mut d.rows).range(1..=max)).accessible_name(tl!("Rows"));
                        }
                    });
                    if d.custom_grid {
                        // The grid's fields take the room the page order had.
                        ui.horizontal(|ui| page_order(ui, &mut d.order));
                    }
                    ui.horizontal(|ui| {
                        ui.add(widgets::checkbox(&mut d.border, tl!("Print page border")));
                        ui.add(widgets::checkbox(&mut d.auto_rotate, tl!("Auto-rotate pages")));
                    });
                    if d.order == PageOrder::CutStack {
                        ui.label(tl!(
                            "Print single-sided. Keep the sheets in order, cut at the marks, then stack the piles left to right, top to bottom."
                        ));
                    }
                }
                Handling::Booklet => {
                    ui.horizontal(|ui| {
                        ui.label(tl!("Booklet subset:"));
                        combo(
                            ui,
                            "booklet",
                            tl!("Booklet subset:"),
                            &mut d.booklet_subset,
                            &[
                                (BookletSubset::BothSides, tl!("Both sides")),
                                (BookletSubset::FrontOnly, tl!("Front side only")),
                                (BookletSubset::BackOnly, tl!("Back side only")),
                            ],
                            130.0,
                        );
                        ui.label(tl!("Binding:"));
                        combo(ui, "binding", tl!("Binding:"), &mut d.binding, &[(Binding::Left, tl!("Left")), (Binding::Right, tl!("Right"))], 80.0);
                        let creep_label = ui.label(tl!("Creep:"));
                        ui.add(egui::DragValue::new(&mut d.creep).range(0.0..=print::MAX_CREEP).speed(0.05).suffix(" pt"))
                            .labelled_by(creep_label.id)
                            .on_hover_text(tl!("Moves each sheet's pages toward the fold by this much more than the sheet outside it, so the margins match once the booklet is folded and trimmed."));
                    });
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(tl!("Orientation:"));
                widgets::radio_value(ui, &mut d.orientation, Orientation::Auto, tl!("Auto portrait/landscape"));
                widgets::radio_value(ui, &mut d.orientation, Orientation::Portrait, tl!("Portrait"));
                widgets::radio_value(ui, &mut d.orientation, Orientation::Landscape, tl!("Landscape"));
            });
            ui.add_space(6.0);
            widgets::section_title(ui, tl!("Comments & Forms"));
            combo(
                ui,
                "content",
                tl!("Comments & Forms"),
                &mut d.content,
                &[
                    (Content::Document, tl!("Document")),
                    (Content::DocumentAndMarkups, tl!("Document and markups")),
                    (Content::DocumentAndStamps, tl!("Document and stamps")),
                    (Content::FormFieldsOnly, tl!("Form fields only")),
                ],
                220.0,
            );
            // Advanced (Acrobat's Advanced Print Setup): print as image. Folded unless it is on.
            ui.add_space(6.0);
            egui::CollapsingHeader::new(egui::RichText::new(crate::i18n::upper_label("Advanced")).font(theme::semibold(10.5)).color(t.text_faint))
                .id_salt("print-advanced")
                .default_open(d.as_image)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(widgets::checkbox(&mut d.as_image, tl!("Print as image")));
                        ui.add_enabled_ui(d.as_image, |ui| {
                            ui.label(tl!("Resolution:"));
                            let mut dpi = d.dpi.round() as u32;
                            combo(ui, "print-dpi", tl!("Resolution:"), &mut dpi, &[(150, "150 dpi"), (300, "300 dpi"), (600, "600 dpi")], 100.0);
                            d.dpi = f64::from(dpi);
                        });
                    });
                    ui.label(
                        egui::RichText::new(tl!(
                            "Use it when a printer skips text, shading or transparency. The print is no longer searchable text; with Print in grayscale, the pictures are grey."
                        ))
                        .small()
                        .color(t.text_muted),
                    );
                });
        });
        ui.add_space(12.0);
        // Preview.
        ui.vertical(|ui| {
            ui.set_width(PREVIEW_W);
            let (area, _) = ui.allocate_exact_size(vec2(PREVIEW_W, PREVIEW_H), egui::Sense::hover());
            ui.painter().rect_filled(area, 6.0, t.hover);
            match (&settings, sheets.get(d.sheet)) {
                (Err(e), _) => {
                    ui.put(area.shrink(16.0), egui::Label::new(egui::RichText::new(tl!(e)).color(t.text_muted)).wrap());
                }
                (Ok(_), None) if too_much_creep => {
                    let msg = tl!("The creep moves the innermost sheet's pages too far for this many pages. Reduce Creep.");
                    ui.put(area.shrink(16.0), egui::Label::new(egui::RichText::new(msg).color(t.text_muted)).wrap());
                }
                (Ok(_), Some(sheet)) => {
                    let k = (PREVIEW_BOX.0 / sheet.size.0 as f32).min(PREVIEW_BOX.1 / sheet.size.1 as f32);
                    let paper = Rect::from_center_size(area.center(), vec2(sheet.size.0 as f32 * k, sheet.size.1 as f32 * k));
                    ui.painter().rect_filled(paper, 0.0, Color32::WHITE);
                    ui.painter().rect_stroke(paper, 0.0, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
                    let to_screen = |x: f64, y: f64| -> Pos2 { pos2(paper.left() + x as f32 * k, paper.bottom() - y as f32 * k) };
                    let painter = ui.painter().with_clip_rect(paper);
                    for pl in &sheet.placed {
                        let (dw, dh) = sizes[pl.page];
                        let [x0, y0, x1, y1] = pl.clip;
                        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| {
                            let (sx, sy) = pl.matrix.apply(x, y);
                            to_screen(sx, sy)
                        });
                        // UVs of the visible part (texture y runs down from the page top).
                        let uv = |x: f64, y: f64| pos2((x / dw) as f32, (1.0 - y / dh) as f32);
                        let uvs = [uv(x0, y0), uv(x1, y0), uv(x1, y1), uv(x0, y1)];
                        match thumb(pl.page) {
                            Some(tex) => {
                                let mut mesh = egui::Mesh::with_texture(tex);
                                for (p, u) in corners.iter().zip(uvs) {
                                    mesh.vertices.push(egui::epaint::Vertex {
                                        pos: *p,
                                        uv: u,
                                        color: if d.grayscale { Color32::from_gray(235) } else { Color32::WHITE },
                                    });
                                }
                                mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
                                painter.add(egui::Shape::mesh(mesh));
                            }
                            None => {
                                painter.add(egui::Shape::convex_polygon(
                                    corners.to_vec(),
                                    Color32::from_gray(245),
                                    Stroke::new(0.5, Color32::from_gray(180)),
                                ));
                                let c = corners.iter().fold(vec2(0.0, 0.0), |a, p| a + p.to_vec2()) / 4.0;
                                painter.text(
                                    c.to_pos2(),
                                    egui::Align2::CENTER_CENTER,
                                    (pl.page + 1).to_string(),
                                    theme::regular(11.0),
                                    Color32::from_gray(120),
                                );
                            }
                        }
                    }
                    for b in &sheet.borders {
                        painter.rect_stroke(
                            Rect::from_two_pos(to_screen(b[0], b[1]), to_screen(b[2], b[3])),
                            0.0,
                            Stroke::new(0.6, Color32::BLACK),
                            egui::StrokeKind::Middle,
                        );
                    }
                    for l in &sheet.lines {
                        painter.line_segment([to_screen(l[0], l[1]), to_screen(l[2], l[3])], Stroke::new(0.6, Color32::BLACK));
                    }
                }
                _ => {}
            }
            ui.horizontal(|ui| {
                let n = sheets.len();
                if ui.add_enabled(d.sheet > 0, egui::Button::new("‹")).on_hover_text(tl!("Previous sheet")).clicked() {
                    d.sheet -= 1;
                }
                ui.label(if n == 0 {
                    tl!("No sheets").to_string()
                } else {
                    crate::i18n::fmt(tl!("Sheet {s} of {n}"), &[("s", &(d.sheet + 1).to_string()), ("n", &n.to_string())])
                });
                if ui.add_enabled(d.sheet + 1 < n, egui::Button::new("›")).on_hover_text(tl!("Next sheet")).clicked() {
                    d.sheet += 1;
                }
            });
            if let Some(s) = sheets.get(d.sheet) {
                ui.label(egui::RichText::new(d.units.size(s.size.0, s.size.1)).small().color(t.text_muted));
                if d.handling == Handling::Size
                    && let Some(p) = s.placed.first()
                {
                    let scale = p.matrix.0[0].hypot(p.matrix.0[1]) * 100.0;
                    ui.label(crate::i18n::fmt(tl!("PDF scale: {scale}%"), &[("scale", &format!("{scale:.2}"))]));
                    ui.label(egui::RichText::new(tl!("Printer paper and scaling may differ. Check driver settings.")).small().color(t.text_muted));
                    if (scale - 100.0).abs() > 0.01 {
                        ui.label(egui::RichText::new(tl!("Drawing scale changes. Use Actual size for full-scale plans.")).small().color(t.text_muted));
                    }
                }
            }
        });
    });
    }); // end ScrollArea (#4252, #5317)
    ui.add_space(12.0);
    let (mut go, mut cancel) = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let looking = still_looking(d);
        let waiting_paper = d.papers_loading();
        let label = if d.printer.is_some() || looking { tl!("Print") } else { tl!("Save as PDF") };
        if ui
            .add_enabled_ui(settings.is_ok() && !sheets.is_empty() && !looking && !waiting_paper, |ui| widgets::pill_button(ui, label, true))
            .inner
            .clicked()
>>>>>>> upstream/main
        {
            *name = t.to_owned();
        }
        if on("custom_width")
            && let Some(v) = num("custom_width")
        {
            *width = typed(v);
        }
        if on("custom_height")
            && let Some(v) = num("custom_height")
        {
            *height = typed(v);
        }
    }
    // Only the choices away from the printer's defaults are kept, and sent with the job.
    if let Some((_, options)) = &d.driver_options {
        for o in options {
            let key = driver_key(&o.key);
            if !on(&key) {
                continue;
            }
            match text(&key) {
                Some(pick) if pick != o.default => {
                    d.driver_choices.insert(o.key.clone(), pick.to_owned());
                }
                Some(_) => {
                    d.driver_choices.remove(&o.key);
                }
                None => {}
            }
        }
    }
}
