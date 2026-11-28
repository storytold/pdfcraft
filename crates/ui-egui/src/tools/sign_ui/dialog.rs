//! The signing dialog: Sign with a Digital ID, Configure a Digital ID for Signing and Sign as, one
//! `FormDialog` over the draft that `PdfCraftApp::sign_draft` holds.

use std::borrow::Cow;

use craft::behavior::{DisplayOrder, ItemSource, Selection, SelectionChange};
use craft::host::PickRequest;
use craft::review::SavedSignature;
use craft::review::ui::signature::paint_signature;
use craft::theme::variant::Tone;
use craft::ui::buttons::Button;
use craft::ui::choice::{Checkbox, Radio, RadioGroup};
use craft::ui::contract::{Bind, CollectionEvent, Entry, Props as _, SelectMode, Show, ShowCollection as _, ShowContainer as _};
use craft::ui::display::{Picture, PictureSize, Text};
use craft::ui::form::Field;
use craft::ui::layout_components::{Row, Stack};
use craft::ui::list::{List, ListRow, ThumbAspect, ThumbSize};
use craft::ui::notices::{Notice, NoticeAction};
use craft::ui::patterns::form_dialogs::FormDialog;
use craft::ui::pickers::Select;
use craft::ui::primitives::PaintShape;
use craft::ui::sections::{DialogRole, FooterButton};
use craft::ui::style::DataColor;
use craft::ui::text::TextField;

use super::{
    Appearance, AppearanceLabels, DigitalIdEntry, KEY_ALGORITHMS, NewIdDraft, PdfCraftApp, SignDraft, SignGraphic, SignStep, StatusCaptions, sign,
};
use crate::FilePurpose;

/// A line of explanatory text.
fn caption(text: impl Into<String>) -> impl Show {
    Text::new(text.into())
}

/// The footer index of OK.
const OK: usize = 0;
/// The footer index of the first extra button: OK and Cancel come first.
const FIRST_EXTRA: usize = 2;

/// The digital IDs as the rows of a list keyed by position; an unusable ID cannot take the lead.
struct IdRows<'a>(&'a [DigitalIdEntry]);

impl DisplayOrder<usize> for IdRows<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn key(&self, at: usize) -> usize {
        at
    }

    fn enabled(&self, at: usize) -> bool {
        self.0.get(at).is_some_and(|e| e.unusable.is_none())
    }

    fn text(&self, at: usize) -> Option<Cow<'_, str>> {
        self.0.get(at).map(|e| Cow::Borrowed(e.name.as_str()))
    }
}

impl ItemSource<usize> for IdRows<'_> {}

/// What a pass of the body asks of the app, which the body cannot do while it borrows the draft.
#[derive(Default)]
struct Asked {
    /// Remove the digital ID at this list index (its confirmation was accepted).
    remove: Option<usize>,
    /// Browse… for a Digital ID file.
    browse: bool,
    /// Choose image…
    image: bool,
    /// Enter in the password field.
    submit: bool,
}

