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
    // A click selects a swatch on its press and activates it on its release, and the palette reports both: one
    // click on a colour that is not the current one is a pick on the press and another on the release, which the
    // host then applies twice (two undo steps). A pick is the activation, so one made while the primary button is
    // held is dropped; the release reports the click. Space and Enter pick with no button held.
    let held = ui.input(|i| i.pointer.primary_down());
    // Snap swatch picks back to the exact f64 SWATCHES entry instead of an f32 round-trip.
    let picked =
        out.picked.filter(|_| !held).map(|c| SWATCHES.iter().find(|(_, rgb)| to_srgba(*rgb) == c).map_or_else(|| to_rgb(c), |(_, rgb)| *rgb));
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    /// What a host of the palette keeps: the colour it shows, and every pick the palette reported to it.
    struct Host {
        installed: bool,
        current: Rgb,
        picks: Vec<Rgb>,
        recent: RecentColours,
    }

    fn harness() -> Harness<'static, Host> {
        let state = Host { installed: false, current: SWATCHES[0].1, picks: Vec::new(), recent: RecentColours::default() };
        let mut h = Harness::builder().with_size(egui::vec2(400.0, 300.0)).build_ui_state(
            |ui, host: &mut Host| {
                if !std::mem::replace(&mut host.installed, true) {
                    let spec = craft::ui::InstallSpec::new(craft::host::target::ShellInfo::default(), Arc::new(craft::window::system::NoWindows));
                    craft::ui::shell::embed::install(ui.ctx(), spec, &crate::fonts::ui_fonts(crate::i18n::Lang::EN)).expect("the runtime installs");
                }
                let pick = palette(ui, "Colour", Some(host.current), "test.colour", &host.recent);
                // As the quick bar does: the colour picked is the one shown from then on, and each report is an edit.
                if let Some(c) = pick.picked {
                    host.current = c;
                    host.picks.push(c);
                }
            },
            state,
        );
        h.run_steps(3);
        h
    }

    fn named(name: &str) -> Rgb {
        SWATCHES.iter().find(|(n, _)| *n == name).map(|(_, c)| *c).expect("a named swatch")
    }

    /// A click selects a swatch on its press and activates it on its release. One click is one pick, whether the
    /// colour is a new one or the one already shown (which still recolours the rest of a selection), so a host that
    /// makes an undo step of each pick makes one step for each click.
    #[test]
    fn one_click_on_a_swatch_is_one_pick() {
        let mut h = harness();
        let (other, current) = ("Blue", SWATCHES[0].0);
        assert_ne!(named(other), h.state().current);

        h.get_by_label(other).click();
        h.run_steps(4);
        assert_eq!(h.state().picks, [named(other)], "a click on another colour picks it once");

        h.get_by_label(other).click();
        h.run_steps(4);
        assert_eq!(h.state().picks, [named(other); 2], "a click on the colour shown is a pick, and one");

        h.get_by_label(current).click();
        h.run_steps(4);
        assert_eq!(h.state().picks.len(), 3, "and back again");
    }

    fn press(h: &mut Harness<'static, Host>, at: egui::Pos2, pressed: bool) {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
    }

    /// A press and a release that arrive in one frame (a quick click on a slow frame) are one pick as well.
    #[test]
    fn a_press_and_release_in_one_frame_are_one_pick() {
        let mut h = harness();
        let at = h.get_by_label("Blue").rect().center();
        h.event(egui::Event::PointerMoved(at));
        h.run_steps(2);
        press(&mut h, at, true);
        press(&mut h, at, false);
        h.run_steps(4);
        assert_eq!(h.state().picks, [named("Blue")]);
    }

    /// A press that is dragged off the swatch and released elsewhere is no click, so it picks nothing; and a screen
    /// reader's click, which has no held button, still picks once.
    #[test]
    fn a_cancelled_press_picks_nothing_and_a_screen_reader_click_picks_once() {
        let mut h = harness();
        let at = h.get_by_label("Blue").rect().center();
        h.event(egui::Event::PointerMoved(at));
        h.run_steps(2);
        press(&mut h, at, true);
        h.run_steps(1);
        let off = egui::pos2(390.0, 290.0);
        h.event(egui::Event::PointerMoved(off));
        h.run_steps(1);
        press(&mut h, off, false);
        h.run_steps(4);
        assert!(h.state().picks.is_empty(), "{:?}", h.state().picks);

        h.get_by_label("Blue").click_accesskit();
        h.run_steps(4);
        assert_eq!(h.state().picks, [named("Blue")]);
    }
}
