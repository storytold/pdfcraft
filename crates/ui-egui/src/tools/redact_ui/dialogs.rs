//! The Redact dialogs: Redaction Tool Properties, Mark Page Range, Find text and redact, the
//! Apply and Sanitize prompts and Remove hidden information.

use craft::pages::ui::range::{PageRangeChoices, PageRangeOptions};
use craft::pages::{PageRange, PageScope};
use craft::theme::Srgba;
use craft::ui::buttons::Button;
use craft::ui::choice::{Checkbox, Radio, RadioGroup};
use craft::ui::contract::{Bind, Entry, Props as _, Show as _, ShowContainer as _};
use craft::ui::layout_components::Row;
use craft::ui::dialog::{PromptChoice, StandardPrompt};
use craft::ui::display::{Shade, Text};
use craft::ui::form::{Field, FieldGroup};
use craft::ui::patterns::form_dialogs::{DialogFields, FormDialog, HostedParamDialog, ParamDialog};
use craft::ui::pickers::Select;
use craft::ui::runtime::env::UnitKind;
use craft::ui::runtime::requests::ColorRequest;
use craft::ui::text::{NumberField, TextField};
use egui::Ui;
use pdfcraft_engine::{Edit, OverlayFont, REDACT_PATTERNS, REDACTION_CODE_SETS, RedactPattern, Rgb};

use super::{HiddenDraft, RedactPrefs, SearchDraft, SearchMode};
use crate::comments::colours::{self, RecentColours};
use crate::{FilePurpose, PdfCraftApp};

const FILL_KEY: &str = "redact.fill";
const FONT_KEY: &str = "redact.font";
const OUTLINE_KEY: &str = "redact.outline";
const HOVER_KEY: &str = "redact.hover";

/// A labelled colour row: `palette` over `current`, `Some` when the user picked a colour. An "Other colour…" request
/// goes to `request`, which the dialog's owner answers with the app's picker.
fn colour_row(
    ui: &mut Ui,
    label: &'static str,
    current: Rgb,
    key: &'static str,
    enabled: bool,
    recent: &RecentColours,
    request: &mut Option<ColorRequest>,
) -> Option<Rgb> {
    let mut picked = None;
    Field::new(label).enabled(enabled).show(ui, |ui, _| {
        let pick = colours::palette(ui, label, Some(current), key, recent);
        picked = pick.picked;
        if pick.request.is_some() {
            *request = pick.request;
        }
    });
    picked
}

