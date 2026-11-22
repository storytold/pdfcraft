//! Format text: the font; size, colour, lists and alignment; bold, italic, underline, superscript and
//! subscript; line and paragraph spacing; horizontal scale and character spacing. One framework
//! `FormatText` panel serves a paragraph being edited (with the fonts already on its page and the
//! spacing controls) and added text (the standard families, size, colour, style, alignment and
//! character spacing). What PdfCraft can't apply yet (lists, paragraph spacing, or line spacing and
//! scale for added text) is shown disabled and says so ([`unsupported`]).
//!
//! This module maps the panel's values from the thing being formatted ([`values`]) and its edits back
//! onto it ([`apply`]); the layout, the number fields and their parsing are the framework's.

use std::ops::RangeInclusive;

use craft::behavior::EditEvent;
#[cfg(not(target_arch = "wasm32"))]
use craft::font_manager::picker::{HostFontGroup, HostFontRow};
#[cfg(not(target_arch = "wasm32"))]
use craft::text::ui::character::FontPickerSource;
use craft::text::ui::character::{
    BaselinePosition, CharacterChange, CharacterValues, FormatControl, FormatText, FormatTextChange, FormatTextOutput, LineSpacing, ParagraphChange,
    ParagraphValues, TextAlign as PanelAlign,
};
use craft::theme::Rgba;
use craft::ui::contract::{Props as _, ShowContainer as _};
use craft::ui::runtime::env::Label;
use craft::ui::sections::Section;
use pdfcraft_engine::text_script::Script;
use pdfcraft_engine::{AddedText, FontFamily, TextAlign};

use crate::PdfCraftApp;
use crate::content_ui::TextDraft;
use crate::edit_text_ui::{Extras, LineEditor, SelectionFormatting};

/// A font picked in the font row. The caller applies it, since keeping the paragraph's own font
/// also means restoring the family it started with.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FontPick {
    /// The fonts the paragraph already uses.
    KeepSource,
    /// One of the page's fonts, by resource name.
    Resource(String),
    /// A standard family.
    Family(FontFamily),
    /// Load font…: pick a font file.
    Load,
    /// Reload: look for installed fonts again.
    Reload,
}

/// The standard families, in the order the font row lists them.
const STANDARD: [FontFamily; 3] = [FontFamily::Helvetica, FontFamily::Times, FontFamily::Courier];

impl FontPick {
    /// The key the font picker reports when the host row of this pick is run.
    fn key(&self) -> String {
        match self {
            FontPick::KeepSource => "keep".to_string(),
            FontPick::Resource(resource) => format!("res:{resource}"),
            FontPick::Family(family) => format!("std:{}", family.label()),
            FontPick::Load => "load".to_string(),
            FontPick::Reload => "reload".to_string(),
        }
    }

    /// The pick a host row's key names; `None` for a key this module did not make.
    fn from_key(key: &str) -> Option<FontPick> {
        match key {
            "keep" => Some(FontPick::KeepSource),
            "load" => Some(FontPick::Load),
            "reload" => Some(FontPick::Reload),
            _ => {
                if let Some(resource) = key.strip_prefix("res:") {
                    return Some(FontPick::Resource(resource.to_string()));
                }
                let label = key.strip_prefix("std:")?;
                STANDARD.into_iter().find(|f| f.label() == label).map(FontPick::Family)
            }
        }
    }
}

/// Which target a colour answer applies to, named when the swatch was clicked: the picker answers on a later
/// pass, when the panel's own target value is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Recolour {
    /// The open paragraph.
    Paragraph,
    /// The box being typed.
    Draft,
    /// The selected added text, by page and place among that page's added content.
    Added(usize, usize),
    /// The style the next text gets.
    Armed,
}

/// Applies the colour a picker answered to what it was asked for, as one change.
fn recolour(ctx: &egui::Context, app: &mut PdfCraftApp, view: usize, which: Recolour, color: craft::theme::Srgba) {
    let rgb = [color.r, color.g, color.b].map(f64::from);
    match which {
        Recolour::Paragraph => {
            if let Some(mut ed) = app.views.get(view).and_then(|v| v.session.line_editor.clone()) {
                ed.look.set_color(rgb);
                app.apply_line_format(ctx, view, ed);
            }
        }
        Recolour::Draft => {
            if let Some(mut style) = app.views.get(view).and_then(|v| v.session.content.draft.as_ref()).map(|d| d.style.clone()) {
                style.set_color(rgb);
                app.restyle_draft(ctx, view, style);
            }
        }
        Recolour::Added(page, index) => {
            let current = app.views.get(view).and_then(|v| app.session.get(v.id)).and_then(|doc| {
                match &doc.added().iter().filter(|a| a.page == page).nth(index)?.content {
                    pdfcraft_engine::AddedContent::Text(text) => Some(text.clone()),
                    _ => None,
                }
            });
            if let Some(mut style) = current {
                style.set_color(rgb);
                // A gray or CMYK ink the new colour no longer shows goes with it (#6440).
                style.drop_stale_ink();
                if app.apply_edit(
                    ctx,
                    pdfcraft_engine::Edit::UpdateContent { page, index, content: pdfcraft_engine::AddedContent::Text(style.clone()) },
                ) {
                    app.text_style = AddedText { text: String::new(), rect: [0.0; 4], aligns: Vec::new(), ..style };
                }
            }
        }
        Recolour::Armed => {
            app.text_style.set_color(rgb);
            app.text_style.drop_stale_ink();
        }
    }
}

