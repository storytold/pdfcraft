//! The ArtCraft mark and wordmarks, drawn from the SVGs in `docs/brand/` exactly as shipped (AGENTS.md §1.2: the
//! logos are used unmodified). The pictures come from the framework's SVG source, which bounds the decode and draws
//! nothing for bytes it cannot read.

use craft::ui::contract::Show as _;
use craft::ui::display::{Picture, PictureSize};

const MARK: &[u8] = include_bytes!("../../../docs/brand/artcraft-mark.svg");

/// The square ArtCraft mark at `size`.
pub(crate) fn mark(ui: &mut egui::Ui, size: PictureSize) {
    let _ = Picture::svg("ArtCraft".to_string(), MARK).extent(size).show(ui);
}
