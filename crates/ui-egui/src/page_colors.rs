//! Display colours shared by page textures and live drawing previews.

use egui::Color32;
use pdfcraft_render::{DarkImages, DarkPages, DarkPalette, PRINT_TAG, ReadingColors};

/// The dark-page palettes with their English labels, for the menus.
pub(crate) const DARK_PALETTES: [(DarkPalette, &str); 3] =
    [(DarkPalette::Soft, "Soft dark"), (DarkPalette::Black, "Black"), (DarkPalette::Sepia, "Sepia")];
/// What dark pages do with images, with their English labels, for the menus.
pub(crate) const DARK_IMAGES: [(DarkImages, &str); 3] =
    [(DarkImages::KeepOpaque, "Keep opaque images"), (DarkImages::KeepAll, "Keep all images"), (DarkImages::Recolour, "Recolour images")];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PageColors {
    #[default]
    Original,
    /// Every pixel filtered into the palette (images too).
    Dark(DarkPalette),
    /// The renderer already applies this preset to raster pixels.
    Reading(ReadingColors),
}

impl PageColors {
    /// An explicit reading-colour preset wins; the saved dark-page choice (`dark`, when on)
    /// resumes when it is off. Dark pages that keep their images are drawn by the renderer
    /// through the same remapping as a preset, so they map as one.
    pub(crate) fn for_reading(dark: Option<DarkPages>, reading: ReadingColors) -> Self {
        match dark {
            _ if reading.params.is_some() => Self::Reading(reading),
            Some(style) if style.renderer_draws() => Self::Reading(style.reading_colors()),
            Some(style) => Self::Dark(style.palette),
            None => Self::Original,
        }
    }

    pub(crate) fn for_request(dark: Option<DarkPages>, reading: ReadingColors, tag: u64) -> Self {
        if tag & PRINT_TAG != 0 { Self::Original } else { Self::for_reading(dark, reading) }
    }

    pub(crate) fn raster(self, color: Color32) -> Color32 {
        if let Self::Dark(palette) = self { dark_page_color(color, palette) } else { color }
    }

    /// Map an unrendered drawing stroke, using the renderer's own preset mapping.
    pub(crate) fn line_art(self, color: Color32) -> Color32 {
        match self {
            Self::Reading(reading) => {
                let [r, g, b, a] = reading.map_line_art(color.to_srgba_unmultiplied());
                Color32::from_rgba_unmultiplied(r, g, b, a)
            }
            _ => self.raster(color),
        }
    }

    /// Map live text independently of the preset's line-art opt-out.
    pub(crate) fn text(self, color: Color32) -> Color32 {
        match self {
            Self::Reading(reading) => {
                let [r, g, b, a] = reading.map_text(color.to_srgba_unmultiplied());
                Color32::from_rgba_unmultiplied(r, g, b, a)
            }
            _ => self.raster(color),
        }
    }

    /// Raw images keep their colours under reading presets; dark pages filter all pixels. A run
    /// of equal pixels (a page's paper) is mapped once.
    pub(crate) fn image(self, mut image: egui::ColorImage) -> egui::ColorImage {
        if matches!(self, Self::Dark(_)) {
            let mut last: Option<(Color32, Color32)> = None;
            for pixel in &mut image.pixels {
                let mapped = match last {
                    Some((from, to)) if from == *pixel => to,
                    _ => {
                        let to = self.raster(*pixel);
                        last = Some((*pixel, to));
                        to
                    }
                };
                *pixel = mapped;
            }
        }
        image
    }

    /// A decoded image drawn over the page (an image signature being placed or moved), as the
    /// renderer draws it: filtered on dark pages that recolour images; with transparency, mapped
    /// as ink where dark pages keep only opaque images; otherwise as it is.
    pub(crate) fn decoded_image(self, mut image: egui::ColorImage) -> egui::ColorImage {
        match self {
            Self::Dark(_) => self.image(image),
            Self::Reading(reading) if reading.params.is_some_and(|p| p.keep_hue && p.masked_images) => {
                if image.pixels.iter().any(|p| p.a() < 255) {
                    for pixel in &mut image.pixels {
                        let [r, g, b, a] = reading.map_line_art(pixel.to_srgba_unmultiplied());
                        *pixel = Color32::from_rgba_unmultiplied(r, g, b, a);
                    }
                }
                image
            }
            _ => image,
        }
    }

