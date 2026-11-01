//! Image samples converted to sRGB the way pages draw them.
//!
//! The renderer converts an image's colours to sRGB as it draws the page. Image export
//! (`pdfcraft-create`) converts the samples it writes out through the same conversion, here,
//! so an exported image shows the page's colours instead of those of a second, simpler
//! formula (#2269). Bootstrap: hayro's colour conversion (vendored hayro-interpret patches 71
//! and 72), replaced together with the renderer (ADR-0004).

use std::panic::{AssertUnwindSafe, catch_unwind};

use hayro::hayro_interpret::color::{CieToSrgb, IccToSrgb, device_cmyk_to_srgb};

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

/// A `CalGray`, `CalRGB` or `Lab` colour space's conversion to sRGB, as pages draw it. The
/// constructors take the colour space dictionary's entries, `None` for one that is missing or
/// isn't as many numbers as it should be (pages then use its default).
#[derive(Clone, Debug)]
pub struct CieSpace(CieToSrgb);

impl CieSpace {
    /// `CalGray`: its `/WhitePoint`, `/BlackPoint` and `/Gamma`.
    pub fn cal_gray(white_point: Option<[f32; 3]>, black_point: Option<[f32; 3]>, gamma: Option<f32>) -> Self {
        Self(CieToSrgb::cal_gray(white_point, black_point, gamma))
    }

    /// `CalRGB`: its `/WhitePoint`, `/BlackPoint`, `/Gamma` and `/Matrix`.
    pub fn cal_rgb(white_point: Option<[f32; 3]>, black_point: Option<[f32; 3]>, gamma: Option<[f32; 3]>, matrix: Option<[f32; 9]>) -> Self {
        Self(CieToSrgb::cal_rgb(white_point, black_point, gamma, matrix))
    }

    /// `Lab`: its `/WhitePoint` and `/Range`.
    pub fn lab(white_point: Option<[f32; 3]>, range: Option<[f32; 4]>) -> Self {
        Self(CieToSrgb::lab(white_point, range))
    }

    /// The components of a colour: 1 for `CalGray`, 3 for `CalRGB` and `Lab`.
    pub fn components(&self) -> usize {
        self.0.components()
    }

    /// An image's 8-bit samples (the space's components a pixel, decoded as with the default
    /// `/Decode`: for `Lab`, onto L\* 0–100 and the a\* and b\* `/Range`) as sRGB, three a pixel.
    /// `None` when they aren't whole pixels.
    pub fn to_srgb(&self, samples: &[u8]) -> Option<Vec<u8>> {
        self.convert(samples, |input, output| self.0.convert(input, output))
    }

    /// The entries of an `/Indexed` palette over this space (its lookup table's bytes) as sRGB,
    /// three an entry, the way pages look them up. `None` when they aren't whole entries.
    pub fn palette_to_srgb(&self, entries: &[u8]) -> Option<Vec<u8>> {
        self.convert(entries, |input, output| self.0.convert_palette(input, output))
    }

    /// One colour, its components as the PDF writes them (a soft mask's `/Matte`), in sRGB.
    /// `None` when they aren't one colour's.
    pub fn colour_to_srgb(&self, components: &[f32]) -> Option<[u8; 3]> {
        catch_unwind(AssertUnwindSafe(|| self.0.convert_colour(components))).ok().flatten()
    }

