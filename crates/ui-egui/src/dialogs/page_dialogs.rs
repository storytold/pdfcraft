//! The page dialogs: Split, Extract, Rotate, Number pages and Replace, each a [`HostedParamDialog`] the dialog host
//! owns. The rows are the dialog's values; the footer button answers them and the pass applies the answer.

use std::sync::Arc;

use craft::pages::ui::{PageRangeChoices, PageRangeOptions};
use craft::pages::{PageRange, PageScope, Subset};
use craft::theme::variant::Tone;
use craft::ui::contract::{Bind, Entry, Outcome as _, Show as _, ShowContainer as _};
use craft::ui::layout_components::Stack;
use craft::ui::pickers::Select;
use craft::ui::dialog::{DialogResult, dialog_id};
use craft::ui::display::Text;
use craft::ui::patterns::form_dialogs::{DialogFields, FieldValue, HostedParamDialog, Param, ParamDialog, ParamKind, ParamResult};
use craft::ui::runtime::env::Label;
use craft::ui::runtime::memory::SaveKey;
use pdfcraft_engine::{Edit, LabelStyle, PageOrientation, PageParity};

use crate::i18n::fmt;
use crate::workflows::files::{ExtractOptions, PickTarget, SplitPlan, pages_to_extract, strip_pdf};
use crate::{PdfCraftApp, dialogs};

const SPLIT: &str = "page.split";
const EXTRACT: &str = "page.extract";
const ROTATE: &str = "page.rotate_dialog";
const NUMBER: &str = "page.number";
const REPLACE: &str = "page.replace";

/// `(id, label)` options of a radio or choice row.
fn options(items: &[(&str, &str)]) -> Vec<(String, Label)> {
    items.iter().map(|(id, label)| (id.to_string(), Label::from(label.to_string()))).collect()
}

/// What Split reads of the active document.
#[derive(Clone)]
struct SplitFacts {
    count: usize,
    /// The selected pages other than the first: a part cannot start before page 1.
    cuts: Vec<usize>,
    /// The first page of each top-level bookmark.
    marks: Vec<usize>,
}

impl SplitFacts {
    /// What Split does with the dialog's values.
    fn plan(&self, fields: &DialogFields) -> SplitPlan {
        match fields.text("mode") {
            Some("size") => SplitPlan::Size((fields.number("size").unwrap_or(2.0).max(0.0) * 1_048_576.0) as usize),
            Some("bookmarks") => SplitPlan::Bookmarks,
            Some("selection") => SplitPlan::By(pdfcraft_engine::SplitBy::Before(self.cuts.clone())),
            _ => SplitPlan::By(pdfcraft_engine::SplitBy::PageCount(fields.int("pages").map_or(1, |n| n.max(1) as usize))),
        }
    }

    /// How many files `plan` writes; `None` when it depends on the sizes of the parts.
    fn files(&self, plan: &SplitPlan) -> Option<usize> {
        match plan {
            SplitPlan::By(by) => Some(pdfcraft_engine::split_ranges(self.count, by).len()),
            SplitPlan::Bookmarks => Some(pdfcraft_engine::split_ranges(self.count, &pdfcraft_engine::SplitBy::Before(self.marks.clone())).len()),
            SplitPlan::Size(_) => None,
        }
    }

    fn dialog<'b>(&self, fields: &'b mut DialogFields) -> ParamDialog<'b> {
        // A remembered mode may name a choice this document does not offer.
        let mode = fields.text("mode").map(str::to_string);
        if (mode.as_deref() == Some("selection") && self.cuts.is_empty()) || (mode.as_deref() == Some("bookmarks") && self.marks.is_empty()) {
            fields.set("mode", "pages");
        }
        let plan = self.plan(fields);
        let files = self.files(&plan);
        let count = self.count.to_string();
        let note = match files {
            Some(1) => fmt(tl!("Creates 1 file from {n} pages."), &[("n", &count)]),
            Some(f) => fmt(tl!("Creates {f} files from {n} pages."), &[("f", &f.to_string()), ("n", &count)]),
            None => fmt(tl!("Each file holds as many of the {n} pages as fit."), &[("n", &count)]),
        };
        let bookmarks = fmt(tl!("Top-level bookmarks ({n})"), &[("n", &self.marks.len().to_string())]);
        let mut unavailable = Vec::new();
        if self.marks.is_empty() {
            unavailable.push("bookmarks");
        }
        if self.cuts.is_empty() {
            unavailable.push("selection");
        }
        let params = vec![
            Param::new(
                "mode",
                tl!("Split by"),
                ParamKind::radio(
                    options(&[
                        ("pages", tl!("Number of pages")),
                        ("size", tl!("File size")),
                        ("bookmarks", bookmarks.as_str()),
                        ("selection", tl!("Before each selected page (select pages in Organize)")),
                    ]),
                    Some("pages".into()),
                ),
            )
            .disabled_options(unavailable),
            Param::new("pages", tl!("Pages per file"), ParamKind::int(Some(1), Some(self.count.max(1) as i64), 1)).enabled_when("mode", "pages"),
            Param::new("size", tl!("At most (MB)"), ParamKind::number(Some(0.05), Some(2000.0), 2.0, 0.1, 2)).enabled_when("mode", "size"),
        ];
        ParamDialog::new(SPLIT, tl!("Split document"), params)
            .ok_label(tl!("Split").into())
            .ok_enabled(files.is_none_or(|f| f > 1))
            .footer_note(note)
            .remember(SaveKey::new("pdfcraft.page_split"))
            .values(Bind::bound(fields))
    }
}

