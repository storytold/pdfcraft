//! The format bar: Format text and Edit image on a task bar over the pages, instead of a section that opens above the
//! Edit panel's tools and moves them. It shows for the paragraph open for editing, the box of added text being typed,
//! the selected added text or image, and Add text armed, at the top of the view, clear of the text around them; its
//! grip moves it and its More menu pins it.
//!
//! The bar shows the common controls; All text formatting opens the panel's own Format text over it, so a control
//! Format text gains reaches the bar too. Its changes are Format text's ([`FormatTextChange`]), applied by the same
//! [`format_text_ui::apply`](crate::format_text_ui::apply). Hide bar, in its More menu, puts Format text back in the
//! panel, whose Show on page brings the bar back.

use std::sync::Arc;

use craft::behavior::overlay::Side;
use craft::behavior::{EditEvent, IconId, Mixed};
use craft::pages::ui::PageBody;
use craft::text::ui::character::{
    COLOR_KEY, CharacterChange, CharacterWords, FormatControl, FormatTextChange, FormatTextWords, ParagraphChange, ParagraphWords,
    TextAlign as PanelAlign,
};
use craft::theme::Srgba;
use craft::ui::buttons::{IconButton, PopupMark, ToggleButton};
use craft::ui::canvas_controls::{TaskBar, TaskBarDismiss, TaskBarPlace};
use craft::ui::choice::Swatch;
use craft::ui::contract::{Bind, Entry, PopupKind, Props, Show as _, TooltipSpec, Trigger};
use craft::ui::display::Divider;
use craft::ui::menu::Menu;
use craft::ui::pickers::Select;
use craft::ui::runtime::env::Env;
use craft::ui::runtime::env::Label;
use craft::ui::runtime::requests::ColorRequest;

use crate::content_ui::ImageAction;
use crate::format_text_ui::{self, Target};
use crate::panels::edit::{Formatting, format_target};
use crate::{PdfCraftApp, QuickTool};

use super::PageSession;

/// The format bar's preferences: hidden from its More menu, and where it was moved or pinned.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct FormatBarPrefs {
    /// Hide bar was picked: Format text and Edit image show in the Edit panel instead.
    #[serde(default)]
    pub hidden: bool,
    /// The bar's place: following what it formats until moved, or pinned.
    #[serde(default)]
    pub place: TaskBarPlace,
}

/// What the page body reads to show the bar.
pub(crate) struct FormatBarCx<'a> {
    pub prefs: FormatBarPrefs,
    /// The style Add text gives the next text.
    pub text_style: &'a pdfcraft_engine::AddedText,
    /// The installed fonts the paragraph's font picker lists; `None` while they are being found.
    #[cfg(not(target_arch = "wasm32"))]
    pub installed: Option<&'a crate::files::InstalledFonts>,
}

/// What the bar did in a pass, for the app to apply after the pages.
pub(crate) struct FormatBarOut {
    shown: Formatting,
    edits: Vec<EditEvent<FormatTextChange>>,
    colour: Option<ColorRequest>,
    image: Option<ImageAction>,
    place: TaskBarPlace,
    hide: bool,
}

/// One pick of a control: a change made at once, as Format text's menus and toggles report one.
fn picked(change: FormatTextChange, name: &str) -> EditEvent<FormatTextChange> {
    EditEvent::set(Mixed::Mixed, change, Arc::from(name), None)
}

/// `component`, named `name`, disabled for `reason` when one is given, the reason in its tooltip, as Format text's own
/// controls are.
fn disabled_for<P: Props>(component: P, name: &Label, reason: Option<&Label>) -> P {
    match reason {
        Some(reason) => component.enabled(false).tooltip(TooltipSpec::new(name.clone()).disabled_text(reason.clone())),
        None => component,
    }
}

fn mixed<T>(value: Option<T>) -> Mixed<T> {
    value.map_or(Mixed::Mixed, Mixed::Same)
}