/// What Format text formats.
pub(crate) enum Target {
    /// The paragraph open in the editor of the active view, with its look and spacing.
    Paragraph(Box<LineEditor>),
    /// The box of added text being typed.
    Draft(Box<TextDraft>),
    /// The selected added text: its page, its place among that page's added content, and its style.
    Added {
        /// The page.
        page: usize,
        /// The item's place among the page's added content.
        index: usize,
        /// The text's style as the page has it.
        text: AddedText,
    },
    /// Add text is armed: the style the next text gets.
    Armed(AddedText),
}

impl Target {
    /// What a colour picked for this target applies to.
    fn recolour(&self) -> Recolour {
        match self {
            Target::Paragraph(_) => Recolour::Paragraph,
            Target::Draft(_) => Recolour::Draft,
            Target::Added { page, index, .. } => Recolour::Added(*page, *index),
            Target::Armed(_) => Recolour::Armed,
        }
    }

    /// The look the panel shows and changes.
    fn look(&self) -> &AddedText {
        match self {
            Target::Paragraph(ed) => &ed.look,
            Target::Draft(draft) => &draft.style,
            Target::Added { text, .. } | Target::Armed(text) => text,
        }
    }

    /// The spacing a paragraph has beyond the look; added text has none.
    fn extras(&self) -> Option<&Extras> {
        match self {
            Target::Paragraph(ed) => Some(&ed.extras),
            _ => None,
        }
    }

    /// The text being typed in, for the alignment and script buttons; none for text that is not open.
    fn paragraphs(&self) -> Option<Paragraphs<'_>> {
        match self {
            Target::Paragraph(ed) => {
                let selection = ed.selection.as_ref().map(|s| crate::edit_text_ui::selected_bytes(&ed.text, s));
                Some(Paragraphs { text: &ed.text, selection, tidy: false })
            }
            Target::Draft(draft) => {
                let selection = draft.selection.as_ref().map(|s| crate::edit_text_ui::selected_bytes(&draft.text, s));
                Some(Paragraphs { text: &draft.text, selection, tidy: true })
            }
            Target::Added { .. } | Target::Armed(_) => None,
        }
    }
}

/// Text being typed in, for the alignment buttons: with some of it selected, they show and set the
/// alignment of the paragraphs (lines between `\n`s) the selection touches; with none, the box's.
pub(crate) struct Paragraphs<'a> {
    pub text: &'a str,
    /// A byte range of `text`; empty or `None` is nothing selected.
    pub selection: Option<std::ops::Range<usize>>,
    /// The box's own alignment is known (added text), so a paragraph set to it keeps no entry of its
    /// own. An edited paragraph's alignment is only guessed from where its rows sit: its rows keep
    /// every alignment set.
    pub tidy: bool,
}

impl Paragraphs<'_> {
    /// The paragraphs a selection touches, or `None` with nothing selected.
    fn selected(&self) -> Option<std::ops::RangeInclusive<usize>> {
        self.selection.clone().filter(|r| !r.is_empty()).map(|r| pdfcraft_engine::paragraph_align::selected(self.text, r))
    }

    /// The alignment the buttons show: the first selected paragraph's, or the box's.
    fn shown(&self, look: &AddedText) -> TextAlign {
        self.selected().map_or(look.align, |which| look.paragraph_align(*which.start()))
    }

    /// `align` picked: for the selected paragraphs, or the whole box (dropping theirs).
    fn choose(&self, look: &mut AddedText, align: TextAlign) {
        if !self.tidy && self.selected().is_some() {
            return;
        }
        match self.selected() {
            Some(which) => {
                let count = pdfcraft_engine::paragraph_align::count(self.text);
                look.aligns = pdfcraft_engine::paragraph_align::set(&look.aligns, count, which, align, self.tidy.then_some(look.align));
            }
            None => {
                look.align = align;
                look.aligns.clear();
            }
        }
    }
}