/// What Extract reads of the active document.
#[derive(Clone)]
struct ExtractFacts {
    selected: Vec<usize>,
    count: usize,
}

impl ExtractFacts {
    fn dialog<'b>(&self, fields: &'b mut DialogFields) -> ParamDialog<'b> {
        let typed = fields.text("pages").unwrap_or_default().to_string();
        // The hint is the selection as a range, which an empty field takes.
        let hint = match crate::combine_ui::range_of(&self.selected, self.count) {
            all if all.is_empty() && self.count > 1 => format!("1-{}", self.count),
            all if all.is_empty() => "1".to_string(),
            some => some,
        };
        let chosen = pages_to_extract(&typed, self.selected.clone(), self.count);
        let message = match &chosen {
            Ok(pages) if pages.len() == 1 => tl!("1 page selected.").to_string(),
            Ok(pages) => fmt(tl!("{n} pages selected."), &[("n", &pages.len().to_string())]),
            Err(e) => e.clone(),
        };
        let bad = chosen.is_err();
        let params = vec![
            Param::new("pages", tl!("Pages"), ParamKind::text("")).hint(hint),
            Param::custom("pages-message", "", move |ui, _, _| {
                let text = Text::new(message.clone());
                if bad { text.tone(Tone::Bad).show(ui) } else { text.show(ui) }
            })
            .full_width(),
            Param::new("name", tl!("File name"), ParamKind::text("")),
            Param::new("delete", tl!("Delete pages after extracting"), ParamKind::bool(false)),
            Param::new("separate", tl!("Extract pages as separate files"), ParamKind::bool(false)),
        ];
        let dialog = ParamDialog::new(EXTRACT, tl!("Extract pages"), params).ok_label(tl!("Extract").into()).values(Bind::bound(fields));
        dialog.ok_enabled(chosen.is_ok())
    }
}

/// The rotation a Direction choice names.
fn degrees_of(fields: &DialogFields) -> i64 {
    match fields.text("direction") {
        Some("270") => 270,
        Some("180") => 180,
        _ => 90,
    }
}

fn orientation_of(fields: &DialogFields) -> PageOrientation {
    match fields.text("orientation") {
        Some("landscape") => PageOrientation::Landscape,
        Some("portrait") => PageOrientation::Portrait,
        _ => PageOrientation::Both,
    }
}

fn parity_of(subset: Subset) -> PageParity {
    match subset {
        Subset::All => PageParity::Both,
        Subset::Even => PageParity::Even,
        Subset::Odd => PageParity::Odd,
    }
}

fn rotate_dialog<'b>(fields: &'b mut DialogFields, count: usize, current: usize) -> ParamDialog<'b> {
    let valid = PageRangeOptions::value(fields, "pages").is_some();
    let params = vec![
        Param::new(
            "direction",
            tl!("Direction:"),
            ParamKind::choice(
                options(&[("90", tl!("Clockwise 90 degrees")), ("270", tl!("Counterclockwise 90 degrees")), ("180", tl!("180 degrees"))]),
                Some("90".into()),
            ),
        ),
        PageRangeOptions::param("pages", tl!("Pages:"), count, current, PageRangeChoices::SelectedFromTo),
        // Production's Rotate row: the subset list, then the orientation list, under one caption.
        Param::custom("orientation", tl!("Rotate:"), |ui, fields, _| {
            let shown = Stack::new().show(ui, |ui, _| {
                PageRangeOptions::subset_select(ui, fields, "pages");
                let entries = [
                    Entry::new("both".to_string(), tl!("Landscape and Portrait Pages")),
                    Entry::new("landscape".to_string(), tl!("Landscape Pages")),
                    Entry::new("portrait".to_string(), tl!("Portrait Pages")),
                ];
                let now = fields.text("orientation").unwrap_or("both").to_string();
                let picked = Select::new(tl!("Rotate:"), Bind::given(now), &entries).show(ui);
                if let Some(chosen) = picked.value().cloned() {
                    fields.set("orientation".to_string(), FieldValue::Text(chosen));
                }
            });
            shown.output
        })
        .opens_with(FieldValue::Text("both".into())),
    ];
    ParamDialog::new(ROTATE, tl!("Rotate Pages"), params).ok_enabled(valid).remember(SaveKey::new("pdfcraft.page_rotate")).values(Bind::bound(fields))
}