/// Shows the signing dialog while a signing draft is open, and carries out what it asks.
pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let Some(step) = app.sign_draft.as_ref().map(|d| d.step) else { return };
    // The time as the signature will show it (signing takes the session clock then).
    let date = sign::pdf::display_date(&app.session.signing_date());
    let title = match (step, app.sign_draft.as_ref()) {
        (SignStep::Choose, _) => tl!("Sign with a Digital ID").to_string(),
        (SignStep::Configure, _) => tl!("Configure a Digital ID for Signing").to_string(),
        (SignStep::SignAs, Some(d)) => {
            let name = d.selected.and_then(|i| app.digital_ids.get(i)).map(|e| e.name.as_str()).unwrap_or_default();
            let verb = if d.certify.is_some() { tl!("Certify") } else { tl!("Sign") };
            crate::i18n::fmt(tl!("{verb} as \"{name}\""), &[("verb", verb), ("name", name)])
        }
        (SignStep::SignAs, None) => return,
    };
    let configure_creates = app.sign_draft.as_ref().is_some_and(|d| d.new_id.create);
    let ok_label = match step {
        SignStep::Choose => tl!("Continue"),
        SignStep::Configure if configure_creates => tl!("Save"),
        SignStep::Configure => tl!("Continue"),
        SignStep::SignAs => tl!("Sign"),
    };

    let mut asked = Asked::default();
    let mut open = true;
    let out = {
        let PdfCraftApp { sign_draft, digital_ids, signature, .. } = &mut *app;
        let Some(draft) = sign_draft.as_mut() else { return };
        let ids: &[DigitalIdEntry] = digital_ids;
        let extras = extra_buttons(draft, ids);
        let ok_enabled = step != SignStep::Choose || draft.selected.is_some();
        let english_note = tl!("The labels are in English: the signature's font can't draw them in this language.");
        FormDialog::new(title, Bind::bound(draft))
            .opened(Bind::bound(&mut open))
            .ok_label(ok_label)
            .ok_enabled(ok_enabled)
            .accept_stays_open(true)
            .extra_buttons(extras)
            .show(ui, |ui, _, d| {
                match d.step {
                    SignStep::Choose => choose(ui, d, ids, &mut asked),
                    SignStep::Configure => configure(ui, d, &mut asked),
                    SignStep::SignAs => sign_as(ui, d, ids, signature.as_ref(), &date, english_note, &mut asked),
                }
                true
            })
    };
    if out.closed.is_some() {
        // Escape and Cancel; the draft's `Drop` clears the passwords.
        app.sign_draft = None;
        return;
    }
    run(ui.ctx(), app, step, out.picked, asked);
}

/// The footer buttons that follow OK and Cancel at this step.
fn extra_buttons(d: &SignDraft, ids: &[DigitalIdEntry]) -> Vec<FooterButton> {
    match d.step {
        SignStep::Choose => {
            let can_remove = removable(d, ids).is_some() && d.confirm_remove.is_none();
            vec![
                FooterButton::new(tl!("Configure New Digital ID"), DialogRole::Other),
                FooterButton::new(tl!("Remove"), DialogRole::Other).enabled(can_remove),
            ]
        }
        SignStep::Configure if ids.is_empty() => Vec::new(),
        SignStep::Configure | SignStep::SignAs => vec![FooterButton::new(tl!("Back"), DialogRole::Other)],
    }
}

/// The selected list entry when it is a file the app may take off the list (the OS key stores' IDs stay).
fn removable(d: &SignDraft, ids: &[DigitalIdEntry]) -> Option<usize> {
    d.selected.filter(|&i| ids.get(i).is_some_and(|e| !e.path.starts_with("keychain:") && !e.path.starts_with("windows:")))
}

/// Carries out the footer button picked this pass and what the body asked.
fn run(ctx: &egui::Context, app: &mut PdfCraftApp, step: SignStep, picked: Option<usize>, asked: Asked) {
    if let Some(i) = asked.remove {
        app.remove_digital_id(i);
    }
    if asked.browse {
        app.pick_first(
            PickRequest::open(FilePurpose::DigitalId.key()).filter(tl!("Digital ID"), &["p12", "pfx"]),
            None,
            |app: &mut PdfCraftApp, _: &egui::Context, path: std::path::PathBuf| {
                // The answer belongs to the dialog that asked: none open now, nothing to fill.
                if let Some(d) = app.sign_draft.as_mut() {
                    d.new_id.file = path.to_string_lossy().into_owned();
                }
            },
        );
    }
    if asked.image {
        app.pick_sign_graphic_image();
    }
    let has_ids = !app.digital_ids.is_empty();
    let mut sign_now = asked.submit && step == SignStep::SignAs;
    let Some(d) = app.sign_draft.as_mut() else { return };
    match (step, picked) {
        (SignStep::Choose, Some(OK)) => {
            d.step = SignStep::SignAs;
            d.error = None;
        }
        (SignStep::Choose, Some(n)) if n == FIRST_EXTRA => {
            d.step = SignStep::Configure;
            d.error = None;
        }
        (SignStep::Choose, Some(n)) if n == FIRST_EXTRA + 1 => {
            d.confirm_remove = removable(d, &app.digital_ids);
        }
        (SignStep::Configure, Some(OK)) => {
            let result = if d.new_id.create { app.create_digital_id() } else { app.import_digital_id() };
            let Some(d) = app.sign_draft.as_mut() else { return };
            match result {
                Ok(i) => {
                    d.selected = Some(i);
                    d.step = SignStep::Choose;
                    d.error = None;
                    d.new_id = NewIdDraft::default();
                }
                Err(e) => d.error = Some(e),
            }
        }
        (SignStep::Configure, Some(n)) if n == FIRST_EXTRA && has_ids => {
            d.new_id = NewIdDraft::default();
            d.step = SignStep::Choose;
            d.error = None;
        }
        (SignStep::SignAs, Some(n)) if n == FIRST_EXTRA => {
            zeroize::Zeroize::zeroize(&mut d.password);
            d.step = SignStep::Choose;
            d.error = None;
        }
        (SignStep::SignAs, Some(OK)) => sign_now = true,
        _ => {}
    }
    if sign_now {
        match app.finish_signing(ctx) {
            Ok(()) => app.sign_draft = None,
            Err(e) => {
                if let Some(d) = app.sign_draft.as_mut() {
                    d.error = Some(e);
                }
            }
        }
    }
}