const SIZES: [f64; 16] = [4.0, 6.0, 8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 36.0, 48.0, 72.0];
const LINE_SPACINGS: [f64; 7] = [1.0, 1.15, 1.2, 1.5, 2.0, 2.5, 3.0];
const SCALES: [f64; 8] = [50.0, 75.0, 90.0, 100.0, 110.0, 125.0, 150.0, 200.0];
const CHAR_SPACINGS: [f64; 8] = [-1.0, -0.5, 0.0, 0.5, 1.0, 2.0, 3.0, 5.0];
/// What can be typed: horizontal scale in percent, character spacing in points.
const SCALE_RANGE: std::ops::RangeInclusive<f64> = 10.0..=400.0;
const CHAR_SPACING_RANGE: std::ops::RangeInclusive<f64> = -5.0..=50.0;
/// The sizes the engine takes, in points: the bound the size field shows and `apply` enforces.
const SIZE_RANGE: RangeInclusive<f64> = 1.0..=500.0;

/// What the panel shows for a [`Target`].
pub(crate) struct Values {
    /// The character controls.
    pub character: CharacterValues,
    /// The paragraph controls.
    pub paragraph: ParagraphValues,
    /// The line spacing; `None` for none (added text).
    pub line_spacing: Option<LineSpacing>,
    /// The host font row the font field shows as current; `None` while an installed family or a mix is.
    pub font: Option<FontPick>,
}

/// A colour as the panel's swatch holds it.
fn rgba(color: [f64; 3]) -> Rgba {
    let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgba([byte(color[0]), byte(color[1]), byte(color[2]), 255])
}

fn panel_align(align: TextAlign) -> PanelAlign {
    match align {
        TextAlign::Left => PanelAlign::Left,
        TextAlign::Center => PanelAlign::Center,
        TextAlign::Right => PanelAlign::Right,
        TextAlign::Justify => PanelAlign::JustifyLeft,
    }
}

fn engine_align(align: PanelAlign) -> TextAlign {
    match align {
        PanelAlign::Left => TextAlign::Left,
        PanelAlign::Center => TextAlign::Center,
        PanelAlign::Right => TextAlign::Right,
        _ => TextAlign::Justify,
    }
}

/// The values the panel shows for `target`: the selection's formatting (mixed size, font, bold,
/// italic and colour show empty), the look and its spacing, and the alignment the selection has.
/// `fmt` is the open paragraph's [`LineEditor::selection_formatting`]; `sources` its page's fonts.
pub(crate) fn values(target: &Target, fmt: Option<&SelectionFormatting>, sources: &[(String, String)]) -> Values {
    let look = target.look();
    let extras = target.extras();
    let paragraphs = target.paragraphs();
    let mut character = CharacterValues::default();
    let shown_font = match fmt {
        Some(fmt) if fmt.font_shown != tl!("Mixed") => Some(fmt.font_shown.clone()),
        Some(_) => None,
        None => Some(look.family.label().to_string()),
    };
    character.family = shown_font.clone();
    character.size = match fmt {
        Some(fmt) => fmt.size.map(|s| s as f32),
        None => Some(look.size as f32),
    };
    character.color = match fmt {
        Some(fmt) => fmt.color.map(rgba),
        None => Some(rgba(look.color)),
    };
    character.faux_bold = fmt.map_or(Some(look.bold), |f| f.bold);
    character.faux_italic = fmt.map_or(Some(look.italic), |f| f.italic);
    character.underline = Some(extras.is_some_and(|e| e.underline));
    character.position = Some(match &paragraphs {
        Some(p) if pdfcraft_engine::text_script::is_in(p.text, p.selection.as_ref(), Script::Super) => BaselinePosition::Superscript,
        Some(p) if pdfcraft_engine::text_script::is_in(p.text, p.selection.as_ref(), Script::Sub) => BaselinePosition::Subscript,
        _ => BaselinePosition::Normal,
    });
    character.horizontal_scale = Some(extras.map_or(100.0, |e| e.scale as f32));
    character.char_spacing = Some(extras.map_or(look.char_spacing, |e| e.char_spacing) as f32);

    let mut paragraph = ParagraphValues::default();
    let align = paragraphs.as_ref().map_or(look.align, |p| p.shown(look));
    paragraph.align = Some(panel_align(align));
    paragraph.space_after = Some(0.0);

    // A spacing of 0 is the paragraph's own.
    let line_spacing = extras.map(|e| if e.line_spacing == 0.0 { LineSpacing::Auto } else { LineSpacing::Multiple(e.line_spacing as f32) });
    let font = match (target, shown_font) {
        (Target::Paragraph(_), Some(shown)) => {
            if shown == tl!("Keep source fonts") {
                Some(FontPick::KeepSource)
            } else if let Some((resource, _)) = sources.iter().find(|(_, face)| *face == shown) {
                Some(FontPick::Resource(resource.clone()))
            } else {
                STANDARD.into_iter().find(|f| f.label() == shown).map(FontPick::Family)
            }
        }
        (Target::Paragraph(_), None) => None,
        _ => Some(FontPick::Family(look.family)),
    };
    Values { character, paragraph, line_spacing, font }
}