/// The label styles Number pages offers.
const STYLES: [(&str, LabelStyle, &str); 6] = [
    ("decimal", LabelStyle::Decimal, "1, 2, 3"),
    ("lower_roman", LabelStyle::LowerRoman, "i, ii, iii"),
    ("upper_roman", LabelStyle::UpperRoman, "I, II, III"),
    ("lower_alpha", LabelStyle::LowerAlpha, "a, b, c"),
    ("upper_alpha", LabelStyle::UpperAlpha, "A, B, C"),
    ("none", LabelStyle::None, "None (prefix only)"),
];

fn style_of(fields: &DialogFields) -> LabelStyle {
    let id = fields.text("style").unwrap_or("decimal");
    STYLES.iter().find(|(s, ..)| *s == id).map_or(LabelStyle::Decimal, |(_, style, _)| *style)
}

/// The 1-based `from` and `to` of a Number pages or Replace dialog, as the rows hold them.
fn span(fields: &DialogFields) -> (usize, usize) {
    let page = |key| fields.int(key).map_or(1, |n| n.max(1) as usize);
    (page("from"), page("to"))
}

fn number_dialog<'b>(fields: &'b mut DialogFields, count: usize) -> ParamDialog<'b> {
    let (from, to) = span(fields);
    let (style, prefix) = (style_of(fields), fields.text("prefix").unwrap_or_default().to_string());
    let start = fields.int("start").map_or(1, |n| n.max(1) as u32);
    let label = |k: u32| format!("{prefix}{}", style.format(k));
    // `to` can be below `from` while it is being typed.
    let preview = if to <= from {
        label(start)
    } else {
        let last = start.saturating_add(u32::try_from(to - from).unwrap_or(u32::MAX));
        format!("{}, {} … {}", label(start), label(start.saturating_add(1)), label(last))
    };
    let last = count.max(1) as i64;
    let params = vec![
        Param::new("from", tl!("Pages"), ParamKind::int(Some(1), Some(last), 1)),
        Param::new("to", fmt(tl!("to (of {n})"), &[("n", &count.to_string())]), ParamKind::int(Some(1), Some(last), last)),
        Param::new(
            "style",
            tl!("Style"),
            ParamKind::choice(STYLES.iter().map(|(id, _, label)| (id.to_string(), Label::from(label.to_string()))).collect(), Some("decimal".into())),
        ),
        Param::new("prefix", tl!("Prefix"), ParamKind::text("")),
        Param::new("start", tl!("Start"), ParamKind::int(Some(1), Some(99_999), 1)),
    ];
    ParamDialog::new(NUMBER, tl!("Number pages"), params)
        .ok_enabled(from <= to)
        .footer_note(fmt(tl!("Labels: {preview}. Later pages keep their labels."), &[("preview", &preview)]))
        .remember(SaveKey::new("pdfcraft.page_number"))
        .values(Bind::bound(fields))
}

/// The file and document a Replace dialog was opened for; the dialog's rows hold only the page numbers.
pub(crate) struct ReplaceSource {
    name: String,
    bytes: Arc<Vec<u8>>,
    src_pages: usize,
    count: usize,
    /// The selection when the dialog opened (0-based): an untouched range replaces these pages, so a gap between
    /// selected pages is left alone.
    pages: Vec<usize>,
    /// The range rows' opening values; rows that differ from them are an edit.
    opening: (usize, usize),
    /// The document whose pages are replaced, as it was when the dialog opened.
    target: Option<PickTarget>,
}

impl ReplaceSource {
    /// The pages to replace (0-based).
    fn pages(&self, fields: &DialogFields) -> Vec<usize> {
        let (from, to) = span(fields);
        if (from, to) == self.opening { self.pages.clone() } else { (from.saturating_sub(1)..to).collect() }
    }

