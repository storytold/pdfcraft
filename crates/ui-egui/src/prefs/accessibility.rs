//! Preferences ▸ Accessibility: whether anything animates. Reading colours (Acrobat's Replace
//! Document Colors) are in Appearance, below the interface theme (#2627).

use super::{card, radios};
use crate::PdfCraftApp;
use crate::theme::Tokens;

pub(super) fn show(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    // Whether anything animates, the wheel's scrolling and zooming included; left alone it
    // follows the system's animation setting.
    card(ui, t, Some(tl!("Animation effects")), |ui| {
        radios(ui, &mut app.animation_effects, crate::motion::AnimationEffects::ALL.map(|c| (c, tl_ctx!("animation effects", c.label()))));
    });
}