/// The controls `target` can't apply, each with why: the two list buttons and paragraph spacing
/// always; underline, line spacing and scale for added text; superscript and subscript for text
/// that is not open for typing; alignment while a paragraph selection is not tidy.
pub(crate) fn unsupported(target: &Target) -> Vec<(FormatControl, Label)> {
    let planned = || Label::from(tl!("Planned").to_string());
    let mut out =
        vec![(FormatControl::BulletedList, planned()), (FormatControl::NumberedList, planned()), (FormatControl::ParagraphSpacing, planned())];
    let paragraphs = target.paragraphs();
    if target.extras().is_none() {
        out.extend([(FormatControl::Underline, planned()), (FormatControl::LineSpacing, planned()), (FormatControl::HorizontalScale, planned())]);
    }
    if paragraphs.is_none() {
        out.extend([(FormatControl::Superscript, planned()), (FormatControl::Subscript, planned())]);
    }
    if paragraphs.is_some_and(|p| !p.tidy && p.selected().is_some()) {
        out.push((FormatControl::Align, planned()));
    }
    out
}

/// The rows the font picker lists above the installed families, as groups of rows: for a paragraph
/// (`sources` given) `Keep source fonts` and the page's fonts, then the standard three, then
/// `Load font…`, `Reload` and what the installed-font scan found. Added text lists the standard
/// three only. The caller borrows each group's rows into a `HostFontGroup`.
#[cfg(not(target_arch = "wasm32"))]
fn font_groups(sources: Option<&[(String, String)]>, installed: Option<&[pdfcraft_engine::text_fonts::FontFile]>) -> Vec<Vec<HostFontRow>> {
    let mut groups = Vec::new();
    if let Some(sources) = sources {
        let mut page = vec![HostFontRow::action(FontPick::KeepSource.key(), tl!("Keep source fonts").to_string())];
        for (resource, face) in sources.iter().filter(|(r, _)| !r.starts_with("PCApproved")) {
            page.push(HostFontRow::face(FontPick::Resource(resource.clone()).key(), face.clone(), None));
        }
        groups.push(page);
    }
    groups.push(STANDARD.into_iter().map(|f| HostFontRow::face(FontPick::Family(f).key(), f.label(), None)).collect());
    if sources.is_some() {
        let mut more = vec![
            HostFontRow::action(FontPick::Load.key(), tl!("Load font…").to_string()),
            HostFontRow::action(FontPick::Reload.key(), tl!("Reload").to_string()),
        ];
        match installed {
            None => more.push(HostFontRow::notice(tl!("Finding document fonts…").to_string())),
            Some([]) => more.push(HostFontRow::notice(tl!("No openly licensed document fonts found. Load a font file.").to_string())),
            Some(_) => {}
        }
        groups.push(more);
    }
    groups
}

/// The face's style as the picker's style rows name it: the face without its family's name.
#[cfg(not(target_arch = "wasm32"))]
fn face_style(font: &pdfcraft_engine::text_fonts::FontFile) -> &str {
    font.face.strip_prefix(font.family.as_str()).map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&font.face)
}

/// The installed font file of `family`, in `style` when one is given: the face of that style, else
/// the family's plain face, else its first.
#[cfg(not(target_arch = "wasm32"))]
fn pick_installed<'a>(
    files: &'a [pdfcraft_engine::text_fonts::FontFile],
    family: &str,
    style: Option<&str>,
) -> Option<&'a pdfcraft_engine::text_fonts::FontFile> {
    let of_family = || files.iter().filter(|f| f.family == family);
    let wanted = style.and_then(|style| of_family().find(|f| face_style(f) == style || f.face == style));
    wanted.or_else(|| of_family().find(|f| f.face == f.family || face_style(f) == "Regular")).or_else(|| of_family().next())
}