fn props_rows(ui: &mut Ui, d: &mut RedactPrefs, recent: &RecentColours, request: &mut Option<ColorRequest>) {
    FieldGroup::new(tl!("Redacted area")).show(ui, |ui, _| {
        let choice = colours::fill_choice(ui, d.fill, FILL_KEY, recent);
        if let Some(fill) = choice.choice {
            d.fill = fill;
        }
        if choice.request.is_some() {
            *request = choice.request;
        }
        Checkbox::new(Bind::bound(&mut d.use_overlay)).label(tl!("Use overlay text")).show(ui);
        let on = d.use_overlay;
        RadioGroup::new(tl!("Overlay text source"), Bind::bound(&mut d.use_code)).show(ui, |ui, _| {
            Radio::new(false).label(tl!("Custom text:")).enabled(on).show(ui);
            Radio::new(true).label(tl!("Redaction code:")).enabled(on).show(ui);
        });
        let codes_on = on && d.use_code;
        Field::new(tl!("Custom text")).enabled(on && !d.use_code).show(ui, |ui, _| {
            TextField::new(tl!("Custom text"), Bind::bound(&mut d.overlay)).show(ui);
        });
        let sets: Vec<Entry<_>> = REDACTION_CODE_SETS.iter().map(|s| Entry::new(*s, s.name)).collect();
        Field::new(tl!("Code set")).enabled(codes_on).show(ui, |ui, _| {
            if let Some(set) = Select::new(tl!("Code set"), Bind::given(d.code_set), &sets).show(ui).value().copied()
                && set != d.code_set
            {
                d.code_set = set;
                d.codes.clear();
            }
        });
        let set = d.code_set;
        for &code in set.codes.iter() {
            let was = d.codes.contains(&code);
            let mut picked = was;
            Checkbox::new(Bind::bound(&mut picked)).label(code).enabled(codes_on).show(ui);
            if picked != was {
                d.codes.retain(|c| *c != code);
                if picked {
                    d.codes.push(code);
                }
            }
        }
        let fonts: Vec<Entry<OverlayFont>> = OverlayFont::ALL.iter().map(|f| Entry::new(*f, f.name())).collect();
        Field::new(tl!("Font:")).enabled(on).show(ui, |ui, _| {
            if let Some(font) = Select::new(tl!("Font:"), Bind::given(d.look.font), &fonts).show(ui).value().copied() {
                d.look.font = font;
            }
        });
        let was_auto = d.look.size <= 0.0;
        let mut auto = was_auto;
        Checkbox::new(Bind::bound(&mut auto)).label(tl!("Auto-size text to fit redaction region")).enabled(on).show(ui);
        if auto != was_auto {
            d.look.size = if auto { 0.0 } else { 10.0 };
        }
        Field::new(tl!("Font size:")).enabled(on && !auto).show(ui, |ui, _| {
            let field = NumberField::new(tl!("Font size:"), Bind::given(Some(d.look.size.max(2.0)))).range(2.0..=144.0).unit_kind(UnitKind::Points);
            if let Some(Some(size)) = field.show(ui).value().copied() {
                d.look.size = size.clamp(2.0, 144.0);
            }
        });
        if let Some(c) = colour_row(ui, tl!("Font colour:"), d.look.color, FONT_KEY, on, recent, request) {
            d.look.color = c;
        }
        Checkbox::new(Bind::bound(&mut d.look.repeat)).label(tl!("Repeat overlay text")).enabled(on).show(ui);
        Field::new(tl!("Text alignment:")).enabled(on).show(ui, |ui, _| {
            RadioGroup::new(tl!("Text alignment:"), Bind::bound(&mut d.look.align)).show(ui, |ui, _| {
                Radio::new(0u8).label(tl!("Left")).show(ui);
                Radio::new(1u8).label(tl!("Center")).show(ui);
                Radio::new(2u8).label(tl!("Right")).show(ui);
            });
        });
    });
    FieldGroup::new(tl!("Redaction mark appearance")).show(ui, |ui, _| {
        if let Some(c) = colour_row(ui, tl!("Outline colour:"), d.outline, OUTLINE_KEY, true, recent, request) {
            d.outline = c;
        }
        if let Some(c) = colour_row(ui, tl!("Fill colour:"), d.hover_fill, HOVER_KEY, true, recent, request) {
            d.hover_fill = c;
        }
        Text::new(tl!("Shown while the pointer is over a mark.")).shade(Shade::Muted).show(ui);
        Field::new(tl!("Opacity:")).show(ui, |ui, _| {
            let field = NumberField::new(tl!("Opacity:"), Bind::given(Some(d.opacity * 100.0)))
                .range(10.0..=100.0)
                .commit_decimals(0)
                .unit_kind(UnitKind::Percent);
            if let Some(Some(pct)) = field.show(ui).value().copied() {
                d.opacity = (pct / 100.0).clamp(0.1, 1.0);
            }
        });
        Checkbox::new(Bind::bound(&mut d.remember)).label(tl!("Make properties default")).show(ui);
    });
}

/// Redaction Tool Properties while `app.redact_props` holds the draft: OK keeps it for this run
/// (and as the saved defaults with "Make properties default"), Cancel and Escape drop it.
pub(crate) fn show_props(app: &mut PdfCraftApp, ui: &mut Ui) {
    let Some(mut draft) = app.redact_props.take() else { return };
    let mut open = true;
    let recent = app.recent_colours.clone();
    let mut request = None;
    let shown = FormDialog::new(tl!("Redaction Tool Properties"), Bind::bound(&mut draft)).opened(Bind::bound(&mut open)).show(ui, |ui, _, d| {
        props_rows(ui, d, &recent, &mut request);
        true
    });
    if let Some(request) = request {
        let key = request.key.clone();
        app.open_colour_picker(crate::colour_picker::PickerHost::App, request, move |app: &mut PdfCraftApp, _: &egui::Context, colour: Srgba| {
            let Some(draft) = app.redact_props.as_mut() else { return };
            let colour = colours::to_rgb(colour);
            match key.as_str() {
                FILL_KEY => draft.fill = Some(colour),
                FONT_KEY => draft.look.color = colour,
                OUTLINE_KEY => draft.outline = colour,
                HOVER_KEY => draft.hover_fill = colour,
                _ => {}
            }
        });
    }
    match (shown.accepted, shown.closed) {
        (Some(prefs), _) => {
            if prefs.remember {
                app.redact_defaults = prefs.clone();
            }
            app.redact_prefs = prefs;
        }
        (None, Some(_)) => {}
        (None, None) => app.redact_props = Some(draft),
    }
}

