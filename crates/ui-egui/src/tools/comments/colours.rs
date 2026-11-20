//! The comment colour rows: the named swatches as the framework palette takes them, the session's
//! recent custom colours, and the "no fill" / "same as text" / form fields' "no color" choices above a palette.

use std::sync::{Arc, PoisonError, RwLock};

use craft::behavior::Mixed;
use craft::color_ui::ui::SwatchPalette;
use craft::theme::Srgba;
use craft::ui::buttons::ToggleButton;
use craft::ui::contract::{Bind, Show as _};
use craft::ui::runtime::env::Label;
use craft::ui::runtime::requests::ColorRequest;

use super::{Rgb, SWATCHES};

/// How many custom colours the "Recent colours" row keeps.
const RECENT_CUSTOM: usize = 5;

/// How far apart two colours' components may be and still count as the same colour.
const SAME_WITHIN: f64 = 0.02;

/// The named swatches in the palette's own types, named in the current language.
pub(crate) fn swatches() -> Vec<(Label, Srgba)> {
    SWATCHES.iter().map(|(name, c)| (Label::from(tl!(name).to_string()), to_srgba(*c))).collect()
}

/// A colour in the palette's float type; components outside 0..=1 are clamped.
pub(crate) fn to_srgba(c: Rgb) -> Srgba {
    let [r, g, b] = c.map(|v| v.clamp(0.0, 1.0) as f32);
    Srgba::rgb(r, g, b)
}

/// A palette colour as the engine's colour; alpha is dropped.
pub(crate) fn to_rgb(c: Srgba) -> Rgb {
    [c.r, c.g, c.b].map(|v| f64::from(v.clamp(0.0, 1.0)))
}

/// Whether two colours are the same one, give or take the rounding a picker adds.
pub(crate) fn same_colour(a: &Rgb, b: &Rgb) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < SAME_WITHIN)
}

/// Custom colours picked with "Other colour…" this session, newest first. A handle: clones share the
/// list, so a dialog's colour rows, which cannot borrow the app, show what the app remembers.
#[derive(Clone, Default)]
pub(crate) struct RecentColours(Arc<RwLock<Vec<Rgb>>>);

impl RecentColours {
    /// Puts `c` first (once, at most five). A colour that is a swatch, or not a finite colour, is not kept.
    pub(crate) fn remember(&self, c: Rgb) {
        if !c.iter().all(|v| v.is_finite()) || SWATCHES.iter().any(|(_, s)| same_colour(s, &c)) {
            return;
        }
        let c = c.map(|v| v.clamp(0.0, 1.0));
        let mut recent = self.0.write().unwrap_or_else(PoisonError::into_inner);
        recent.retain(|r| !same_colour(r, &c));
        recent.insert(0, c);
        recent.truncate(RECENT_CUSTOM);
    }

    /// The remembered colours, newest first, as the palette takes them.
    pub(crate) fn as_srgba(&self) -> Vec<Srgba> {
        self.0.read().unwrap_or_else(PoisonError::into_inner).iter().copied().map(to_srgba).collect()
    }
}

/// What a palette reported this pass.
#[derive(Default)]
pub(crate) struct PalettePick {
    /// The colour of a swatch or recent colour that was clicked, also the one already current.
    pub(crate) picked: Option<Rgb>,
    /// The Other… cell was activated: the host opens its colour picker for this request.
    pub(crate) request: Option<ColorRequest>,
}

/// The swatches, an Other… cell keyed `key` and the recent colours, with `current` marked (none marks no
/// swatch). Every click on a swatch is reported, so a click on the colour a lead comment has still colours
/// the others of a selection.
pub(crate) fn palette(ui: &mut egui::Ui, label: &str, current: Option<Rgb>, key: &str, recent: &RecentColours) -> PalettePick {
    let mut shown: Mixed<Option<Srgba>> = Mixed::Same(current.map(to_srgba));
    let named = swatches();
    let recent = recent.as_srgba();
    let out = SwatchPalette::optional(Label::from(label.to_string()), Bind::bound_mixed(&mut shown), &named)
        .other(key, tl!("Other colour…"))
        .recent(tl!("Recent colours"), &recent)
        .show(ui);
    // Snap swatch picks back to the exact f64 SWATCHES entry instead of an f32 round-trip.
    let picked = out.picked.map(|c| SWATCHES.iter().find(|(_, rgb)| to_srgba(*rgb) == c).map_or_else(|| to_rgb(c), |(_, rgb)| *rgb));
    PalettePick { picked, request: out.pick_color }
}

/// What a none-or-colour choice reported this pass.
#[derive(Default)]
pub(crate) struct ColourChoice {
    /// `Some(None)`: the "none" row was chosen; `Some(Some(c))`: a colour.
    pub(crate) choice: Option<Option<Rgb>>,
    /// The Other… cell was activated.
    pub(crate) request: Option<ColorRequest>,
}

fn choice(ui: &mut egui::Ui, none_label: &str, current: Option<Rgb>, key: &str, recent: &RecentColours) -> ColourChoice {
    let was = current.is_none();
    let mut on = was;
    ToggleButton::new(none_label.to_string(), Bind::bound(&mut on)).show(ui);
    let pick = palette(ui, tl!("Colour"), current, key, recent);
    // Turning the row off does nothing: it stands for no colour, which only another pick replaces.
    let choice = if on && !was { Some(None) } else { pick.picked.map(Some) };
    ColourChoice { choice, request: pick.request }
}

/// "No fill" above the palette.
pub(crate) fn fill_choice(ui: &mut egui::Ui, current: Option<Rgb>, key: &str, recent: &RecentColours) -> ColourChoice {
    choice(ui, tl!("No fill"), current, key, recent)
}

/// "Same as text" above the palette, for a border that takes the text's colour unless picked.
pub(crate) fn border_choice(ui: &mut egui::Ui, current: Option<Rgb>, key: &str, recent: &RecentColours) -> ColourChoice {
    choice(ui, tl!("Same as text"), current, key, recent)
}

/// "No color" above the palette, for a form field's border or fill: on, the field has none; off, it is black
/// until a colour is picked. Fields that differ (`mixed`) show it off, so it can be turned on for all of them.
pub(crate) fn field_choice(ui: &mut egui::Ui, current: Option<Rgb>, mixed: bool, key: &str, recent: &RecentColours) -> ColourChoice {
    let was = !mixed && current.is_none();
    let mut none = was;
    ToggleButton::new(tl!("No color").to_string(), Bind::bound(&mut none)).show(ui);
    let pick = palette(ui, tl!("Colour"), current, key, recent);
    let choice = if none != was { Some((!none).then_some([0.0; 3])) } else { pick.picked.map(Some) };
    ColourChoice { choice, request: pick.request }
}