/// Format text for `target`, and its edits applied. The colour swatch's request opens the app's
/// colour picker, whose answer is applied to the same target when it comes. `fmt` and `sources` are
/// the open paragraph's ([`values`]).
pub(crate) fn panel(
    app: &mut PdfCraftApp,
    ui: &mut egui::Ui,
    view: usize,
    target: Target,
    fmt: Option<&SelectionFormatting>,
    sources: &[(String, String)],
) {
    let shown = Section::fixed(tl!("Format text").to_string()).show(ui, |ui, _| {
        #[cfg(not(target_arch = "wasm32"))]
        let installed = if matches!(target, Target::Paragraph(_)) { app.installed_document_fonts(false) } else { None };
        show(
            ui,
            &target,
            fmt,
            sources,
            #[cfg(not(target_arch = "wasm32"))]
            installed,
        )
    });
    let Some(mut out) = shown.inner else { return };
    if let Some(request) = out.requests.take_color() {
        let which = target.recolour();
        app.open_colour_picker(crate::colour_picker::PickerHost::App, request, move |app, ctx, color| recolour(ctx, app, view, which, color));
    }
    apply(ui.ctx(), app, view, target, &out.edits);
}

/// The Format text controls for `target`, shown. The font row is the font picker over the installed
/// families with the page's fonts, the standard three and `Load font…` above them, for a paragraph;
/// otherwise it lists the standard families.
fn show(
    ui: &mut egui::Ui,
    target: &Target,
    fmt: Option<&SelectionFormatting>,
    sources: &[(String, String)],
    #[cfg(not(target_arch = "wasm32"))] installed: Option<&crate::files::InstalledFonts>,
) -> FormatTextOutput {
    let v = values(target, fmt, sources);
    let unsupported = unsupported(target);
    let families: Vec<String> = STANDARD.iter().map(|f| f.label().to_string()).collect();
    let sizes: Vec<f32> = SIZES.iter().map(|v| *v as f32).collect();
    let scales: Vec<f32> = SCALES.iter().map(|v| *v as f32).collect();
    let char_spacings: Vec<f32> = CHAR_SPACINGS.iter().map(|v| *v as f32).collect();
    let mut line_presets = vec![LineSpacing::Auto];
    line_presets.extend(LINE_SPACINGS.iter().map(|v| LineSpacing::Multiple(*v as f32)));
    let range = |r: &RangeInclusive<f64>| *r.start() as f32..=*r.end() as f32;
    let mut text = FormatText::new(tl!("Format text").to_string(), &v.character, &v.paragraph, &families)
        .line_presets(&line_presets)
        .size_presets(&sizes)
        .size_range(range(&SIZE_RANGE))
        .scale_presets(&scales)
        .scale_range(range(&SCALE_RANGE))
        .char_spacing_presets(&char_spacings)
        .char_spacing_range(range(&CHAR_SPACING_RANGE))
        .unsupported(&unsupported);
    // With nothing to set it on the row still reads Auto, as production's does, not Mixed.
    text = text.line_spacing(v.line_spacing.unwrap_or(LineSpacing::Auto));
    #[cfg(not(target_arch = "wasm32"))]
    if matches!(target, Target::Paragraph(_))
        && let Some(installed) = installed
    {
        let rows = font_groups(Some(sources), Some(&installed.files));
        let groups: Vec<HostFontGroup<'_>> = rows.iter().map(|rows| HostFontGroup::new(None, rows)).collect();
        let source = FontPickerSource::new(&installed.catalog).host(&groups).current_host(v.font.as_ref().map(FontPick::key));
        return text.font_picker(source).show(ui);
    }
    // PLAN: pdf-workflows-w13 answers `installed_document_fonts` with an empty catalogue while it scans (lead ruling), so this
    // paragraph branch is always taken and `Finding document fonts…` shows; its `Option` return still has a `None` until then.
    text.show(ui)
}

/// What a step of one change leaves for the caller, beyond the look and spacing it changed in place.
enum Special {
    /// Superscript or Subscript, for the selected paragraphs' text.
    Script(Script),
    /// A size to commit, within [`SIZE_RANGE`].
    Size(f64),
    /// A font picked in the font row.
    Font(FontPick),
    /// An installed family picked (a family not among the standard three).
    Family(String),
    /// A style row picked for the family.
    Style(String),
}