/// Opens Mark Page Range over the current page.
pub(crate) fn open_pages(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some((i, id)) = app.active_ids() else { return };
    let Some(doc) = app.session.get(id) else { return };
    let count = doc.info.pages.len();
    let current = app.views.get(i).map_or(0, |v| v.current());
    let range = PageRange { scope: PageScope::Current, ..PageRange::default() };
    let fields = DialogFields::from_pairs([("pages", PageRangeOptions::field_value(&range))]);
    let dialog = HostedParamDialog::new(ctx, "redact.pages", fields, move |fields| {
        let rows = vec![PageRangeOptions::param("pages", "", count, current, PageRangeChoices::CurrentFromTo)];
        ParamDialog::new("redact.pages", tl!("Mark Page Range"), rows).values(Bind::bound(fields))
    });
    crate::dialogs::host::open(app, dialog);
}

/// Mark Page Range was confirmed with `fields`: marks the pages the range names.
// PLAN: pdf-hub-w2 routes the "redact.pages" result here.
pub(crate) fn finish_pages(ctx: &egui::Context, app: &mut PdfCraftApp, fields: &DialogFields) {
    let Some(range) = PageRangeOptions::value(fields, "pages") else { return };
    let Some((i, id)) = app.active_ids() else { return };
    let Some(doc) = app.session.get(id) else { return };
    let count = doc.info.pages.len();
    let current = app.views.get(i).map_or(0, |v| v.current());
    let pages = range.pages(count, current, &[]);
    app.redact_pages(ctx, &pages);
}

fn search_rows(ui: &mut Ui, d: &mut SearchDraft, import: &mut bool) {
    let mode = d.mode;
    let patterns: Vec<Entry<RedactPattern>> = REDACT_PATTERNS.iter().map(|p| Entry::new(*p, tl!(p.label()))).collect();
    FieldGroup::new(tl!("Find text and redact")).show(ui, |ui, _| {
        RadioGroup::new(tl!("Search for"), Bind::bound(&mut d.mode)).show(ui, |ui, _| {
            Radio::new(SearchMode::Phrase).label(tl!("Single word or phrase")).show(ui);
            TextField::new(tl!("Text to find"), Bind::bound(&mut d.text))
                .placeholder(tl!("Text to find"))
                .enabled(mode == SearchMode::Phrase)
                .show(ui);
            Radio::new(SearchMode::Words).label(tl!("Multiple words or phrases")).show(ui);
            TextField::new(tl!("Words or phrases to redact"), Bind::bound(&mut d.words))
                .placeholder(tl!("One word or phrase per line"))
                .lines(4)
                .enabled(mode == SearchMode::Words)
                .show(ui);
            *import = Button::new(tl!("Import list…")).enabled(mode == SearchMode::Words).show(ui).clicked();
            let n = pdfcraft_engine::redact_word_list(&d.words).len();
            Text::new(crate::i18n::fmt(tl!("{n} word(s) or phrase(s)"), &[("n", &n.to_string())])).shade(Shade::Muted).show(ui);
            Radio::new(SearchMode::Patterns).label(tl!("Patterns")).show(ui);
            Field::new(tl!("Pattern")).enabled(mode == SearchMode::Patterns).show(ui, |ui, _| {
                if let Some(p) = Select::new(tl!("Pattern"), Bind::given(d.pattern), &patterns).show(ui).value().copied() {
                    d.pattern = p;
                }
            });
        });
        let phrase_or_words = mode != SearchMode::Patterns;
        Checkbox::new(Bind::bound(&mut d.whole_words)).label(tl!("Whole words only")).enabled(phrase_or_words).show(ui);
        Checkbox::new(Bind::bound(&mut d.case_sensitive)).label(tl!("Case-sensitive")).enabled(phrase_or_words).show(ui);
        Text::new(tl!("Text in images isn't found; recognise text (OCR) first.")).shade(Shade::Muted).show(ui);
        if let Some(n) = d.found {
            let line = if n == 0 {
                tl!("No matches.").to_string()
            } else {
                crate::i18n::fmt(tl!("{n} match(es) marked for redaction."), &[("n", &n.to_string())])
            };
            Text::new(line).show(ui);
        }
    });
}

/// Find text and redact while `app.redact_search_open`: OK marks every match and stays open to
/// show the count; Cancel and Escape close it.
pub(crate) fn show_search(app: &mut PdfCraftApp, ui: &mut Ui) {
    if !app.redact_search_open {
        return;
    }
    // The draft is the app's, written by the word-list import and read by the search; it is
    // lent to the form for the pass.
    let mut draft = std::mem::take(&mut app.redact_search);
    let mut open = true;
    let mut import = false;
    let shown = FormDialog::new(tl!("Find text and redact"), Bind::bound(&mut draft))
        .opened(Bind::bound(&mut open))
        .ok_label(tl!("Mark all"))
        .accept_stays_open(true)
        .show(ui, |ui, _, d| {
            search_rows(ui, d, &mut import);
            true
        });
    app.redact_search = draft;
    if shown.accepted.is_some() || shown.picked == Some(0) {
        let n = app.redact_search(ui.ctx());
        app.redact_search.found = Some(n);
    } else if shown.closed.is_some() {
        app.redact_search_open = false;
    }
    if import {
        app.pick_files(FilePurpose::RedactWords, false);
    }
}