/// The error line of the draft.
fn error(ui: &mut egui::Ui, error: &Option<String>) {
    if let Some(e) = error.as_ref().filter(|e| !e.is_empty()) {
        Notice::new(Tone::Bad, tl!(e).to_owned()).show(ui);
    }
}

/// A row's second line: the store the ID lives in, then why it can't sign, or its email, issuer and expiry.
fn details(e: &DigitalIdEntry) -> String {
    let store = if e.path.starts_with("keychain:") {
        tl!("Keychain  ·  ").to_string()
    } else if e.path.starts_with("windows:") {
        tl!("Windows store  ·  ").to_string()
    } else {
        String::new()
    };
    match &e.unusable {
        Some(reason) => format!("{store}{reason}"),
        None => format!(
            "{store}{email}{issued}{issuer}{expires}{date}",
            email = if e.email.is_empty() { String::new() } else { format!("{}  ·  ", e.email) },
            issued = tl!("Issued by: "),
            issuer = e.issuer,
            expires = tl!(", Expires: "),
            date = e.expires,
        ),
    }
}

/// Sign with a Digital ID: the list of IDs, unusable ones with their reason, and the removal question.
fn choose(ui: &mut egui::Ui, d: &mut SignDraft, ids: &[DigitalIdEntry], asked: &mut Asked) {
    caption(tl!("Choose the digital ID that you want to use for signing:")).show(ui);
    if ids.is_empty() {
        caption(tl!("No digital IDs yet.")).show(ui);
    } else {
        let source = IdRows(ids);
        let mut selection = d.selected.map_or_else(Selection::default, Selection::single);
        let out = List::<usize>::new(tl!("Digital IDs"))
            .select(SelectMode::Single)
            .natural_height(true)
            .selection(Bind::bound(&mut selection))
            .show_items(ui, &source, &mut |_, icx| match ids.get(icx.at) {
                Some(e) => {
                    let sub = details(e);
                    // Names are as long as their owners made them: the row cuts them and the tip says them whole.
                    ListRow::new(e.name.clone()).tooltip(format!("{}\n{sub}", e.name)).below(caption(sub), |_| {})
                }
                None => ListRow::new(String::new()),
            });
        for event in out.events {
            match event {
                CollectionEvent::Selection(SelectionChange::Set(keys)) => {
                    d.selected = keys.last().copied().filter(|&i| ids.get(i).is_some_and(|e| e.unusable.is_none()));
                    d.confirm_remove = None;
                }
                CollectionEvent::Selection(SelectionChange::Toggle(key)) => {
                    d.selected = (d.selected != Some(key)).then_some(key);
                    d.confirm_remove = None;
                }
                CollectionEvent::Selection(SelectionChange::Clear) => d.selected = None,
                CollectionEvent::Activate(key) if ids.get(key).is_some_and(|e| e.unusable.is_none()) => {
                    d.selected = Some(key);
                    d.step = SignStep::SignAs;
                }
                _ => {}
            }
        }
    }
    if let Some(i) = d.confirm_remove
        && let Some(e) = ids.get(i)
    {
        let question = crate::i18n::fmt(tl!("Remove \"{name}\" from this list? The file itself is not deleted."), &[("name", &e.name)]);
        let out = Notice::new(Tone::Warn, question).action(NoticeAction::new(tl!("Remove"))).action(NoticeAction::new(tl!("Cancel"))).show(ui);
        match out.action {
            Some(0) => asked.remove = Some(i),
            Some(_) => d.confirm_remove = None,
            None => {}
        }
    }
}