/// The bar for what the view formats, over the pages; what it did, or `None` with nothing to format or the bar hidden.
pub(super) fn show(
    ui: &mut egui::Ui,
    body: &PageBody<'_>,
    session: &PageSession,
    doc: &pdfcraft_engine::Document,
    tool: QuickTool,
    cx: &FormatBarCx<'_>,
) -> Option<FormatBarOut> {
    if cx.prefs.hidden {
        return None;
    }
    let shown = format_target(session, doc, tool, cx.text_style)?;
    // At the top of the view, below the middle of its top edge: clear of the text being formatted and of the text
    // around it, wherever that is.
    let top = egui::Rect::from_center_size(egui::pos2(body.rect.center().x, body.rect.top()), egui::Vec2::splat(1.0));
    let anchor = [top.left_top(), top.right_top(), top.right_bottom(), top.left_bottom()].map(|p| body.view.to_doc(p));
    let mut place = cx.prefs.place;
    let mut edits = Vec::new();
    let mut colour = None;
    let mut image = None;
    let mut controls = |ui: &mut egui::Ui| match &shown {
        Formatting::Text { target, fmt, sources, hint } => {
            text_controls(ui, target, fmt.as_ref(), sources, *hint, cx, &mut edits, &mut colour);
        }
        Formatting::Image { img, .. } => image = image_controls(ui, img),
    };
    let label = match shown {
        Formatting::Image { .. } => tl!("Edit image"),
        Formatting::Text { .. } => tl!("Format text"),
    };
    // Its own id scope: the box being typed shows another task bar, Done and Discard, in the same pass.
    let bar_out = ui
        .push_id("format-bar", |ui| {
            let bar = TaskBar::new(label, &anchor, Bind::bound(&mut place)).side(Side::Below).dismiss(TaskBarDismiss::Keep);
            bar.show(ui, body.rect, |ui, _| controls(ui)).output
        })
        .inner;
    // Kept where it first shows: its width changes with what it formats (the hint, the image tools), and centred it
    // would shift sideways each time. Reset bar position centres it again.
    if place.at.is_none() && bar_out.rect.is_positive() {
        let offset = bar_out.rect.min - body.rect.min;
        place.at = Some([offset.x, offset.y]);
    }
    let hide = bar_out.hide;
    Some(FormatBarOut { shown, edits, colour, image, place, hide })
}