/// Opens the Apply redactions prompt; `prompts_pass` routes a Proceed to `finish_apply`.
pub(crate) fn open_apply(app: &mut PdfCraftApp) {
    let marks = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(0, |d| d.redaction_marks());
    let message = if marks == 1 {
        tl!("You are about to apply 1 redaction mark. Text, images and drawings under the marks, and comments and form fields that overlap them, are removed permanently.").to_string()
    } else {
        crate::i18n::fmt(
            tl!(
                "You are about to apply {n} redaction marks. Text, images and drawings under the marks, and comments and form fields that overlap them, are removed permanently."
            ),
            &[("n", &marks.to_string())],
        )
    };
    let prompt = StandardPrompt::custom(message)
        .title(tl!("Apply redactions"))
        .paragraph(tl!("Save the document afterwards: saving rewrites the whole file so no trace of the removed content stays in it."))
        .button(PromptChoice::Proceed, tl!("Apply"))
        .button(PromptChoice::Cancel, tl!("Cancel"))
        .instance("redact-apply");
    app.prompts.apply_redactions = Some(crate::dialogs::host::open(app, prompt));
}

/// Opens the Sanitize prompt; `prompts_pass` routes a Proceed to `finish_sanitize`.
pub(crate) fn open_sanitize(app: &mut PdfCraftApp) {
    let prompt = StandardPrompt::custom(tl!("Sanitizing removes hidden information from the document: metadata, file attachments, comments, form fields (flattened), hidden text and layers, bookmarks, links, actions and scripts, and private application data."))
        .title(tl!("Sanitize document"))
        .paragraph(tl!("Save the document afterwards; saving rewrites the whole file so nothing removed stays in it."))
        .button(PromptChoice::Proceed, tl!("Sanitize"))
        .button(PromptChoice::Cancel, tl!("Cancel"))
        .instance("redact-sanitize");
    app.prompts.sanitize = Some(crate::dialogs::host::open(app, prompt));
}

/// Remove hidden information while `app.hidden_draft` holds the categories found.
pub(crate) fn show_hidden(app: &mut PdfCraftApp, ui: &mut Ui) {
    let Some(mut draft) = app.hidden_draft.take() else { return };
    let mut open = true;
    let removable = draft.found.iter().any(|f| f.2 && f.1 > 0);
    let shown = FormDialog::new(tl!("Remove hidden information"), Bind::bound(&mut draft))
        .opened(Bind::bound(&mut open))
        .ok_label(tl!("Remove"))
        .ok_enabled(removable)
        .show(ui, |ui, _, d: &mut HiddenDraft| {
            Text::new(tl!("Select the items to remove from this document.")).shade(Shade::Muted).show(ui);
            for (h, n, on) in d.found.iter_mut() {
                // The count stands beside its checkbox, on one row.
                Row::new().show(ui, |ui, _| {
                    Checkbox::new(Bind::bound(on)).label(tl!(h.label())).enabled(*n > 0).show(ui);
                    let count = if *n == 0 { tl!("None found").to_string() } else { n.to_string() };
                    Text::new(count).shade(Shade::Muted).show(ui);
                });
            }
            Text::new(tl!("Form fields are flattened: their values stay visible. Saving rewrites the whole file.")).shade(Shade::Muted).show(ui);
            true
        });
    match (shown.accepted, shown.closed) {
        (Some(d), _) => {
            let which: Vec<pdfcraft_engine::Hidden> = d.found.iter().filter(|f| f.2 && f.1 > 0).map(|f| f.0).collect();
            let n: usize = d.found.iter().filter(|f| f.2).map(|f| f.1).sum();
            if app.apply_edit(ui.ctx(), Edit::RemoveHidden { which }) {
                if let Some((_, id)) = app.active_ids() {
                    app.renew_recovery(id);
                }
                if n == 1 {
                    app.notify_tr("Removed 1 hidden item. Save to remove them from the file.");
                } else {
                    app.notify_fmt("Removed {n} hidden items. Save to remove them from the file.", &[("n", &n.to_string())]);
                }
            }
        }
        (None, Some(_)) => {}
        (None, None) => app.hidden_draft = Some(draft),
    }
}