/// A labelled text field row.
fn text_row(ui: &mut egui::Ui, label: &'static str, value: &mut String) {
    Field::new(label).show(ui, |ui, _| {
        TextField::new(label, Bind::bound(&mut *value)).show(ui);
    });
}

/// Configure a Digital ID for Signing: from a file, or a new self-signed one.
fn configure(ui: &mut egui::Ui, d: &mut SignDraft, asked: &mut Asked) {
    let n = &mut d.new_id;
    RadioGroup::new(tl!("Digital ID"), Bind::bound(&mut n.create)).show(ui, |ui, _| {
        Radio::new(false).label(tl!("Use a Digital ID from a file")).show(ui);
        Radio::new(true).label(tl!("Create a new Digital ID (self-signed, saved to a password-protected file)")).show(ui);
    });
    if n.create {
        text_row(ui, tl!("Name"), &mut n.name);
        text_row(ui, tl!("Organizational Unit"), &mut n.unit);
        text_row(ui, tl!("Organization Name"), &mut n.organization);
        text_row(ui, tl!("Email Address"), &mut n.email);
        Field::new(tl!("Country/Region")).show(ui, |ui, _| {
            TextField::new(tl!("Country/Region"), Bind::bound(&mut n.country)).placeholder("US").limit(2).show(ui);
        });
        let entries: Vec<Entry<usize>> = KEY_ALGORITHMS.iter().enumerate().map(|(i, (label, _))| Entry::new(i, *label)).collect();
        Field::new(tl!("Key Algorithm")).show(ui, |ui, _| {
            Select::new(tl!("Key Algorithm"), Bind::bound(&mut n.key), &entries).show(ui);
        });
        for (label, value) in [(tl!("Password"), &mut n.password), (tl!("Confirm Password"), &mut n.confirm)] {
            Field::new(label).show(ui, |ui, _| {
                TextField::new(label, Bind::bound(&mut *value)).masked().show(ui);
            });
        }
        caption(tl!("Valid for 5 years, for digital signatures. Self-signed IDs are usually not trusted by others.")).show(ui);
    } else {
        Field::new(tl!("File (.p12, .pfx)")).show(ui, |ui, _| {
            TextField::new(tl!("File (.p12, .pfx)"), Bind::bound(&mut n.file)).show(ui);
            if Button::new(tl!("Browse…")).show(ui).clicked() {
                asked.browse = true;
            }
        });
        Field::new(tl!("Password")).show(ui, |ui, _| {
            TextField::new(tl!("Password"), Bind::bound(&mut n.file_password)).masked().show(ui);
        });
    }
    error(ui, &d.error);
}

/// The saved signature or image `g` stands for, given the saved Fill & Sign `signature`.
pub(super) fn graphic_of(signature: Option<&SavedSignature>, g: &SignGraphic) -> Option<SavedSignature> {
    match g {
        SignGraphic::Name => None,
        SignGraphic::FillSign => signature.cloned(),
        SignGraphic::Image(image) => Some(SavedSignature::Image(image.clone())),
    }
}