    fn dialog<'b>(&self, fields: &'b mut DialogFields) -> ParamDialog<'b> {
        let n = self.pages(fields).len().max(1);
        let max_start = (self.src_pages.saturating_sub(n) + 1).max(1);
        let src_from = fields.int("src_from").map_or(1, |s| s.max(1) as usize).min(max_start);
        fields.set("src_from", src_from as i64);
        let (from, to) = span(fields);
        let fits = n <= self.src_pages && from <= to;
        let count = self.count.max(1) as i64;
        let params = vec![
            Param::new("from", tl!("Original: replace pages"), ParamKind::int(Some(1), Some(count), 1)),
            Param::new("to", tl!("to"), ParamKind::int(Some(1), Some(count), count)),
            Param::new(
                "src_from",
                fmt(tl!("Replacement: pages of {name}"), &[("name", &self.name)]),
                ParamKind::int(Some(1), Some(max_start as i64), 1),
            ),
            Param::read_only_text(
                "totals",
                "",
                fmt(
                    tl!("Original: {n} pages. Replacement: pages {a} to {b} of {m}."),
                    &[
                        ("n", &count.to_string()),
                        ("a", &src_from.to_string()),
                        ("b", &(src_from + n.saturating_sub(1)).to_string()),
                        ("m", &self.src_pages.to_string()),
                    ],
                ),
            )
            .full_width(),
        ];
        let note = if fits {
            tl!("Only the page content changes: links, comments, form fields and bookmarks on the original pages stay.")
        } else {
            tl!("The replacement file doesn't have that many pages.")
        };
        ParamDialog::new(REPLACE, tl!("Replace Pages"), params).ok_enabled(fits).footer_note(note).values(Bind::bound(fields))
    }
}

impl PdfCraftApp {
    /// Opens the dialog `key`; `seeds` are this open's facts, which win over the values the dialog remembers.
    fn open_page_dialog(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        seeds: Vec<(&'static str, FieldValue)>,
        build: impl for<'b> Fn(&'b mut DialogFields) -> ParamDialog<'b> + Send + 'static,
    ) {
        let mut dialog = HostedParamDialog::new(ctx, key, DialogFields::new(), build);
        for (field, value) in seeds {
            dialog.set_field(field, value);
        }
        dialogs::host::open(self, dialog);
    }

    fn split_facts(&self) -> Option<SplitFacts> {
        let (i, id) = self.active_ids()?;
        let count = self.session.get(id)?.info.pages.len();
        let cuts = self.views[i].selected.items.iter().copied().filter(|p| *p > 0).collect();
        let marks = self.session.bookmark_splits(id).into_iter().map(|m| m.0).collect();
        Some(SplitFacts { count, cuts, marks })
    }

    /// Show Split; `before_selected` opens it on the cuts before the selected pages when there are any.
    pub fn open_split(&mut self, ctx: &egui::Context, before_selected: bool) {
        let Some(facts) = self.split_facts() else { return };
        let mut seeds = Vec::new();
        if before_selected && !facts.cuts.is_empty() {
            seeds.push(("mode", FieldValue::from("selection")));
        }
        self.open_page_dialog(ctx, SPLIT, seeds, move |fields| facts.dialog(fields));
    }

    /// Show Extract pages, its file name the document's (#737) and no page ranges typed, so it takes the selected
    /// pages until some are (#3593).
    pub fn open_extract(&mut self, ctx: &egui::Context) {
        let Some((i, id)) = self.active_ids() else { return };
        let name = self.session.get(id).map(|d| strip_pdf(&d.name).to_string()).unwrap_or_default();
        let facts = ExtractFacts { selected: self.views[i].target_pages(), count: self.views[i].page_count() };
        self.open_page_dialog(ctx, EXTRACT, vec![("name", FieldValue::from(name))], move |fields| facts.dialog(fields));
    }

    /// Show Rotate Pages, on the selected pages when there are any, else on all.
    pub fn open_rotate(&mut self, ctx: &egui::Context) {
        let Some((i, id)) = self.active_ids() else { return };
        let count = self.session.get(id).map_or(1, |d| d.info.pages.len());
        let current = self.views[i].current();
        let scope = if self.views[i].selected.items.is_empty() { PageScope::All } else { PageScope::Selection };
        let range = PageRangeOptions::field_value(&PageRange { scope, ..PageRange::default() });
        self.open_page_dialog(ctx, ROTATE, vec![("pages", range)], move |fields| rotate_dialog(fields, count, current));
    }

