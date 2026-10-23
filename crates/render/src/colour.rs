//! Image samples converted to sRGB the way pages draw them.
//!
//! The renderer converts an image's colours to sRGB as it draws the page. Image export
//! (`pdfcraft-create`) converts the samples it writes out through the same conversion, here,
//! so an exported image shows the page's colours instead of those of a second, simpler
//! formula (#2269). Bootstrap: hayro's colour conversion (vendored hayro-interpret patch 51),
//! replaced together with the renderer (ADR-0004).

use std::panic::{AssertUnwindSafe, catch_unwind};

use hayro::hayro_interpret::color::{IccToSrgb, device_cmyk_to_srgb};

/// 8-bit DeviceCMYK samples, four a pixel, as sRGB, three a pixel, converted as pages draw
/// DeviceCMYK. `None` when `cmyk` doesn't hold whole pixels.
pub fn cmyk_to_srgb(cmyk: &[u8]) -> Option<Vec<u8>> {
    if !cmyk.len().is_multiple_of(4) {
        return None;
    }
    let mut rgb = vec![0; cmyk.len() / 4 * 3];
    catch_unwind(AssertUnwindSafe(|| device_cmyk_to_srgb(cmyk, &mut rgb))).ok().flatten()?;
    Some(rgb)
}

/// An `ICCBased` colour space's conversion of 8-bit samples to sRGB, as pages draw it.
#[derive(Clone, Debug)]
pub struct IccProfile {
    conversion: IccToSrgb,
    components: usize,
}

impl IccProfile {
    /// The conversion through `profile` (the ICC stream's data) for samples of `components`
    /// (1, 3 or 4) components. `None` when the renderer can't read the profile for that many
    /// components; pages then draw the colour space's `/Alternate`.
    pub fn new(profile: &[u8], components: usize) -> Option<Self> {
        let conversion = catch_unwind(AssertUnwindSafe(|| IccToSrgb::new(profile, components))).ok().flatten()?;
        Some(Self { conversion, components })
    }

    /// Whether pages draw the samples as they are: an RGB profile marked as sRGB.
    pub fn is_srgb(&self) -> bool {
        self.conversion.is_srgb()
    }

    /// `samples` (8-bit, the profile's components a pixel) as sRGB, three a pixel. `None` when
    /// they aren't whole pixels.
    pub fn to_srgb(&self, samples: &[u8]) -> Option<Vec<u8>> {
        // `new` succeeded, so `components` is 1, 3 or 4.
        if !samples.len().is_multiple_of(self.components) {
            return None;
        }
        let mut rgb = vec![0; samples.len() / self.components * 3];
        catch_unwind(AssertUnwindSafe(|| self.conversion.convert(samples, &mut rgb))).ok().flatten()?;
        Some(rgb)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{PageRenderer, RenderConfig, RenderRequest};

    /// The CMYK profile the renderer draws DeviceCMYK with: a real ICC profile, to tag an image.
    const CMYK_PROFILE: &[u8] = include_bytes!("../../../vendor/hayro-interpret/assets/CGATS001Compat-v2-micro.icc");

    /// One page, 40 × 10 pt, showing a 4 × 1 image in `space` with `samples` (and `extra`, an
    /// ICC profile stream as object 6, when given).
    fn image_page(space: &str, samples: &[u8], extra: Option<&[u8]>) -> Vec<u8> {
        let content = "q 40 0 0 10 0 0 cm /Im Do Q";
        let mut pdf = format!(
            "%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 10] /Contents 4 0 R /Resources << /XObject << /Im 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /XObject /Subtype /Image /Width 4 /Height 1 /ColorSpace {space} /BitsPerComponent 8 /Length {} >> stream
",
            content.len(),
            samples.len()
        )
        .into_bytes();
        pdf.extend_from_slice(samples);
        pdf.extend_from_slice(b"\nendstream endobj\n");
        if let Some(profile) = extra {
            pdf.extend(format!("6 0 obj << /N 4 /Length {} >> stream\n", profile.len()).bytes());
            pdf.extend_from_slice(profile);
            pdf.extend_from_slice(b"\nendstream endobj\n");
        }
        pdf.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF");
        pdf
    }

    /// The colours the page draws its image's four samples in: the centres of their squares.
    fn drawn(pdf: Vec<u8>) -> Vec<u8> {
        let out = PageRenderer::new(Arc::new(pdf), RenderConfig::default()).render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
        assert!(out.error.is_none(), "{:?}", out.error);
        (0..4).flat_map(|i| out.rgba.get((5 * 40 + 10 * i + 5) * 4..).unwrap()[..3].to_vec()).collect()
    }

    /// Cyan, magenta, black and a mixed ink.
    const INKS: [u8; 16] = [255, 0, 0, 0, 0, 255, 0, 0, 0, 0, 0, 255, 40, 120, 200, 30];

    #[test]
    fn cmyk_samples_convert_to_the_colours_pages_draw() {
        let page = drawn(image_page("/DeviceCMYK", &INKS, None));
        let rgb = cmyk_to_srgb(&INKS).unwrap();
        assert_eq!(rgb, page);
        // Not the naive (1 − c)(1 − k) formula: cyan ink isn't pure (0, 255, 255).
        assert_ne!(&rgb[..3], &[0, 255, 255]);
        assert_eq!(cmyk_to_srgb(&INKS[..6]), None);
        assert_eq!(cmyk_to_srgb(&[]), Some(Vec::new()));
    }

    #[test]
    fn icc_samples_convert_to_the_colours_pages_draw() {
        let page = drawn(image_page("[/ICCBased 6 0 R]", &INKS, Some(CMYK_PROFILE)));
        let profile = IccProfile::new(CMYK_PROFILE, 4).unwrap();
        assert!(!profile.is_srgb());
        assert_eq!(profile.to_srgb(&INKS).unwrap(), page);
        // Partial pixels, a profile of other components and damaged profiles are refused.
        assert_eq!(profile.to_srgb(&INKS[..6]), None);
        assert!(IccProfile::new(CMYK_PROFILE, 3).is_none());
        assert!(IccProfile::new(&CMYK_PROFILE[..200], 4).is_none());
        assert!(IccProfile::new(b"not a profile", 3).is_none());
        assert!(IccProfile::new(CMYK_PROFILE, 7).is_none());
    }
}
