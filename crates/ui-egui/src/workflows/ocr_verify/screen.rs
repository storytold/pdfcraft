//! The OCR Verify screen's layout: the Pages column, the centre (options bar, info strip, source
//! page, legend, status line) and the Recognized text / Word column, with the commands that
//! reach the screen while focus is in it. The controls write [`Intents`], never the app while
//! `app.ocr_verify` is read; [`super::apply`] applies them after the drawing.

use craft::behavior::ColumnWidth;
use craft::theme::variant::Tone;
use craft::theme::{TypeRole, Variant};
use craft::ui::bars::{Bar, BarKind, StatusMessage};
use craft::ui::buttons::Button;
use craft::ui::choice::{Checkbox, Radio, RadioGroup};
use craft::ui::contract::{Bind, CommandSet, Commands as _, Entry, LeastWidth, Props as _, Show as _, ShowContainer as _};
use craft::ui::display::{Divider, Shade, Text};
use craft::ui::layout_components::{Grid, Row, Stack};
use craft::ui::pickers::Select;
use craft::ui::runtime::commands::focus_within;
use craft::ui::sections::Panel;
use craft::ui::text::NumberField;
use egui::{Id, Ui};
use pdfcraft_engine::ocr::{EngineChoice, MergeStrategy, OutputMode};

use super::{Phase, apply, panes, source};
use crate::PdfCraftApp;
use crate::workflows::ocr_ui::{self, OcrPages};

/// What a frame's controls asked for, applied after the drawing (the panes read `app` while
/// the controls write it, so nothing mutates through an open borrow).
#[derive(Default)]
pub(super) struct Intents {
    pub(super) run: bool,
    pub(super) cancel: bool,
    pub(super) cancel_reread: bool,
    pub(super) accept: bool,
    pub(super) reject: bool,
    pub(super) exit: bool,
    pub(super) undo: bool,
    pub(super) next_uncertain: bool,
    pub(super) prev_uncertain: bool,
    pub(super) toggle_page_verified: bool,
    pub(super) reocr_region: bool,
    pub(super) reocr_page: bool,
    pub(super) accept_page: bool,
    pub(super) reject_page: bool,
    pub(super) select_page: Option<usize>,
    pub(super) select_word: Option<(usize, usize)>,
    pub(super) correct: Option<String>,
    pub(super) set_removed: Option<bool>,
    pub(super) set_word_verified: Option<bool>,
    pub(super) dictionary_add: bool,
}

/// The keys of the screen. They reach it while focus is inside it; a focused text field keeps
/// Ctrl+T and Ctrl+Z (the typing scope), and F8 is the screen's either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, CommandSet)]
#[command(prefix = "ocrVerify", scope = "ocr_verify")]
pub(super) enum OcrVerifyCommands {
    /// The next word that asks for review.
    #[command(label = "Next uncertain", default = "F8")]
    NextUncertain,
    /// The previous word that asks for review.
    #[command(label = "Previous uncertain", default = "Shift+F8")]
    PreviousUncertain,
    /// Mark the shown page reviewed, or unmark it.
    #[command(label = "Page verified", default = "Cmd+T")]
    TogglePageVerified,
    /// Undo the last review change.
    #[command(label = "Undo", default = "Cmd+Z")]
    Undo,
}

/// The page list and text column use their usual widths; the page takes the remaining room.
const COLUMNS: &[ColumnWidth] = &[ColumnWidth::Fixed(190.0), ColumnWidth::fill(1.0), ColumnWidth::Fixed(340.0)];

/// The OCR Verify screen (Scan & OCR ▸ Correct recognized text), in the document area.
pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut Ui) {
    follow_active_document(app);
    keep_models_probing(ui);
    let ctx = ui.ctx().clone();
    let pages_id = Id::unique("ocr-verify-pages");
    let text_id = Id::unique("ocr-verify-text");
    let mut n = Intents::default();

    // PLAN: the sheet puts `command_scope` on a `Row`; only `Stack` forwards it, so a `Stack` holds the scope and a `Grid` lays the columns out.
    Stack::new().fill().command_scope(OcrVerifyCommands::SCOPE).show(ui, |ui, root| {
        while let Some((command, _)) = root.take_command::<OcrVerifyCommands>() {
            match command {
                OcrVerifyCommands::NextUncertain => n.next_uncertain = true,
                OcrVerifyCommands::PreviousUncertain => n.prev_uncertain = true,
                OcrVerifyCommands::TogglePageVerified => n.toggle_page_verified = true,
                OcrVerifyCommands::Undo => n.undo = true,
            }
        }
        Grid::new(COLUMNS).fill().show(ui, |ui, _| {
            Panel::new(tl!("Pages")).id(pages_id).least_width(LeastWidth::DockColumn).selected(focus_within(&ctx, pages_id)).show(ui, |ui, _| {
                panes::pages(app, ui, &mut n);
            });
            centre(app, ui, &mut n);
            Stack::new().fill().show(ui, |ui, _| {
                Panel::new(tl!("Recognized text"))
                    .id(text_id)
                    .least_width(LeastWidth::DockColumn)
                    .selected(focus_within(&ctx, text_id))
                    .show(ui, |ui, _| panes::text(app, ui, &mut n));
                panes::inspector(app, ui, &mut n);
            });
        });
    });

    apply(app, &ctx, n);
}