    /// Editor surfaces follow the page paper even when a preset only replaces black paints.
    pub(crate) fn editor_paper(self, original: Color32) -> Color32 {
        if matches!(self, Self::Reading(_)) {
            let paper = self.paper();
            Color32::from_rgba_unmultiplied(paper.r(), paper.g(), paper.b(), original.a())
        } else {
            self.raster(original)
        }
    }

    pub(crate) fn paper(self) -> Color32 {
        match self {
            Self::Reading(ReadingColors { params: Some(p) }) => Color32::from_rgb(p.background[0], p.background[1], p.background[2]),
            _ => self.raster(Color32::WHITE),
        }
    }

    pub(crate) fn loading_color(self, fallback: Color32) -> Color32 {
        match self {
            Self::Reading(ReadingColors { params: Some(p) }) => Color32::from_rgb(p.text[0], p.text[1], p.text[2]),
            Self::Dark(_) => Color32::from_gray(190),
            _ => fallback,
        }
    }
}

/// Reading filter that keeps hues recognisable while exchanging light and dark luminance, into
/// `palette`'s paper and ink.
fn dark_page_color(pixel: Color32, palette: DarkPalette) -> Color32 {
    let [r, g, b, a] = pdfcraft_render::dark_page_rgba_in(pixel.to_srgba_unmultiplied(), palette);
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Dark pages that filter every pixel, for the tests of that mode (keeping images is the default).
#[cfg(test)]
pub(crate) const FILTERING: DarkPages = DarkPages { palette: DarkPalette::Soft, images: DarkImages::Recolour };

#[cfg(test)]
pub(crate) fn uploaded_textures(ctx: &egui::Context, textures: &[&egui::TextureHandle]) -> Vec<Vec<Color32>> {
    let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
    let mut uploads = std::mem::take(&mut output.textures_delta.set);
    // Assert only after clearing deltas: egui's Drop asserts too, even during unwinding.
    output.textures_delta.clear();
    textures
        .iter()
        .map(|texture| {
            let deltas = uploads.remove(&texture.id()).expect("texture upload");
            match &deltas.last().expect("image delta").image {
                egui::ImageData::Color(image) => image.pixels.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn uploaded_pixels(ctx: &egui::Context, texture: &egui::TextureHandle) -> Vec<Color32> {
    uploaded_textures(ctx, &[texture]).pop().expect("one requested texture")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_texture_upload_panics_once_without_aborting_cleanup() {
        let ctx = egui::Context::default();
        let old = ctx.load_texture("old", egui::ColorImage::filled([1, 1], Color32::WHITE), egui::TextureOptions::LINEAR);
        assert_eq!(uploaded_pixels(&ctx, &old), [Color32::WHITE]);
        let _pending = ctx.load_texture("pending", egui::ColorImage::filled([1, 1], Color32::BLACK), egui::TextureOptions::LINEAR);
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| uploaded_pixels(&ctx, &old)));
        assert!(failure.is_err());
        let mut output = ctx.run_ui(egui::RawInput::default(), |_| {});
        output.textures_delta.clear();
    }

    #[test]
    fn dark_images_map_every_pixel_as_the_raster_mapping_does() {
        let pixels = [Color32::WHITE, Color32::WHITE, Color32::BLACK, Color32::from_rgb(200, 30, 40), Color32::WHITE, Color32::TRANSPARENT];
        let image = egui::ColorImage::new([3, 2], pixels.to_vec());
        let mapped = PageColors::Dark(DarkPalette::Soft).image(image.clone());
        let expected: Vec<Color32> = pixels.iter().map(|p| PageColors::Dark(DarkPalette::Soft).raster(*p)).collect();
        assert_eq!(mapped.pixels, expected);
        assert_eq!(PageColors::Original.image(image.clone()).pixels, image.pixels, "light pages are left alone");
        let reading = PageColors::Reading(ReadingColors::WHITE_ON_BLACK);
        assert_eq!(reading.image(image.clone()).pixels, image.pixels, "the renderer already mapped a preset");
    }

    #[test]
    fn decoded_images_follow_what_the_renderer_does_with_images() {
        let signature = || egui::ColorImage::new([2, 1], vec![Color32::BLACK, Color32::TRANSPARENT]);
        let photo = || egui::ColorImage::new([2, 1], vec![Color32::BLACK, Color32::WHITE]);
        let ink = DarkPalette::Soft.ink();
        let style = |images| PageColors::for_reading(Some(DarkPages { palette: DarkPalette::Soft, images }), ReadingColors::default());
        let opaque = style(DarkImages::KeepOpaque);
        assert_eq!(opaque.decoded_image(signature()).pixels[0].to_array()[..3], ink, "a signature with transparency becomes light ink");
        assert_eq!(opaque.decoded_image(photo()).pixels, photo().pixels, "an opaque image is kept");
        assert_eq!(style(DarkImages::KeepAll).decoded_image(signature()).pixels, signature().pixels, "every image kept");
        assert_eq!(
            style(DarkImages::Recolour).decoded_image(photo()).pixels[1],
            style(DarkImages::Recolour).raster(Color32::WHITE),
            "every pixel filtered"
        );
        assert_eq!(PageColors::Original.decoded_image(signature()).pixels, signature().pixels);
        let preset = PageColors::for_reading(Some(DarkPages::default()), ReadingColors::WHITE_ON_BLACK);
        assert_eq!(preset.decoded_image(signature()).pixels, signature().pixels, "a reading preset keeps images");
    }

    #[test]
    fn dark_styles_pick_the_filter_or_the_renderers_remapping() {
        for palette in DarkPalette::ALL {
            let filter = DarkPages { palette, images: DarkImages::Recolour };
            let colors = PageColors::for_reading(Some(filter), ReadingColors::default());
            assert_eq!(colors, PageColors::Dark(palette));
            assert_eq!(colors.paper().to_array()[..3], palette.paper(), "{palette:?}: paper");
            assert_eq!(colors.raster(Color32::BLACK).to_array()[..3], palette.ink(), "{palette:?}: ink");
            let keep = DarkPages { palette, images: DarkImages::KeepOpaque };
            let colors = PageColors::for_reading(Some(keep), ReadingColors::default());
            assert_eq!(colors, PageColors::Reading(keep.reading_colors()), "the renderer draws it");
            assert_eq!(colors.raster(Color32::RED), Color32::RED, "rendered pixels, images included, are not mapped again");
            assert_eq!(colors.paper().to_array()[..3], palette.paper());
            assert_eq!(colors.text(Color32::BLACK).to_array()[..3], palette.ink(), "live text takes the ink");
            assert_eq!(PageColors::for_reading(Some(keep), ReadingColors::YELLOW_ON_BLACK), PageColors::Reading(ReadingColors::YELLOW_ON_BLACK));
            assert_eq!(PageColors::for_request(Some(keep), ReadingColors::default(), PRINT_TAG), PageColors::Original);
        }
        assert_eq!(PageColors::for_reading(None, ReadingColors::default()), PageColors::Original);
    }

    #[test]
    fn reading_presets_take_precedence_and_prints_always_use_original_colours() {
        for reading in [ReadingColors::WHITE_ON_BLACK, ReadingColors::YELLOW_ON_BLACK, ReadingColors::BLACK_ON_WHITE] {
            let colors = PageColors::for_reading(Some(FILTERING), reading);
            assert_eq!(colors.raster(Color32::BLACK), Color32::BLACK, "already rendered pixels are not mapped twice");
            assert_eq!(colors.line_art(Color32::BLACK).to_array()[..3], reading.params.unwrap().text);
            assert_eq!(colors.paper().to_array()[..3], reading.params.unwrap().background);
            assert_eq!(PageColors::for_request(Some(FILTERING), reading, PRINT_TAG), PageColors::Original);
        }
        assert_eq!(PageColors::for_reading(Some(FILTERING), ReadingColors::default()), PageColors::Dark(DarkPalette::Soft));
        let flags = ReadingColors::parse("white-on-black", None, None, Some(true), Some(false)).unwrap();
        let colors = PageColors::for_reading(Some(FILTERING), flags);
        assert_eq!(colors.line_art(Color32::BLACK), Color32::BLACK, "honour line-art opt-out");
        assert_eq!(colors.text(Color32::BLACK), Color32::WHITE, "text is still mapped");
        assert_eq!(colors.editor_paper(Color32::WHITE), Color32::BLACK, "editor paper follows the preset even with only-black and line-art opt-out");
        let flags = ReadingColors::parse("white-on-black", None, None, Some(true), Some(true)).unwrap();
        assert_eq!(PageColors::for_reading(Some(FILTERING), flags).line_art(Color32::RED), Color32::RED, "honour only-black mapping");
    }
}