    fn convert(&self, input: &[u8], convert: impl FnOnce(&[u8], &mut [u8]) -> Option<()>) -> Option<Vec<u8>> {
        // At least one component, so the division is defined.
        let n = self.components().max(1);
        if !input.len().is_multiple_of(n) {
            return None;
        }
        let mut rgb = vec![0; input.len() / n * 3];
        catch_unwind(AssertUnwindSafe(|| convert(input, &mut rgb))).ok().flatten()?;
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

    const D65: Option<[f32; 3]> = Some([0.9505, 1.0, 1.089]);
    const D50: Option<[f32; 3]> = Some([0.9642, 1.0, 0.8249]);

    #[test]
    fn calibrated_and_lab_samples_convert_to_the_colours_pages_draw() {
        let rgb = [200, 60, 30, 0, 128, 255, 255, 255, 255, 10, 20, 30];
        for (space, cie, samples) in [
            ("[/CalGray << /WhitePoint [0.9505 1 1.089] >>]", CieSpace::cal_gray(D65, None, None), &[0, 90, 200, 255][..]),
            (
                "[/CalGray << /WhitePoint [0.9642 1 0.8249] /BlackPoint [0.01 0.01 0.01] /Gamma 2.2 >>]",
                CieSpace::cal_gray(D50, Some([0.01; 3]), Some(2.2)),
                &[0, 90, 200, 255],
            ),
            (
                "[/CalRGB << /WhitePoint [0.9505 1 1.089] /Gamma [1.8 1.8 1.8] /Matrix [0.4497 0.2446 0.0252 0.3163 0.672 0.1412 0.1845 0.0833 0.9227] >>]",
                CieSpace::cal_rgb(D65, None, Some([1.8; 3]), Some([0.4497, 0.2446, 0.0252, 0.3163, 0.672, 0.1412, 0.1845, 0.0833, 0.9227])),
                &rgb,
            ),
            ("[/CalRGB << /WhitePoint [0.9642 1 0.8249] /BlackPoint [0.02 0.02 0.02] >>]", CieSpace::cal_rgb(D50, Some([0.02; 3]), None, None), &rgb),
            ("[/Lab << /WhitePoint [0.9505 1 1.089] >>]", CieSpace::lab(D65, None), &rgb),
            ("[/Lab << /WhitePoint [0.9642 1 0.8249] /Range [-128 127 -128 127] >>]", CieSpace::lab(D50, Some([-128.0, 127.0, -128.0, 127.0])), &rgb),
        ] {
            assert_eq!(cie.to_srgb(samples).unwrap(), drawn(image_page(space, samples, None)), "{space}");
            if cie.components() > 1 {
                assert_eq!(cie.to_srgb(&samples[..cie.components() + 1]), None, "{space}: a partial pixel");
            }
        }
    }

    #[test]
    fn palettes_over_calibrated_and_lab_spaces_convert_to_the_colours_pages_draw() {
        // The first three Lab entries come out a level apart from the same bytes as image samples:
        // a palette decodes them in other float steps (one colour in about 100,000 does).
        let lab = CieSpace::lab(D50, Some([-128.0, 127.0, -128.0, 127.0]));
        let lab_table = [18, 234, 249, 76, 150, 30, 128, 105, 165, 200, 60, 30];
        assert_ne!(lab.palette_to_srgb(&lab_table), lab.to_srgb(&lab_table));
        for (space, cie, table) in [
            (
                "[/CalRGB << /WhitePoint [0.9505 1 1.089] /Gamma [2.2 2.2 2.2] >>]",
                CieSpace::cal_rgb(D65, None, Some([2.2; 3]), None),
                [200, 60, 30, 0, 128, 255, 255, 255, 255, 10, 20, 30],
            ),
            ("[/Lab << /WhitePoint [0.9642 1 0.8249] /Range [-128 127 -128 127] >>]", lab, lab_table),
        ] {
            let hex: String = table.iter().map(|b| format!("{b:02X}")).collect();
            let page = drawn(image_page(&format!("[/Indexed {space} 3 <{hex}>]"), &[0, 1, 2, 3], None));
            assert_eq!(cie.palette_to_srgb(&table).unwrap(), page, "{space}");
        }
    }

    #[test]
    fn calibrated_and_lab_colours_convert_from_their_components() {
        // The ends of each component's range decode exactly: a colour given by its components
        // converts as the samples for it do.
        let lab = CieSpace::lab(D50, Some([-128.0, 127.0, -128.0, 127.0]));
        assert_eq!(lab.colour_to_srgb(&[100.0, -128.0, 127.0]).map(Vec::from), lab.to_srgb(&[255, 0, 255]));
        // L*, a* and b* as written, not fractions of their ranges: L* 100 with no a* or b* is white.
        let white = lab.colour_to_srgb(&[100.0, 0.0, 0.0]).unwrap();
        assert!(white.iter().all(|&c| c >= 250), "{white:?}");
        let gray = CieSpace::cal_gray(D65, None, Some(2.2));
        assert_eq!(gray.colour_to_srgb(&[1.0]).map(Vec::from), gray.to_srgb(&[255]));
        assert_eq!(lab.colour_to_srgb(&[50.0]), None);
        assert_eq!(gray.colour_to_srgb(&[]), None);
    }

    #[test]
    fn hostile_calibrated_and_lab_entries_convert_without_panicking() {
        // Every triple of these, which also makes whole pixels of one component.
        let ends = [0, 1, 127, 128, 254, 255];
        let samples: Vec<u8> = ends.iter().flat_map(|&a| ends.iter().flat_map(move |&b| ends.map(|c| [a, b, c]))).flatten().collect();
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX, f32::MIN_POSITIVE] {
            for cie in [
                CieSpace::cal_gray(Some([bad; 3]), Some([bad; 3]), Some(bad)),
                CieSpace::cal_rgb(Some([bad; 3]), Some([bad; 3]), Some([bad; 3]), Some([bad; 9])),
                CieSpace::cal_rgb(D65, Some([1.0; 3]), None, Some([bad; 9])),
                CieSpace::lab(Some([bad; 3]), Some([bad; 4])),
                CieSpace::lab(D65, Some([100.0, -100.0, bad, -bad])),
            ] {
                assert_eq!(cie.to_srgb(&samples).map(|rgb| rgb.len()), Some(samples.len() / cie.components() * 3), "{bad}");
                assert!(cie.palette_to_srgb(&samples).is_some(), "{bad}");
                assert!(cie.colour_to_srgb(&[bad; 3][..cie.components()]).is_some(), "{bad}");
            }
        }
    }
}