/// With no reading in hand (before a run, or after a review was accepted or rejected), the
/// source pane shows the document in front at the page it is on, so the screen never opens on
/// a blank pane (#6485). A run or a review keeps the document and page it was made for.
fn follow_active_document(app: &mut PdfCraftApp) {
    if app.ocr_verify.has_reading() {
        return;
    }
    let active = app.active_ids();
    app.ocr_verify.doc = active.map(|(_, id)| id);
    app.ocr_verify.current = active.and_then(|(i, _)| app.views.get(i)).map_or(0, |view| view.current());
}

/// The Engine choice is probed in the background; the screen asks for frames until the answer
/// is in, so the Engine entries enable without a pointer move.
fn keep_models_probing(ui: &Ui) {
    ocr_ui::keep_asking_while_probing(ui.ctx());
}

/// The centre column: the options bar, the info strip, the source page with its legend, and the
/// status line.
fn centre(app: &mut PdfCraftApp, ui: &mut Ui, n: &mut Intents) {
    let message = app.ocr_verify.message.clone();
    let tone = if app.ocr_verify.phase == Phase::RecoverableError { Tone::Accent } else { Tone::Neutral };
    Stack::new()
        .fill()
        .footer(move |ui, _| {
            source::legend(ui);
            Bar::new(tl!("Status"), BarKind::Status).show(ui, |ui, _| {
                StatusMessage::new(message.clone()).tone(tone).show(ui);
            });
        })
        .show(ui, |ui, _| {
            options_bar(app, ui, n);
            info_strip(app, ui);
            source::show(app, ui, n);
        });
}

/// The language dropdown's text: its name and its code ("English (en)").
fn language_name(code: &str, name: &str) -> String {
    format!("{name} ({code})")
}

/// The Engine dropdown's entries. An engine that cannot run is disabled and says why; the others
/// say what they do. Automatic's description names the Strategy that combines the two.
fn engine_entries() -> Vec<Entry<EngineChoice>> {
    [
        (EngineChoice::Auto, engine_label(EngineChoice::Auto), tl!("Both engines, combined as the Strategy says")),
        (EngineChoice::Ocrs, engine_label(EngineChoice::Ocrs), tl!("The built-in engine (pure Rust)")),
        (EngineChoice::Tesseract, engine_label(EngineChoice::Tesseract), tl!("The tesseract program, run as an external process")),
    ]
    .into_iter()
    .map(|(choice, label, what)| {
        let entry = Entry::new(choice, label);
        match pdfcraft_engine::ocr::engine_unavailable_reason(choice) {
            Some(reason) => entry.disabled_reason(crate::i18n::t(&reason).to_string()),
            None => entry.description(what),
        }
    })
    .collect()
}

/// The Engine dropdown's text.
fn engine_label(engine: EngineChoice) -> &'static str {
    match engine {
        EngineChoice::Auto => tl!("Automatic"),
        EngineChoice::Ocrs => tl!("Built-in"),
        EngineChoice::Tesseract => "Tesseract",
    }
}

/// The Strategy dropdown's text.
fn strategy_label(strategy: MergeStrategy) -> &'static str {
    match strategy {
        MergeStrategy::PrimaryOnly => tl!("Primary only"),
        MergeStrategy::ConfidenceWeighted => tl!("Confidence weighted"),
        MergeStrategy::RoverVote => tl!("ROVER vote"),
    }
}