/// One change applied to `look` (and `extras`, for a paragraph): what it sets in place, or what it
/// asks of the caller. A value outside what the engine takes changes nothing.
fn step(look: &mut AddedText, extras: Option<&mut Extras>, paragraphs: Option<&Paragraphs<'_>>, change: &FormatTextChange) -> Option<Special> {
    let finite = |v: f32, range: &RangeInclusive<f64>| {
        let v = f64::from(v);
        v.is_finite().then(|| v.clamp(*range.start(), *range.end()))
    };
    match change {
        FormatTextChange::Character(change) => match change {
            CharacterChange::Size(v) => {
                let v = f64::from(*v);
                (v.is_finite() && SIZE_RANGE.contains(&v)).then_some(Special::Size(v))
            }
            CharacterChange::FauxBold(on) => {
                look.bold = *on;
                None
            }
            CharacterChange::FauxItalic(on) => {
                look.italic = *on;
                None
            }
            CharacterChange::Underline(on) => {
                if let Some(extras) = extras {
                    extras.underline = *on;
                }
                None
            }
            CharacterChange::Position(position) => {
                let paragraphs = paragraphs?;
                let on = |script| pdfcraft_engine::text_script::is_in(paragraphs.text, paragraphs.selection.as_ref(), script);
                match position {
                    BaselinePosition::Superscript => Some(Special::Script(Script::Super)),
                    BaselinePosition::Subscript => Some(Special::Script(Script::Sub)),
                    // Pressing the pressed one: back to plain characters.
                    _ if on(Script::Super) => Some(Special::Script(Script::Super)),
                    _ if on(Script::Sub) => Some(Special::Script(Script::Sub)),
                    _ => None,
                }
            }
            CharacterChange::HorizontalScale(v) => {
                if let (Some(extras), Some(v)) = (extras, finite(*v, &SCALE_RANGE)) {
                    extras.scale = v;
                }
                None
            }
            CharacterChange::CharSpacing(v) => {
                if let Some(v) = finite(*v, &CHAR_SPACING_RANGE) {
                    match extras {
                        Some(extras) => extras.char_spacing = v,
                        None => look.char_spacing = v,
                    }
                }
                None
            }
            CharacterChange::Family(name) => match STANDARD.into_iter().find(|f| f.label() == name) {
                Some(family) => Some(Special::Font(FontPick::Family(family))),
                None => Some(Special::Family(name.clone())),
            },
            CharacterChange::Style(style) => Some(Special::Style(style.clone())),
            _ => None,
        },
        FormatTextChange::HostFont(key) => FontPick::from_key(key).map(Special::Font),
        FormatTextChange::Paragraph(ParagraphChange::Align(align)) => {
            let align = engine_align(*align);
            match paragraphs {
                Some(p) => p.choose(look, align),
                None => {
                    look.align = align;
                    look.aligns.clear();
                }
            }
            None
        }
        FormatTextChange::LineSpacing(spacing) => {
            if let Some(extras) = extras {
                match spacing {
                    LineSpacing::Auto => extras.line_spacing = 0.0,
                    LineSpacing::Multiple(times) if times.is_finite() && *times > 0.0 => extras.line_spacing = f64::from(*times),
                    _ => {}
                }
            }
            None
        }
        _ => None,
    }
}

/// The changes of one pass that take effect: a gesture's last value (`Commit`) and a one-step pick
/// (`Set`). `Begin`, `Update` and `Cancel` preview nothing; production applies on commit.
fn committed(edits: &[EditEvent<FormatTextChange>]) -> Vec<&FormatTextChange> {
    edits
        .iter()
        .filter_map(|edit| match edit {
            EditEvent::Commit { after, .. } | EditEvent::Set { after, .. } => Some(after),
            _ => None,
        })
        .collect()
}

