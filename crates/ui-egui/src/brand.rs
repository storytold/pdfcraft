//! The ArtCraft mark and wordmarks, drawn from the SVGs in `docs/brand/` exactly as shipped (AGENTS.md §1.2: the
//! logos are used unmodified). The pictures come from the framework's SVG source, which bounds the decode and draws
//! nothing for bytes it cannot read.

// PLAN: `mod brand;` is declared in lib.rs (pdf-hub-w1); the About dialog calls these (pdf-hub-w2).

use craft::theme::Mode;
use craft::ui::display::{Picture, PictureSize};
use craft::ui::list::ThumbAspect;
use craft::ui::runtime::env::Env;

const MARK: &[u8] = include_bytes!("../../../docs/brand/artcraft-mark.svg");
const LOGO: &[u8] = include_bytes!("../../../docs/brand/artcraft-logo.svg");
const LOGO_WHITE: &[u8] = include_bytes!("../../../docs/brand/artcraft-logo-white.svg");

/// The square ArtCraft mark at `size`.
pub(crate) fn mark(ui: &mut egui::Ui, size: PictureSize) {
    let _ = Picture::svg("ArtCraft".to_string(), MARK).extent(size).show(ui);
}

/// The ArtCraft wordmark at `size`, in the white variant on a dark theme.
pub(crate) fn logo(ui: &mut egui::Ui, size: PictureSize) {
    let bytes = if Env::of(ui).mode() == Mode::Dark { LOGO_WHITE } else { LOGO };
    // The wordmarks' view box is 1038.05 x 108.
    let _ = Picture::svg("ArtCraft".to_string(), bytes).aspect(ThumbAspect::new(1038, 108)).extent(size).show(ui);
}