/// The options bar: every control is live. The choices feed the next run (and are persisted),
/// Run starts a recognition, Accept/Reject/navigate/verify only act in ReviewReady.
fn options_bar(app: &mut PdfCraftApp, ui: &mut Ui, n: &mut Intents) {
    let v = &app.ocr_verify;
    let d = &mut app.ocr_draft;
    let review = v.phase == Phase::ReviewReady;
    let run_enabled = v.can_run() && v.reread.is_none();
    let accept_enabled = review && v.word_count() + v.review.applied_pages.len() > 0;
    let (running, rereading, can_reject) = (v.phase == Phase::Running, v.reread.is_some(), v.can_reject());
    let page_done = review && v.review.verified_pages.contains(&v.current);

    let languages: Vec<Entry<String>> =
        pdfcraft_engine::ocr::LANGUAGES.iter().map(|(code, name)| Entry::new((*code).to_string(), language_name(code, name))).collect();
    let engines = engine_entries();
    let strategies = [
        (MergeStrategy::PrimaryOnly, tl!("Use the chosen engine's reading alone")),
        (MergeStrategy::ConfidenceWeighted, tl!("Add a second engine's reading when the first is unsure")),
        (MergeStrategy::RoverVote, tl!("Always read with both engines and merge the words")),
    ]
    .map(|(strategy, what)| Entry::new(strategy, strategy_label(strategy)).description(what));
    let outputs = [
        Entry::new(OutputMode::Searchable, tl!("Searchable Image (Exact)"))
            .description(tl!("Adds invisible text over each word; the page image is not changed")),
        Entry::new(OutputMode::EditableText, tl!("Editable text")).description(tl!("Writes a new document of visible text; the scan is not changed")),
    ];
    let (mut from, mut to) = (Some(d.from as f64), Some(d.to as f64));
    let mut verified = page_done;
    let engine_before = d.engine;

    // Production's bar is one wide row pair; in a narrower centre column (the side panels stay) the same controls
    // are grouped into rows that each fit, so nothing runs out of the screen.
    let wide = ui.available_width() >= 1000.0;
    let mut language = |ui: &mut Ui| {
        Select::new(tl!("Language"), Bind::bound(&mut d.language), &languages).show(ui);
        Select::new(tl!("Engine"), Bind::bound(&mut d.engine), &engines).show(ui);
    };
    let mut reading = |ui: &mut Ui| {
        Select::new(tl!("Strategy"), Bind::bound(&mut d.strategy), &strategies).show(ui);
        Select::new(tl!("Output"), Bind::bound(&mut d.output_mode), &outputs).show(ui);
    };
    let mut prep = |ui: &mut Ui| {
        for (flag, label, why) in [
            (&mut d.auto_rotate, tl!("Auto-rotate"), tl!("Turns pages scanned sideways or upside down upright before reading")),
            (&mut d.deskew, tl!("Deskew"), tl!("Straightens skewed text lines before reading")),
            (&mut d.denoise, tl!("Denoise"), tl!("A 3×3 median filter against scan speckle")),
            (&mut d.binarize, tl!("Binarize"), tl!("Sauvola binarization: ink to black, paper to white")),
        ] {
            Checkbox::new(Bind::bound(flag)).label(label).tooltip(format!("{label} \u{2014} {why}")).show(ui);
        }
    };
    let mut scope = |ui: &mut Ui| {
        RadioGroup::new(tl!("Pages to read"), Bind::bound(&mut d.pages)).show(ui, |ui, _| {
            Row::new().show(ui, |ui, _| {
                Radio::new(OcrPages::All).label(tl!("All pages")).show(ui);
                Radio::new(OcrPages::Current).label(tl!("Current page")).show(ui);
                Radio::new(OcrPages::Range).label(tl!("From")).show(ui);
            });
        });
        let ranged = d.pages == OcrPages::Range;
        NumberField::new(tl!("From page"), Bind::bound(&mut from)).range(1.0..=9999.0).enabled(ranged).show(ui);
        Text::new(tl!("to")).shade(Shade::Muted).show(ui);
        NumberField::new(tl!("To page"), Bind::bound(&mut to)).range(1.0..=9999.0).enabled(ranged).show(ui);
    };
    let actions = |ui: &mut Ui, n: &mut Intents| {
        if wide {
            Divider::vertical().show(ui);
        }
        if Button::new(tl!("Run OCR")).variant(Variant::PRIMARY).enabled(run_enabled).show(ui).clicked() {
            n.run = true;
        }
        if running && Button::new(tl!("Cancel")).variant(Variant::SECONDARY).show(ui).clicked() {
            n.cancel = true;
        }
        if rereading && Button::new(tl!("Cancel re-read")).variant(Variant::SECONDARY).show(ui).clicked() {
            n.cancel_reread = true;
        }
        if Button::new(tl!("Accept")).variant(Variant::SECONDARY).enabled(accept_enabled).show(ui).clicked() {
            n.accept = true;
        }
        if Button::new(tl!("Reject")).variant(Variant::SECONDARY).enabled(can_reject).show(ui).clicked() {
            n.reject = true;
        }
    };
    let mut review_bar = |ui: &mut Ui, n: &mut Intents| {
        if wide {
            Divider::vertical().show(ui);
        }
        Checkbox::new(Bind::bound(&mut verified))
            .label(tl!("Page verified"))
            .tooltip(tl!("Mark this page as reviewed (Ctrl+T)"))
            .enabled(review)
            .show(ui);
        if Button::new(tl!("Next uncertain"))
            .variant(Variant::GHOST)
            .icon(craft::behavior::IconId::new("chevron_right"))
            .tooltip(tl!("F8: next word to review; Shift+F8 goes back"))
            .enabled(review)
            .show(ui)
            .clicked()
        {
            n.next_uncertain = true;
        }
        if Button::new(tl!("Previous uncertain"))
            .variant(Variant::GHOST)
            .icon(craft::behavior::IconId::new("chevron_left"))
            .tooltip(tl!("Shift+F8: the previous word to review"))
            .enabled(review)
            .show(ui)
            .clicked()
        {
            n.prev_uncertain = true;
        }
        if Button::new(tl!("Exit")).variant(Variant::GHOST).icon(craft::behavior::IconId::new("close")).show(ui).clicked() {
            n.exit = true;
        }
    };

    Stack::new().label(tl!("OCR options")).fill_width().show(ui, |ui, _| {
        if wide {
            Row::new().wrap(true).show(ui, |ui, _| {
                language(ui);
                reading(ui);
                prep(ui);
            });
            Row::new().wrap(true).show(ui, |ui, _| {
                scope(ui);
                actions(ui, n);
                review_bar(ui, n);
            });
        } else {
            Row::new().show(ui, |ui, _| language(ui));
            Row::new().show(ui, |ui, _| reading(ui));
            Row::new().show(ui, |ui, _| prep(ui));
            Row::new().show(ui, |ui, _| scope(ui));
            Row::new().show(ui, |ui, _| actions(ui, n));
            Row::new().show(ui, |ui, _| review_bar(ui, n));
        }
    });

    if let Some(from) = from {
        d.from = from.round().max(1.0) as usize;
    }
    if let Some(to) = to {
        d.to = to.round().max(1.0) as usize;
    }
    if verified != page_done {
        n.toggle_page_verified = true;
    }
    // Automatic with "Primary only" would never run the second engine: the pick moves the strategy to
    // Automatic's own default.
    if d.engine != engine_before && d.engine == EngineChoice::Auto && d.strategy == MergeStrategy::PrimaryOnly {
        d.strategy = d.engine.default_strategy();
    }
}