/// The words of a visible signature's labels in `lang`, and whether they fell back to English.
///
/// The labels are stamped into the document, so they are all in `lang` or all in English, never a
/// mix. English when `lang` is English; when it is written right to left (its catalog is kept in
/// display order, not the order a PDF stores text in); when one label has no translation; or when
/// the signature's font can't draw one (Chinese, Japanese, Cyrillic, Greek in Helvetica), which
/// signing would refuse. The English words are the sign crate's own, so they can't drift apart.
pub(crate) fn appearance_labels(lang: crate::i18n::Lang) -> (AppearanceLabels, bool) {
    let english = AppearanceLabels::default();
    if lang == crate::i18n::Lang::EN {
        return (english, false);
    }
    let said = |s: &str| (!lang.rtl() && crate::i18n::has(lang, s)).then(|| crate::i18n::tr(lang, s).to_string());
    let translated = (|| {
        Some(AppearanceLabels {
            signed_by: said(&english.signed_by)?,
            distinguished_name: said(&english.distinguished_name)?,
            reason: said(&english.reason)?,
            location: said(&english.location)?,
            date: said(&english.date)?,
        })
    })();
    match translated.filter(|l| l.first_undrawable().is_none()) {
        Some(labels) => (labels, false),
        None => (english, true),
    }
}

/// The words a named-layer signature appearance's status caption is drawn with: all three in
/// `lang`, or all three in English when `lang` misses one of them. The sign crate picks the
/// face and shapes right-to-left words itself, so they go in the catalogue's logical order.
pub(crate) fn status_captions(lang: crate::i18n::Lang) -> StatusCaptions {
    let english = StatusCaptions::default();
    if lang == crate::i18n::Lang::EN {
        return english;
    }
    let said = |s: &str| crate::i18n::has(lang, s).then(|| crate::i18n::tr(lang, s).to_owned());
    (|| Some(StatusCaptions { valid: said(&english.valid)?, unknown: said(&english.unknown)?, invalid: said(&english.invalid)? }))()
        .unwrap_or(english)
}

/// The detail lines of a visible signature, as `sign::pdf` stamps them (the distinguished name
/// aside, which the dialog doesn't know before the ID is opened): the label above the name, then
/// the reason, location and date, each after its label. `reason` and `location` are as typed.
pub(super) fn detail_lines(a: &Appearance, labels: &AppearanceLabels, name: &str, reason: &str, location: &str, date: &str) -> Vec<String> {
    let label = |l: &str, v: &str| if a.labels { format!("{l}{v}") } else { v.to_string() };
    let mut lines = Vec::new();
    if a.name {
        if a.labels {
            lines.push(labels.signed_by.clone());
        }
        lines.push(name.to_string());
    }
    if a.reason && !reason.trim().is_empty() {
        lines.push(label(&labels.reason, reason.trim()));
    }
    if a.location && !location.trim().is_empty() {
        lines.push(label(&labels.location, location.trim()));
    }
    if a.date {
        lines.push(label(&labels.date, date));
    }
    lines.retain(|l| !l.is_empty());
    lines
}

/// The signature as it will look: the graphic (or the name in the signature face) on white paper, left of
/// the details `lines`, from [`detail_lines`] with the labels signing will stamp.
fn preview(ui: &mut egui::Ui, name: &str, a: &Appearance, lines: &[String], graphic: Option<SavedSignature>) {
    let shown = graphic.or_else(|| a.name.then(|| SavedSignature::Typed(name.to_string())));
    if shown.is_none() && lines.is_empty() {
        return;
    }
    let ctx = ui.ctx().clone();
    Row::new().show(ui, |ui, _| {
        if let Some(saved) = &shown {
            Picture::painter(tl!("Signature preview"), ThumbAspect::new(2, 1), |pcx, rect| {
                // The page's paper, not a theme colour: the signature shows as it will print.
                pcx.with_draw(|draw| {
                    draw.data_fill(&PaintShape::Rect(rect, egui::CornerRadius::ZERO), DataColor::new(egui::Color32::WHITE));
                    paint_signature(&ctx, draw, rect, saved, DataColor::new(egui::Color32::BLACK));
                });
            })
            .extent(PictureSize::Sample(ThumbSize::Medium))
            .show(ui);
        }
        if !lines.is_empty() {
            Stack::new().show(ui, |ui, _| {
                for line in lines {
                    caption(line.clone()).show(ui);
                }
            });
        }
    });
}