    /// Show Number pages over the selected pages (or the current one).
    pub fn open_number_pages(&mut self, ctx: &egui::Context) {
        let Some((i, id)) = self.active_ids() else { return };
        let count = self.session.get(id).map_or(1, |d| d.info.pages.len());
        let v = &self.views[i];
        let pages: Vec<usize> = if v.selected.items.is_empty() { vec![v.current()] } else { v.selected.items.clone() };
        let (lo, hi) = (pages.iter().min().copied().unwrap_or(0), pages.iter().max().copied().unwrap_or(0));
        let seeds = vec![("from", FieldValue::from((lo + 1) as i64)), ("to", FieldValue::from((hi + 1) as i64))];
        self.open_page_dialog(ctx, NUMBER, seeds, move |fields| number_dialog(fields, count));
    }

    /// Open the Replace Pages dialog for `bytes`, replacing the selection (or the current page).
    pub fn start_replace(&mut self, ctx: &egui::Context, name: String, bytes: Vec<u8>) {
        let Some((i, id)) = self.active_ids() else { return };
        let bytes = Arc::new(bytes);
        let src_pages = match self.session.replace_source(id, &name, &bytes) {
            Ok(n) => n,
            Err(e) => {
                self.notify_fmt("Couldn't use {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                return;
            }
        };
        let count = self.session.get(id).map_or(1, |d| d.info.pages.len());
        let pages = self.views[i].target_pages();
        let opening = (pages.first().map_or(1, |p| p + 1), pages.last().map_or(1, |p| p + 1));
        let target = self.pick_target(Some(id));
        let source = Arc::new(ReplaceSource { name, bytes, src_pages, count, pages, opening, target });
        let seeds =
            vec![("from", FieldValue::from(opening.0 as i64)), ("to", FieldValue::from(opening.1 as i64)), ("src_from", FieldValue::from(1i64))];
        self.replace_source = Some(source.clone());
        self.open_page_dialog(ctx, REPLACE, seeds, move |fields| source.dialog(fields));
    }

    /// The answer of the dialog `key` once it finished with OK; Cancel and a dialog still open give `None`.
    fn answered(&mut self, ctx: &egui::Context, key: &str) -> Option<ParamResult> {
        match self.dialogs.take_by_id::<HostedParamDialog>(dialog_id(key))? {
            DialogResult::Ok(result) => {
                result.remember(ctx);
                Some(result)
            }
            _ => None,
        }
    }

    /// Applies the page dialogs that finished this pass; called by the dialog host's pass.
    pub(crate) fn page_dialogs_pass(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.answered(ctx, SPLIT)
            && let Some(facts) = self.split_facts()
        {
            let plan = facts.plan(&result.fields);
            self.split_active(ctx, &plan);
        }
        if let Some(result) = self.answered(ctx, EXTRACT) {
            let f = &result.fields;
            self.extract_selection(
                ctx,
                ExtractOptions {
                    separate: f.bool("separate").unwrap_or(false),
                    delete: f.bool("delete").unwrap_or(false),
                    name: f.text("name").unwrap_or_default().to_string(),
                    pages: f.text("pages").unwrap_or_default().to_string(),
                },
            );
        }
        if let Some(result) = self.answered(ctx, ROTATE)
            && let Some(range) = PageRangeOptions::value(&result.fields, "pages")
            && let Some((i, id)) = self.active_ids()
        {
            let count = self.session.get(id).map_or(0, |d| d.info.pages.len());
            let base = range.pages(count, self.views[i].current(), &self.views[i].target_pages());

            self.rotate_pages(ctx, degrees_of(&result.fields), base, parity_of(range.subset), orientation_of(&result.fields));
        }
        if let Some(result) = self.answered(ctx, NUMBER) {
            let (from, to) = span(&result.fields);
            self.apply_edit(
                ctx,
                Edit::NumberPages {
                    from: from - 1,
                    to: to.max(from) - 1,
                    style: style_of(&result.fields),
                    prefix: result.fields.text("prefix").unwrap_or_default().to_string(),
                    first: result.fields.int("start").map_or(1, |n| n.max(1) as u32),
                },
            );
        }
        if let Some(done) = self.dialogs.take_by_id::<HostedParamDialog>(dialog_id(REPLACE)) {
            let source = self.replace_source.take();
            if let (DialogResult::Ok(result), Some(source)) = (done, source)
                // The pages belong to the document the dialog was opened on (#167).
                && self.still_pick_target(source.target)
            {
                let pages = source.pages(&result.fields);
                let src_from = result.fields.int("src_from").map_or(1, |s| s.max(1) as usize);
                let n = pages.len();
                self.apply_edit(
                    ctx,
                    Edit::ReplacePages {
                        pages,
                        name: source.name.clone(),
                        bytes: source.bytes.clone(),
                        src_pages: (src_from - 1..src_from - 1 + n).collect(),
                    },
                );
            }
        }
    }
}