/// Font, size, bold, italic, underline, colour, alignment and All text formatting for `target`, in one row. Their
/// changes go to `edits`; the colour swatch's request for the colour picker to `colour`.
#[allow(clippy::too_many_arguments)]
fn text_controls(
    ui: &mut egui::Ui,
    target: &Target,
    fmt: Option<&crate::edit_text_ui::SelectionFormatting>,
    sources: &[(String, String)],
    hint: bool,
    cx: &FormatBarCx<'_>,
    edits: &mut Vec<EditEvent<FormatTextChange>>,
    colour: &mut Option<ColorRequest>,
) {
    let v = format_text_ui::values(target, fmt, sources);
    let unsupported = format_text_ui::unsupported(target);
    let reason = |control: FormatControl| unsupported.iter().find(|(c, _)| *c == control).map(|(_, why)| why);
    // Format text's own words, so each control has the panel's name.
    let env = Env::of(ui);
    let (words, cw, pw) = (env.words::<FormatTextWords>(), env.words::<CharacterWords>(), env.words::<ParagraphWords>());
    // The standard families; the open paragraph's own fonts are in All text formatting's font picker.
    let families: Vec<Entry<String>> = format_text_ui::STANDARD.iter().map(|f| Entry::new(f.label().to_string(), f.label())).collect();
    let family = disabled_for(
        Select::new(cw.family.clone(), Bind::given(mixed(v.character.family.clone())), &families),
        &cw.family,
        reason(FormatControl::Font),
    )
    .show(ui);
    edits.extend(family.events.map(|name: String| FormatTextChange::Character(CharacterChange::Family(name))));
    let sizes: Vec<Entry<f32>> = format_text_ui::SIZES.iter().map(|s| Entry::new(*s as f32, format!("{s} pt"))).collect();
    let size =
        disabled_for(Select::new(cw.size.clone(), Bind::given(mixed(v.character.size)), &sizes), &cw.size, reason(FormatControl::Size)).show(ui);
    edits.extend(size.events.map(|s: f32| FormatTextChange::Character(CharacterChange::Size(s))));
    let _ = Divider::vertical().show(ui);
    let toggles = [
        (&words.bold, "text_bold", v.character.faux_bold, FormatControl::Bold),
        (&words.italic, "text_italic", v.character.faux_italic, FormatControl::Italic),
        (&cw.underline, "text_underline", v.character.underline, FormatControl::Underline),
    ];
    for (name, icon, on, control) in toggles {
        let toggle = ToggleButton::new(name.clone(), Bind::given(mixed(on))).icon(IconId::new(icon)).icon_only();
        let toggle = disabled_for(toggle, name, reason(control)).show(ui);
        edits.extend(toggle.events.map(|on: bool| {
            FormatTextChange::Character(match control {
                FormatControl::Bold => CharacterChange::FauxBold(on),
                FormatControl::Italic => CharacterChange::FauxItalic(on),
                _ => CharacterChange::Underline(on),
            })
        }));
    }
    let _ = Divider::vertical().show(ui);
    let swatch = Swatch::new(cw.text_color.clone(), mixed(v.character.color.map(Srgba::from))).picker(COLOR_KEY);
    let swatch = disabled_for(swatch, &cw.text_color, reason(FormatControl::Color)).show(ui);
    *colour = colour.take().or(swatch.pick_color);
    let aligns = [
        (PanelAlign::Left, "align_left", &pw.left),
        (PanelAlign::Center, "align_center", &pw.center),
        (PanelAlign::Right, "align_right", &pw.right),
        (PanelAlign::JustifyLeft, "align_justify", &pw.justify_left),
    ];
    let current = v.paragraph.align;
    let icon = aligns.iter().find(|(a, ..)| Some(*a) == current).map_or("align_left", |(_, icon, _)| icon);
    let entries: Vec<Entry<PanelAlign>> =
        aligns.iter().map(|&(align, icon, name)| Entry::new(align, name.clone()).icon(IconId::new(icon)).checked(current == Some(align))).collect();
    let opener = IconButton::new(IconId::new(icon)).label(pw.alignment.clone()).popup(PopupKind::Menu).popup_mark(PopupMark::Menu);
    let opener = disabled_for(opener, &pw.alignment, reason(FormatControl::Align)).show(ui).response;
    let align = Menu::new(pw.alignment.clone()).values().opened_by(&opener, Trigger::Press).pick(ui, &entries);
    if let Some(align) = align.picked {
        edits.push(picked(FormatTextChange::Paragraph(ParagraphChange::Align(align)), pw.alignment.as_str()));
    }
    // Everything else Format text has, in its own layout, over the page.
    crate::comments::popover_from(
        ui,
        tl!("All text formatting"),
        |ui| IconButton::new(IconId::new("adjustments")).label(tl!("All text formatting")).show(ui).response,
        |ui| {
            let mut out = format_text_ui::controls(
                ui,
                target,
                fmt,
                sources,
                #[cfg(not(target_arch = "wasm32"))]
                cx.installed,
            );
            *colour = colour.take().or_else(|| out.requests.take_color());
            edits.append(&mut out.edits);
            false
        },
    );
    if hint {
        let tip = tl!(
            "Click on the page to add text; each click starts a new box, and ✓ finishes. Drag items to move them, drag a corner to resize, double-click text to edit it."
        );
        let _ = IconButton::new(IconId::new("info")).label(tip).show(ui);
    }
}

/// Rotate, flip and replace a selected added image, and its crop in a popover.
fn image_controls(ui: &mut egui::Ui, img: &pdfcraft_engine::AddedImage) -> Option<ImageAction> {
    let mut out = crate::content_ui::image_buttons(ui, img);
    let _ = Divider::vertical().show(ui);
    crate::comments::popover_from(
        ui,
        tl!("Crop (% trimmed from each side)"),
        |ui| IconButton::new(IconId::new("crop")).label(tl!("Crop (% trimmed from each side)")).show(ui).response,
        |ui| {
            out = out.take().or_else(|| crate::content_ui::crop_fields(ui, img));
            false
        },
    );
    out
}

/// Applies what the bar did in view `view`: its place, Hide bar, the colour picker it asked for and its edits.
pub(crate) fn apply(app: &mut PdfCraftApp, ctx: &egui::Context, view: usize, out: FormatBarOut) {
    app.format_bar.place = out.place;
    if out.hide {
        app.format_bar.hidden = true;
    }
    match out.shown {
        Formatting::Text { target, .. } => {
            if let Some(request) = out.colour {
                format_text_ui::pick_colour(app, view, &target, request);
            }
            format_text_ui::apply(ctx, app, view, target, &out.edits);
        }
        Formatting::Image { page, index, .. } => match out.image {
            Some(ImageAction::Update(content)) => {
                app.apply_edit(ctx, pdfcraft_engine::Edit::UpdateContent { page, index, content });
            }
            Some(ImageAction::Replace) => app.replace_image_dialog(page, index),
            None => {}
        },
    }
}