/// Sign as: appearance, graphic, permissions, reason, location and the ID's password.
fn sign_as(
    ui: &mut egui::Ui,
    d: &mut SignDraft,
    ids: &[DigitalIdEntry],
    saved: Option<&SavedSignature>,
    date: &str,
    english_note: &str,
    asked: &mut Asked,
) {
    let Some(entry) = d.selected.and_then(|i| ids.get(i)) else {
        d.step = SignStep::Choose;
        return;
    };
    let in_os_key_store = entry.path.starts_with("keychain:") || entry.path.starts_with("windows:");
    if d.rect.is_some() || d.field.is_some() {
        let graphic = graphic_of(saved, &d.graphic);
        let (labels, in_english) = appearance_labels(crate::i18n::current());
        let lines = detail_lines(&d.appearance, &labels, &entry.name, &d.reason, &d.location, date);
        preview(ui, &entry.name, &d.appearance, &lines, graphic);
        if in_english && d.appearance.labels {
            caption(english_note).show(ui);
        }
        let a = &mut d.appearance;
        for (label, value) in [
            (tl!("Name"), &mut a.name),
            (tl!("Date"), &mut a.date),
            (tl!("Reason"), &mut a.reason),
            (tl!("Location"), &mut a.location),
            (tl!("Distinguished name"), &mut a.distinguished_name),
            (tl!("Labels"), &mut a.labels),
        ] {
            Checkbox::new(Bind::bound(value)).label(label).show(ui);
        }
        // Acrobat: Configure Signature Appearance, Text, Draw or Image.
        let mut choice = match d.graphic {
            SignGraphic::Name => 0u8,
            SignGraphic::FillSign => 1,
            SignGraphic::Image(_) => 2,
        };
        RadioGroup::new(tl!("Graphic"), Bind::bound(&mut choice)).show(ui, |ui, _| {
            Radio::new(0u8).label(tl!("Signer's name")).show(ui);
            Radio::new(1u8).label(tl!("Fill & Sign signature")).enabled(saved.is_some()).show(ui);
            Radio::new(2u8).label(tl!("Image")).enabled(d.image.is_some()).show(ui);
        });
        d.graphic = match (choice, &d.image, &d.graphic) {
            (0, _, _) => SignGraphic::Name,
            (1, _, _) if saved.is_some() => SignGraphic::FillSign,
            (2, _, SignGraphic::Image(current)) => SignGraphic::Image(current.clone()),
            (2, Some(image), _) => SignGraphic::Image(image.clone()),
            (_, _, current) => current.clone(),
        };
        if Button::new(tl!("Choose image…")).show(ui).clicked() {
            asked.image = true;
        }
    } else {
        caption(tl!("The signature will be invisible.")).show(ui);
    }
    if let Some(current) = d.certify {
        let label = |p: u8| match p {
            1 => tl!("No changes allowed"),
            3 => tl!("Annotations, form fill-in, and digital signatures"),
            _ => tl!("Form fill-in and digital signatures"),
        };
        let entries: Vec<Entry<u8>> = [1u8, 2, 3].into_iter().map(|p| Entry::new(p, label(p))).collect();
        let mut permitted = current;
        Field::new(tl!("Permitted actions after certifying")).show(ui, |ui, _| {
            Select::new(tl!("Permitted actions after certifying"), Bind::bound(&mut permitted), &entries).show(ui);
        });
        d.certify = Some(permitted);
    }
    for (label, value) in [(tl!("Reason"), &mut d.reason), (tl!("Location"), &mut d.location)] {
        Field::new(label).show(ui, |ui, _| {
            TextField::new(label, Bind::bound(&mut *value)).placeholder(tl!("Optional")).show(ui);
        });
    }
    if in_os_key_store {
        let note = if entry.path.starts_with("windows:") {
            tl!("The key is in the Windows certificate store, which may ask to allow PdfCraft to use it.")
        } else {
            tl!("The key is in the macOS Keychain, which may ask to allow PdfCraft to use it.")
        };
        Notice::new(Tone::Info, note).show(ui);
    } else {
        Field::new(tl!("Digital ID password")).show(ui, |ui, _| {
            // PLAN: `submits()` is assumed to make Enter report `EditResponse::submitted`.
            let out = TextField::new(tl!("Digital ID password"), Bind::bound(&mut d.password)).masked().submits().show(ui);
            if out.submitted {
                asked.submit = true;
            }
        });
    }
    error(ui, &d.error);
}