/// The info strip: where the review stands, in facts — page, average confidence, how many
/// words ask for review, the share verified, and the engine · strategy that read it.
fn info_strip(app: &PdfCraftApp, ui: &mut Ui) {
    let v = &app.ocr_verify;
    let read = v.pages.iter().filter(|p| p.skipped.is_none()).count();
    let avg = v.page(v.current).and_then(pdfcraft_engine::ocr::OcrPage::mean_confidence);
    let cells = [
        (tl!("PAGE"), v.page_indicator(v.doc.and_then(|d| app.session.get(d)).map(|d| d.info.pages.len()))),
        (tl!("AVG CONFIDENCE"), avg.map_or_else(|| "—".into(), |c| format!("{c:.0}%"))),
        (tl!("LOW-CONFIDENCE WORDS"), v.suspect_count().to_string()),
        (tl!("VERIFIED %"), format!("{}%", (v.verified_fraction() * 100.0).round() as i64)),
        (
            tl!("ENGINE · STRATEGY"),
            if read == 0 { "—".into() } else { format!("{} · {}", engine_label(app.ocr_draft.engine), strategy_label(app.ocr_draft.strategy)) },
        ),
    ];
    Row::new().show(ui, |ui, _| {
        for (at, (caption, value)) in cells.iter().enumerate() {
            if at > 0 {
                Divider::vertical().show(ui);
            }
            Text::new(*caption).role(TypeRole::Caption).shade(Shade::Faint).show(ui);
            Text::new(value.clone()).role(TypeRole::Caption).show(ui);
        }
    });
}