/// Applies the edits of one pass to `target`: the engine edit that follows is one undoable change
/// (`apply_line_format`, `restyle_draft` or an `UpdateContent`), made only when something changed.
/// Only this pass's own edits are applied, so a second pass in a frame applies nothing again.
pub(crate) fn apply(ctx: &egui::Context, app: &mut PdfCraftApp, view: usize, target: Target, edits: &[EditEvent<FormatTextChange>]) {
    let changes = committed(edits);
    if changes.is_empty() {
        return;
    }
    match target {
        Target::Paragraph(mut ed) => {
            if apply_paragraph(ctx, app, &mut ed, &changes) {
                app.apply_line_format(ctx, view, *ed);
            }
        }
        Target::Draft(draft) => {
            let mut style = draft.style.clone();
            let mut toggled = None;
            for change in &changes {
                let paragraphs = Paragraphs {
                    text: &draft.text,
                    selection: draft.selection.as_ref().map(|s| crate::edit_text_ui::selected_bytes(&draft.text, s)),
                    tidy: true,
                };
                match step(&mut style, None, Some(&paragraphs), change) {
                    Some(Special::Size(size)) => style.size = size,
                    Some(Special::Font(FontPick::Family(family))) => style.family = family,
                    Some(Special::Script(script)) => {
                        let mut next = toggled.take().unwrap_or_else(|| (*draft).clone());
                        if crate::edit_text_ui::toggle_script(&mut next.text, &mut next.selection, script) {
                            toggled = Some(next);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(next) = toggled
                && let Some(slot) = app.views.get_mut(view)
            {
                slot.session.content.draft = Some(next);
            }
            if style != draft.style {
                app.restyle_draft(ctx, view, style);
            }
        }
        Target::Added { page, index, text } => {
            let mut style = text.clone();
            for change in &changes {
                match step(&mut style, None, None, change) {
                    Some(Special::Size(size)) => style.size = size,
                    Some(Special::Font(FontPick::Family(family))) => style.family = family,
                    _ => {}
                }
            }
            if style != text {
                // The reusable style follows the selection only when the edit took, so a refused one adds nothing odd later.
                if app.apply_edit(
                    ctx,
                    pdfcraft_engine::Edit::UpdateContent { page, index, content: pdfcraft_engine::AddedContent::Text(style.clone()) },
                ) {
                    app.text_style = AddedText { text: String::new(), rect: [0.0; 4], aligns: Vec::new(), ..style };
                }
            }
        }
        Target::Armed(text) => {
            let mut style = text.clone();
            for change in &changes {
                match step(&mut style, None, None, change) {
                    Some(Special::Size(size)) => style.size = size,
                    Some(Special::Font(FontPick::Family(family))) => style.family = family,
                    _ => {}
                }
            }
            if style != text {
                app.text_style = style;
            }
        }
    }
}

/// The open paragraph's changes applied to its editor; whether anything changed, so the caller
/// applies the paragraph.
fn apply_paragraph(ctx: &egui::Context, app: &mut PdfCraftApp, ed: &mut LineEditor, changes: &[&FormatTextChange]) -> bool {
    let before = (ed.look.clone(), ed.extras);
    let mut changed = false;
    // An installed family and its style row arrive as two changes of the same pass.
    let mut family = None;
    let mut style = None;
    for change in changes {
        let special = {
            let selection = ed.selection.as_ref().map(|s| crate::edit_text_ui::selected_bytes(&ed.text, s));
            let paragraphs = Paragraphs { text: &ed.text, selection, tidy: false };
            step(&mut ed.look, Some(&mut ed.extras), Some(&paragraphs), change)
        };
        match special {
            Some(Special::Size(size)) => {
                ed.choose_size(size);
                changed = true;
            }
            Some(Special::Script(script)) => changed |= crate::edit_text_ui::toggle_script(&mut ed.text, &mut ed.selection, script),
            Some(Special::Font(pick)) => match pick {
                FontPick::KeepSource => {
                    ed.keep_source_font();
                    changed = true;
                }
                FontPick::Resource(resource) => {
                    ed.font_resource = Some(resource);
                    changed = true;
                }
                FontPick::Family(f) => {
                    ed.choose_family(f);
                    changed = true;
                }
                FontPick::Load => app.pick_files(crate::files::FilePurpose::TextFont { page: ed.page }, false),
                FontPick::Reload => {
                    #[cfg(not(target_arch = "wasm32"))]
                    let _ = app.installed_document_fonts(true);
                }
            },
            Some(Special::Family(name)) => family = Some(name),
            Some(Special::Style(name)) => style = Some(name),
            None => {}
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(family) = family {
        let path = app.installed_document_fonts(false).and_then(|i| pick_installed(&i.files, &family, style.as_deref()).map(|f| f.path.clone()));
        if let Some(path) = path {
            app.use_paths(ctx, crate::files::FilePurpose::TextFont { page: ed.page }, &[path]);
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (family, style, ctx);
    changed || (ed.look.clone(), ed.extras) != before
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use super::*;

    /// A change as the panel reports a one-step pick.
    fn set(change: FormatTextChange) -> EditEvent<FormatTextChange> {
        EditEvent::set(craft::behavior::Mixed::Mixed, change, "Format text".into(), None)
    }

    /// A one-page document with a proper xref table.
    fn one_page_pdf() -> Vec<u8> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
            "<< /Type /Page /Parent 2 0 R >>",
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// An app with one added text, "Proof", on page 0.
    fn app_with_text() -> PdfCraftApp {
        let mut app = PdfCraftApp::for_unit_tests();
        app.set_option(&egui::Context::default(), "language", "en").unwrap();
        app.open_bytes(&egui::Context::default(), "a.pdf", None, one_page_pdf()).unwrap();
        let text = AddedText { rect: [20.0, 200.0, 180.0, 220.0], text: "Proof".into(), size: 12.0, ..Default::default() };
        assert!(app.apply_edit(&egui::Context::default(), pdfcraft_engine::Edit::AddText { page: 0, text }));
        app
    }

    /// The added text as the page has it.
    fn item(app: &PdfCraftApp) -> AddedText {
        match &app.session.get(app.views[0].id).unwrap().added()[0].content {
            pdfcraft_engine::AddedContent::Text(t) => t.clone(),
            other => panic!("{other:?}"),
        }
    }

    /// How many changes the document has to undo.
    fn undo_depth(app: &PdfCraftApp) -> usize {
        app.session.get(app.views[0].id).unwrap().undo_depth()
    }

    fn open_draft(app: &mut PdfCraftApp) -> TextDraft {
        let stored = item(app);
        let draft = TextDraft::new(0, Some(0), stored.rect, stored.text.clone(), stored);
        app.views[0].session.content.draft = Some(draft.clone());
        draft
    }

    #[test]
    fn a_size_pick_makes_one_edit_per_target_and_a_size_outside_1_to_500_none() {
        let size = |v: f32| vec![set(FormatTextChange::Character(CharacterChange::Size(v)))];
        for (name, bad) in [("in range", false), ("zero", true), ("too big", true), ("not a number", true)] {
            let value = match name {
                "in range" => 24.0,
                "zero" => 0.0,
                "too big" => 501.0,
                _ => f32::NAN,
            };
            // Selected added text.
            let mut app = app_with_text();
            let depth = undo_depth(&app);
            let text = item(&app);
            apply(&egui::Context::default(), &mut app, 0, Target::Added { page: 0, index: 0, text }, &size(value));
            assert_eq!(item(&app).size, if bad { 12.0 } else { 24.0 }, "added text, {name}");
            assert_eq!(undo_depth(&app), depth + usize::from(!bad), "one edit for added text, {name}");

            // The box being typed.
            let mut app = app_with_text();
            let depth = undo_depth(&app);
            let draft = open_draft(&mut app);
            apply(&egui::Context::default(), &mut app, 0, Target::Draft(Box::new(draft)), &size(value));
            assert_eq!(item(&app).size, if bad { 12.0 } else { 24.0 }, "draft, {name}");
            assert_eq!(undo_depth(&app), depth + usize::from(!bad), "one edit for the draft, {name}");

            // The style the next text gets.
            let mut app = app_with_text();
            let armed = app.text_style.clone();
            apply(&egui::Context::default(), &mut app, 0, Target::Armed(armed.clone()), &size(value));
            assert_eq!(app.text_style.size, if bad { armed.size } else { 24.0 }, "armed, {name}");

            // A preview of a gesture changes nothing; only its commit does.
            let mut app = app_with_text();
            let text = item(&app);
            let update = EditEvent::update(craft::behavior::GestureId::new(1), FormatTextChange::Character(CharacterChange::Size(value)));
            apply(&egui::Context::default(), &mut app, 0, Target::Added { page: 0, index: 0, text }, &[update]);
            assert_eq!(item(&app).size, 12.0, "a preview, {name}");
        }
    }

    #[test]
    fn a_paragraph_family_alignment_pick_follows_the_selection() {
        let at = |text: &'static str, selection: Option<Range<usize>>, tidy| Paragraphs { text, selection, tidy };
        let fresh = || AddedText { align: TextAlign::Left, ..Default::default() };

        // A caret sets the box's alignment and clears the paragraphs' own.
        let mut look = fresh();
        look.aligns = vec![Some(TextAlign::Right)];
        at("one\ntwo", None, false).choose(&mut look, TextAlign::Center);
        assert_eq!((look.align, look.aligns.is_empty()), (TextAlign::Center, true), "a caret aligns the whole box");

        // A selection on an untidy paragraph is refused.
        let mut look = fresh();
        at("one\ntwo", Some(4..7), false).choose(&mut look, TextAlign::Right);
        assert_eq!(look, fresh(), "an edited paragraph's selection changes nothing");

        // A selection on a tidy draft sets only the paragraphs it touches.
        let mut look = fresh();
        let tidy = at("one\ntwo\nthree", Some(4..7), true);
        tidy.choose(&mut look, TextAlign::Right);
        assert_eq!(look.align, TextAlign::Left, "the box's own alignment stays");
        assert_eq!(
            (look.paragraph_align(0), look.paragraph_align(1), look.paragraph_align(2)),
            (TextAlign::Left, TextAlign::Right, TextAlign::Left),
            "only the touched paragraph takes it"
        );
        assert_eq!(tidy.shown(&look), TextAlign::Right, "the buttons show the first selected paragraph's");

        // The same pick through `apply` on a draft with a caret sets the whole box.
        let mut app = app_with_text();
        let draft = open_draft(&mut app);
        let pick = vec![set(FormatTextChange::Paragraph(ParagraphChange::Align(PanelAlign::Center)))];
        apply(&egui::Context::default(), &mut app, 0, Target::Draft(Box::new(draft)), &pick);
        assert_eq!(item(&app).align, TextAlign::Center);
    }
}
