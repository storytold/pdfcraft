/*!
A crate for rendering PDF files.

This crate allows you to render pages of a PDF file into bitmaps. It is supposed to be relatively
lightweight, since we do not have any dependencies on the GPU. All the rendering happens on the CPU.

The ultimate goal of this crate is to be a *feature-complete* and *performant* PDF rasterizer.
With that said, we are currently still very far away from reaching that goal: So far, no effort
has been put into performance optimizations, as we are still working on implementing missing features.
However, this crate is currently the most comprehensive and feature-complete
implementation of a PDF rasterizer in pure Rust. This claim is supported by the fact that we currently
include over 1000 PDF files in our regression test suite. The majority of those have been scraped
from the `pdf.js` and `PDFBOX` test suites and therefore represent a very large and diverse sample
of PDF files.

As mentioned, there are still some serious limitations, including lack of support for
encrypted/password-protected PDF files, blending and isolation, knockout groups as well as a range
of smaller features such as color key masking. But you should be able to render the vast majority
of PDF files without too many issues.

## Safety
This crate forbids unsafe code via a crate-level attribute.

## Examples
For usage examples, see the [example](https://github.com/LaurenzV/hayro/tree/master/hayro/examples) in
the GitHub repository.

## Cargo features
This crate has one optional feature:
- `embed-fonts`: See the description of [`hayro-interpret`](https://docs.rs/hayro-interpret/latest/hayro_interpret/#cargo-features) for more information.
*/

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use crate::renderer::Renderer;
use hayro_interpret::Device;
use hayro_interpret::FillRule;
use hayro_interpret::InterpreterCache;
use hayro_interpret::InterpreterSettings;
use hayro_interpret::hayro_syntax::Pdf;
use hayro_interpret::hayro_syntax::page::Page;
use hayro_interpret::util::{RectExt, TransformExt};
use hayro_interpret::{BlendMode, Context};
use hayro_interpret::{ClipPath, interpret_page};
use kurbo::{Affine, Rect, Shape};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::rc::Rc;

pub use hayro_interpret;
pub use hayro_interpret::hayro_syntax;
pub use vello_cpu;

use vello_cpu::color::AlphaColor;
use vello_cpu::color::Srgb;
use vello_cpu::color::palette::css::TRANSPARENT;
use vello_cpu::color::palette::css::WHITE;
use vello_cpu::{Level, Pixmap, RenderMode};

mod renderer;
/// PdfCraft patch: exported for its regression test.
pub use renderer::{image_resampling_size, tiling_cell_scale, usable_cell_scale};
/// PdfCraft patch: exported for its regression test.
#[doc(hidden)]
pub use renderer::set_skip_offscreen_paths;

/// A cache used by the renderer.
///
/// Ideally, such a cache should be constructed once per PDF and then reused across
/// multiple render invocations on the same document.
#[derive(Clone, Default)]
pub struct RenderCache<'a> {
    pub(crate) interpreter_cache: InterpreterCache<'a>,
    pub(crate) outline_cache: Rc<RefCell<HashMap<u128, Rc<kurbo::BezPath>>>>,
}

impl<'a> RenderCache<'a> {
    /// Create a new render cache.
    pub fn new() -> Self {
        Self {
            interpreter_cache: InterpreterCache::new(),
            outline_cache: Rc::new(RefCell::new(HashMap::new())),
        }
    }
}

/// PdfCraft patch: replacement colours for the "reading colours" view setting (Acrobat's
/// "Replace Document Colors"). Solid paints are mapped by luminance: dark paints move towards
/// `text`, light paints towards `background`. Images, shadings and patterns never pass through
/// the mapping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorReplacement {
    /// The sRGB colour that dark paints (low luminance) are mapped towards.
    pub text: [u8; 3],
    /// The sRGB colour that light paints (high luminance) are mapped towards.
    pub background: [u8; 3],
    /// When `true`, paints with a luminance above 10% are left unchanged, so only near-black
    /// text and line art is recoloured.
    pub only_black: bool,
    /// When `true`, line-art paints are mapped in addition to text paints.
    pub line_art: bool,
}

impl ColorReplacement {
    /// Maps a solid paint colour. `is_text` distinguishes glyph fills/strokes from
    /// plain path art. Alpha is copied verbatim. Never panics.
    pub fn map(&self, rgba8: [u8; 4], is_text: bool) -> [u8; 4] {
        if !is_text && !self.line_art {
            return rgba8;
        }
        let [r, g, b, a] = rgba8;
        // Luminance of the gamma-encoded sRGB bytes, normalized to `0.0..=1.0`.
        let t = (0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)) / 255.0;
        if self.only_black && t > 0.10 {
            return rgba8;
        }
        let [text_r, text_g, text_b] = self.text;
        let [background_r, background_g, background_b] = self.background;
        let channel = |text_ch: u8, background_ch: u8| {
            let mixed = f32::from(text_ch) * (1.0 - t) + f32::from(background_ch) * t;
            // The mixture of two channel values with weights summing to one stays within
            // `0.0..=255.0` up to float noise; the clamp keeps the cast total anyway.
            mixed.round().clamp(0.0, 255.0) as u8
        };
        [
            channel(text_r, background_r),
            channel(text_g, background_g),
            channel(text_b, background_b),
            a,
        ]
    }
}

/// Settings to apply during rendering.
#[derive(Clone, Copy)]
pub struct RenderSettings {
    /// How much the contents should be scaled into the x direction.
    pub x_scale: f32,
    /// How much the contents should be scaled into the y direction.
    pub y_scale: f32,
    /// The width of the viewport. If this is set to `None`, the width will be chosen
    /// automatically based on the scale factor and the dimensions of the PDF.
    pub width: Option<u16>,
    /// The height of the viewport. If this is set to `None`, the height will be chosen
    /// automatically based on the scale factor and the dimensions of the PDF.
    pub height: Option<u16>,
    /// The background color. Determines the color of the base
    /// rectangle during rendering to a pixmap.
    pub bg_color: AlphaColor<Srgb>,
    /// PdfCraft patch: top-left corner of the rendered region, in device pixels of the scaled
    /// page. Together with `width`/`height` this renders one tile of a large page.
    pub x_offset: f32,
    /// See `x_offset`.
    pub y_offset: f32,
    /// PdfCraft patch: when set, solid text and line-art paint colours are remapped while
    /// rendering; `None` renders exactly as without the patch.
    pub color_replacement: Option<ColorReplacement>,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            x_scale: 1.0,
            y_scale: 1.0,
            width: None,
            height: None,
            bg_color: TRANSPARENT,
            x_offset: 0.0,
            y_offset: 0.0,
            color_replacement: None,
        }
    }
}

/// Render the page with the given settings to a pixmap.
pub fn render<'a>(
    page: &'a Page<'a>,
    cache: &RenderCache<'a>,
    interpreter_settings: &InterpreterSettings,
    render_settings: &RenderSettings,
) -> Pixmap {
    let (pix_width, pix_height) = render_size(page, render_settings);
    let mut pixmap = Pixmap::new(pix_width, pix_height);
    render_into(
        page,
        cache,
        interpreter_settings,
        render_settings,
        pixmap.data_as_u8_slice_mut(),
    );
    pixmap
}

/// PdfCraft patch: the size in pixels of what [`render`] produces with these settings.
pub fn render_size(page: &Page<'_>, render_settings: &RenderSettings) -> (u16, u16) {
    let (width, height) = page.render_dimensions();
    let (scaled_width, scaled_height) = (
        (width * render_settings.x_scale) as f64,
        (height * render_settings.y_scale) as f64,
    );
    (
        render_settings.width.unwrap_or(scaled_width.floor() as u16),
        render_settings
            .height
            .unwrap_or(scaled_height.floor() as u16),
    )
}

/// PdfCraft patch: [`render`] into `buffer`, premultiplied RGBA8 rows of [`render_size`]
/// pixels, so the caller chooses the allocation (one aligned to whole pixels can then become
/// texture data without a copy). `buffer.len()` must be width × height × 4; any other length
/// leaves `buffer` untouched (vello_cpu asserts on the length, and a panic must not be reachable).
pub fn render_into<'a>(
    page: &'a Page<'a>,
    cache: &RenderCache<'a>,
    interpreter_settings: &InterpreterSettings,
    render_settings: &RenderSettings,
    buffer: &mut [u8],
) {
    let (x_scale, y_scale) = (render_settings.x_scale, render_settings.y_scale);
    let initial_transform = Affine::translate((-render_settings.x_offset as f64, -render_settings.y_offset as f64))
        * Affine::scale_non_uniform(x_scale as f64, y_scale as f64)
        * page.initial_transform(true).to_kurbo();

    let (pix_width, pix_height) = render_size(page, render_settings);
    let expected = (pix_width as usize)
        .checked_mul(pix_height as usize)
        .and_then(|n| n.checked_mul(4));
    if expected != Some(buffer.len()) {
        return;
    }
    let mut state = Context::new(
        initial_transform,
        Rect::new(0.0, 0.0, pix_width as f64, pix_height as f64),
        &cache.interpreter_cache,
        page.xref(),
        interpreter_settings.clone(),
    );

    let vc_settings = vello_cpu::RenderSettings {
        level: Level::new(),
        num_threads: 0,
        render_mode: RenderMode::OptimizeSpeed,
    };

    let mut device = Renderer::new(
        pix_width,
        pix_height,
        vc_settings,
        cache,
        render_settings.color_replacement,
        interpreter_settings.preview,
    );

    // PdfCraft patch (18): the page renders as one transparency group (the page group). Its
    // objects composite with an initially transparent backdrop, and the group is then put on
    // the paper (ISO 32000-2, page group), so a blend mode only blends where something was
    // drawn: a /BM /Screen fill over an empty part of the page keeps its colour (#2970).
    // Under reading colours the background is painted *inside* the group instead, as its
    // initial backdrop, so a Multiply highlight blends with the dark background: outside it,
    // the highlight kept its own flat colour and the words under it vanished into the band
    // (#1548). Reading colours replace the page's colours anyway; for document colours the
    // background stays outside, or every separable mode but Normal, Multiply, Darken and
    // HardLight would blend with the white paper.
    let backdrop_inside = render_settings.color_replacement.is_some();
    if backdrop_inside {
        device.push_transparency_group(1.0, None, BlendMode::Normal);
    }

    device.ctx.set_paint(render_settings.bg_color);
    device
        .ctx
        .fill_rect(&Rect::new(0.0, 0.0, pix_width as f64, pix_height as f64));
    let mut clip_path = page.intersected_crop_box().to_kurbo().to_path(0.1);
    clip_path.apply_affine(initial_transform);
    device.push_clip_path(&ClipPath {
        path: clip_path,
        fill: FillRule::NonZero,
    });
    if !backdrop_inside {
        device.push_transparency_group(1.0, None, BlendMode::Normal);
    }

    interpret_page(page, &mut state, &mut device);

    if !backdrop_inside {
        device.pop_transparency_group();
    }
    device.pop_clip_path();
    if backdrop_inside {
        device.pop_transparency_group();
    }

    let mut resources = vello_cpu::Resources::default();
    device.ctx.render_to_buffer(
        &mut resources,
        buffer,
        pix_width,
        pix_height,
        vc_settings.render_mode,
    );
}

// Just a convenience method for testing.
#[doc(hidden)]
pub fn render_pdf(
    pdf: &Pdf,
    scale: f32,
    settings: InterpreterSettings,
    color_replacement: Option<ColorReplacement>,
    range: Option<RangeInclusive<usize>>,
) -> Option<Vec<Pixmap>> {
    let cache = RenderCache::new();
    let rendered = pdf
        .pages()
        .iter()
        .enumerate()
        .flat_map(|(idx, page)| {
            if range.clone().is_some_and(|range| !range.contains(&idx)) {
                return None;
            }

            let pixmap = render(
                page,
                &cache,
                &settings,
                &RenderSettings {
                    x_scale: scale,
                    y_scale: scale,
                    bg_color: WHITE,
                    color_replacement,
                    ..Default::default()
                },
            );

            Some(pixmap)
        })
        .collect();

    Some(rendered)
}

pub(crate) fn derive_settings(settings: &vello_cpu::RenderSettings) -> vello_cpu::RenderSettings {
    vello_cpu::RenderSettings {
        num_threads: 0,
        ..*settings
    }
}

// PdfCraft patch: unit tests for the reading-colours mapping.
#[cfg(test)]
mod color_replacement_tests {
    use super::ColorReplacement;

    fn config(
        text: [u8; 3],
        background: [u8; 3],
        only_black: bool,
        line_art: bool,
    ) -> ColorReplacement {
        ColorReplacement {
            text,
            background,
            only_black,
            line_art,
        }
    }

    #[test]
    fn black_maps_to_text_colour() {
        let r = config([0x12, 0x34, 0x56], [0xff, 0xff, 0xff], false, true);
        assert_eq!(r.map([0x00, 0x00, 0x00, 0xff], true), [0x12, 0x34, 0x56, 0xff]);
    }

    #[test]
    fn white_maps_to_background() {
        let r = config([0x00, 0x00, 0x00], [0xab, 0xcd, 0xef], false, true);
        assert_eq!(r.map([0xff, 0xff, 0xff, 0xff], true), [0xab, 0xcd, 0xef, 0xff]);
    }

    #[test]
    fn midgrey_interpolates() {
        // 0x80 grey between black text and a white background lands near 0x80.
        let r = config([0x00, 0x00, 0x00], [0xff, 0xff, 0xff], false, true);
        let [mapped_r, mapped_g, mapped_b, mapped_a] = r.map([0x80, 0x80, 0x80, 0xff], true);
        assert!(mapped_r.abs_diff(0x80) <= 2);
        assert!(mapped_g.abs_diff(0x80) <= 2);
        assert!(mapped_b.abs_diff(0x80) <= 2);
        assert_eq!(mapped_a, 0xff);
    }

    #[test]
    fn alpha_copied_verbatim() {
        let r = config([0xff, 0x00, 0x00], [0x00, 0x00, 0xff], false, true);
        assert_eq!(r.map([0x00, 0x00, 0x00, 137], true), [0xff, 0x00, 0x00, 137]);
        assert_eq!(r.map([0xff, 0xff, 0xff, 0], true), [0x00, 0x00, 0xff, 0]);
    }

    #[test]
    fn only_black_threshold() {
        // White text on black: a mapped grey v becomes 255 - v, so mapped and skipped
        // values are distinguishable.
        let r = config([0xff, 0xff, 0xff], [0x00, 0x00, 0x00], true, true);
        // Luminance t = 25/255 ≈ 0.098 is just under the 0.10 threshold: mapped.
        assert_eq!(r.map([25, 25, 25, 255], true), [230, 230, 230, 255]);
        // Luminance t = 27/255 ≈ 0.106 is just over it: left unchanged.
        assert_eq!(r.map([27, 27, 27, 255], true), [27, 27, 27, 255]);
    }

    #[test]
    fn line_art_false_skips_paths_but_not_text() {
        let r = config([0xff, 0xff, 0xff], [0x00, 0x00, 0x00], false, false);
        // A plain path fill is left unchanged when line art is off.
        assert_eq!(r.map([0x00, 0x00, 0x00, 255], false), [0x00, 0x00, 0x00, 255]);
        // Text fills/strokes still map.
        assert_eq!(r.map([0x00, 0x00, 0x00, 255], true), [0xff, 0xff, 0xff, 255]);
    }
}

// PdfCraft patch: end-to-end test for the reading-colours renderer hook.
#[cfg(test)]
mod color_replacement_render_tests {
    use super::{ColorReplacement, InterpreterSettings, Pdf, Pixmap, render_pdf};
    use vello_cpu::color::{AlphaColor, Srgb};

    /// A one-page 200×100 pt PDF with a white background rect, black Helvetica text,
    /// a solid red `rg` rect, a CMYK-filled rect and a 4×4 uncompressed `/DeviceRGB`
    /// image. Byte-level object layout follows `fixture()` in
    /// `crates/automation/tests/automation.rs`.
    fn fixture() -> Vec<u8> {
        let body = "1 1 1 rg 0 0 200 100 re f\n\
                    0 0 0 rg BT /F1 24 Tf 20 70 Td (HI) Tj ET\n\
                    1 0 0 rg 20 20 50 40 re f\n\
                    1 0 0 0 k 100 20 50 40 re f\n\
                    q 20 0 0 20 160 20 cm /Im0 Do Q";
        // 16 opaque pure-blue pixels; blue's low luminance makes an (incorrect) mapping
        // immediately visible as a light grey.
        let image: Vec<u8> = (0..16).flat_map(|_| [0x00_u8, 0x00, 0xff]).collect();

        let mut objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 100] >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> \
              /XObject << /Im0 6 0 R >> >> >>"
                .to_vec(),
            format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes(),
        ];
        let mut image_obj = format!(
            "<< /Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceRGB \
             /BitsPerComponent 8 /Length {} >>\nstream\n",
            image.len()
        )
        .into_bytes();
        image_obj.extend_from_slice(&image);
        image_obj.extend_from_slice(b"\nendstream");
        objs.push(image_obj);

        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objs.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    fn replacement(line_art: bool) -> ColorReplacement {
        ColorReplacement {
            text: [0xff, 0xff, 0xff],
            background: [0x00, 0x00, 0x00],
            only_black: false,
            line_art,
        }
    }

    /// A one-page 100×50 pt PDF whose only content is a 4×4 `/ImageMask` stencil (CCITT-style
    /// line art) painted with the current fill colour, black. Each row's four samples are
    /// `0011`: under `/Decode [0 1]` a sample value of 0 marks the painted area, so the left
    /// half of the 40×40 pt image paints and the right half does not.
    fn stencil_fixture() -> Vec<u8> {
        let body = "0 0 0 rg q 40 0 0 40 30 5 cm /Im0 Do Q";
        // Four rows, one byte each (a 4-wide row uses the top nibble): samples 0,0,1,1 —
        // under Decode [0 1] the zeros paint, so the left half of every row is ink and the
        // right half is unpainted.
        let mask: [u8; 4] = [0x30, 0x30, 0x30, 0x30];

        let objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 50] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>"
                .to_vec(),
            format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes(),
            {
                let mut o = format!(
                    "<< /Type /XObject /Subtype /Image /Width 4 /Height 4 /ImageMask true \
                     /BitsPerComponent 1 /Decode [0 1] /Length {} >>\nstream\n",
                    mask.len()
                )
                .into_bytes();
                o.extend_from_slice(&mask);
                o.extend_from_slice(b"\nendstream");
                o
            },
        ];

        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objs.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    /// Renders the stencil fixture the way the app's render crate drives hayro for a
    /// white-on-black user: the page clears to `bg` (`RenderSettings.bg_color`) while the
    /// solid paints go through `color_replacement`. `render_pdf` cannot express the mapped
    /// clear colour (it always clears white).
    fn render_stencil_on(
        bg: AlphaColor<Srgb>,
        color_replacement: Option<ColorReplacement>,
    ) -> Pixmap {
        use super::{RenderCache, RenderSettings, render};

        let pdf = Pdf::new(stencil_fixture()).unwrap();
        let cache = RenderCache::new();
        let settings = RenderSettings {
            x_scale: 1.0,
            y_scale: 1.0,
            bg_color: bg,
            color_replacement,
            ..RenderSettings::default()
        };
        render(
            pdf.pages().get(0).expect("the fixture has one page"),
            &cache,
            &InterpreterSettings::default(),
            &settings,
        )
    }

    fn render(color_replacement: Option<ColorReplacement>) -> Pixmap {
        let pdf = Pdf::new(fixture()).unwrap();
        render_pdf(
            &pdf,
            1.0,
            InterpreterSettings::default(),
            color_replacement,
            None,
        )
        .unwrap()
        .remove(0)
    }

    /// Samples the pixel at (x, y); all fixture content is opaque, so the premultiplied
    /// value is the straight colour.
    fn sample(pixmap: &Pixmap, x: u16, y: u16) -> [u8; 4] {
        let p = pixmap.sample(x, y);
        [p.r, p.g, p.b, p.a]
    }

    fn count_pixels(
        pixmap: &Pixmap,
        region: (u16, u16, u16, u16),
        matches: impl Fn([u8; 4]) -> bool,
    ) -> u32 {
        let (x0, y0, x1, y1) = region;
        let mut count = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                if matches(sample(pixmap, x, y)) {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn color_replacement_maps_text_and_line_art() {
        let is_white = |p: [u8; 4]| p == [255, 255, 255, 255];
        let is_black = |p: [u8; 4]| p == [0, 0, 0, 255];

        let control = render(None);
        let replaced = render(Some(replacement(true)));

        // Background: the full-page white rect is line art and maps to the
        // `background` colour exactly (t = 1.0).
        assert_eq!(sample(&control, 5, 5), [255, 255, 255, 255]);
        assert_eq!(sample(&replaced, 5, 5), [0, 0, 0, 255]);

        // Text: solid black glyph cores in the control, fully white after replacement.
        // Region covers the "HI" glyphs (PDF baseline y=70, 24 pt → device y 10..34).
        let text_region = (16, 10, 56, 34);
        assert!(count_pixels(&control, text_region, is_black) >= 10);
        assert!(count_pixels(&replaced, text_region, is_white) >= 10);

        // Red rect: luminance t ≈ 0.2126 maps every channel to 255·(1−t) ≈ 201 (≈0xC9),
        // a near-white neutral grey.
        assert_eq!(sample(&control, 45, 60), [255, 0, 0, 255]);
        let [r, g, b, a] = sample(&replaced, 45, 60);
        assert!(r.abs_diff(201) <= 4, "red channel {r} not ≈ 201");
        assert!(g.abs_diff(201) <= 4 && b.abs_diff(201) <= 4, "g/b not ≈ 201: {g}/{b}");
        assert_eq!(a, 255);

        // CMYK rect (cyan): mapped to a neutral grey that differs from the control
        // colour, whatever exact value the CMYK→RGB transform produces.
        let cmyk_control = sample(&control, 125, 60);
        let cmyk_replaced = sample(&replaced, 125, 60);
        assert_ne!(cmyk_control[..3], cmyk_replaced[..3]);
        assert!(
            (i32::from(cmyk_replaced[0]) - i32::from(cmyk_replaced[1])).abs() <= 2
                && (i32::from(cmyk_replaced[1]) - i32::from(cmyk_replaced[2])).abs() <= 2,
            "mapped CMYK not neutral: {cmyk_replaced:?}"
        );
        assert_eq!(cmyk_replaced[3], 255);

        // Image pixels pass through untouched, identical to the control render.
        assert_eq!(sample(&control, 170, 70), [0, 0, 255, 255]);
        assert_eq!(sample(&replaced, 170, 70), [0, 0, 255, 255]);

        // With line-art replacement off, the red rect and the background rect keep
        // their original colours while text is still replaced.
        let no_line_art = render(Some(replacement(false)));
        assert_eq!(sample(&no_line_art, 45, 60), [255, 0, 0, 255]);
        assert_eq!(sample(&no_line_art, 5, 5), [255, 255, 255, 255]);
        assert!(count_pixels(&no_line_art, text_region, is_white) >= 10);
    }

    /// A stencil mask's ink paints with the mapped solid colour on the mapped background:
    /// it is line art in disguise, so white-on-black must leave it visible (black ink on
    /// black would disappear). The image occupies device x 30..70, y 5..45 (PDF 30..70 ×
    /// 5..45 pt); its left half is painted ink, its right half shows the page background.
    #[test]
    fn color_replacement_maps_stencil_mask_ink() {
        use vello_cpu::color::palette::css::{BLACK, WHITE};

        let control = render_stencil_on(WHITE, None);
        let replaced = render_stencil_on(BLACK, Some(replacement(true)));

        // Control: the page clears white and the stencil's ink keeps its black fill colour.
        assert_eq!(sample(&control, 5, 5), [255, 255, 255, 255]);
        assert_eq!(sample(&control, 40, 25), [0, 0, 0, 255]);
        // With white-on-black, the background clears black and the ink maps to white: ink
        // and background stay distinguishable, and each matches its mapped colour exactly.
        assert_eq!(sample(&replaced, 5, 5), [0, 0, 0, 255]);
        let ink = sample(&replaced, 40, 25);
        assert_eq!(ink, [255, 255, 255, 255]);
        assert_ne!(ink[..3], sample(&replaced, 5, 5)[..3], "ink must differ from the background");
        // The unpainted right half of the image area shows the mapped background.
        assert_eq!(sample(&replaced, 60, 25), [0, 0, 0, 255]);

        // The line-art gate holds: with line-art replacement off, the ink keeps its
        // original colour while the page still clears to the background.
        let no_line_art = render_stencil_on(BLACK, Some(replacement(false)));
        assert_eq!(sample(&no_line_art, 5, 5), [0, 0, 0, 255]);
        assert_eq!(sample(&no_line_art, 40, 25), [0, 0, 0, 255]);
    }
}

/// PdfCraft patch (19): a mesh shading paints its triangles alone (`EncodedShadingPattern::extent`),
/// and one with no triangle on the canvas paints nothing.
#[cfg(test)]
mod mesh_extent_tests {
    use super::{InterpreterSettings, Pdf, Pixmap, RenderCache, RenderSettings, render};
    use vello_cpu::color::palette::css::WHITE;

    /// A one-page 200×200 pt PDF whose content is `body`, with a type 4 (free-form triangle)
    /// shading `/Sh0`: one red triangle with corners (x0, y0), (x1, y0) and (x0, y1) in user
    /// space, and an ExtGState `/Half` with 50% alpha.
    fn fixture(body: &str, (x0, y0, x1, y1): (i32, i32, i32, i32)) -> Vec<u8> {
        // Three vertices: a flag, x and y (8 bits each, decoded into the ranges) and RGB.
        let mesh: [u8; 18] = [
            0, 0, 0, 255, 0, 0, //
            0, 255, 0, 255, 0, 0, //
            0, 0, 255, 255, 0, 0,
        ];
        let mut objs: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Shading << /Sh0 5 0 R >> \
              /ExtGState << /Half << /ca 0.5 /CA 0.5 >> >> >> >>"
                .to_vec(),
            format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes(),
        ];
        let mut shading = format!(
            "<< /ShadingType 4 /ColorSpace /DeviceRGB /BitsPerCoordinate 8 /BitsPerComponent 8 \
             /BitsPerFlag 8 /Decode [{x0} {x1} {y0} {y1} 0 1 0 1 0 1] /Length {} >>\nstream\n",
            mesh.len()
        )
        .into_bytes();
        shading.extend_from_slice(&mesh);
        shading.extend_from_slice(b"\nendstream");
        objs.push(shading);

        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes(),
        );
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objs.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    fn render_at(pdf: Vec<u8>, scale: f32) -> Pixmap {
        let pdf = Pdf::new(pdf).unwrap();
        let cache = RenderCache::new();
        let settings = RenderSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
            ..RenderSettings::default()
        };
        render(
            pdf.pages().get(0).expect("the fixture has one page"),
            &cache,
            &InterpreterSettings::default(),
            &settings,
        )
    }

    fn px(pixmap: &Pixmap, x: u16, y: u16) -> [u8; 4] {
        let p = pixmap.sample(x, y);
        [p.r, p.g, p.b, p.a]
    }

    fn inked(pixmap: &Pixmap) -> usize {
        (0..pixmap.height())
            .flat_map(|y| (0..pixmap.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| px(pixmap, x, y) != [255, 255, 255, 255])
            .count()
    }

    /// The triangle (100, 100)–(110, 100)–(100, 110) in a 200 pt page: in device space (y
    /// down) the pixel (102, 97) is inside it, (108, 91) past its hypotenuse, and the rest of
    /// the page is nowhere near it.
    #[test]
    fn a_mesh_paints_its_triangles_alone() {
        let white = [255, 255, 255, 255];
        for scale in [1.0_f32, 4.0] {
            let p = render_at(fixture("/Sh0 sh", (100, 100, 110, 110)), scale);
            let at = |x: f32, y: f32| ((x * scale) as u16, (y * scale) as u16);
            let (x, y) = at(102.5, 97.5);
            assert_eq!(px(&p, x, y), [255, 0, 0, 255], "scale {scale}: inside the triangle");
            let (x, y) = at(108.5, 91.5);
            assert_eq!(px(&p, x, y), white, "scale {scale}: past the hypotenuse");
            let (x, y) = at(150.0, 50.0);
            assert_eq!(px(&p, x, y), white, "scale {scale}: far from it");
            let (x, y) = at(10.0, 190.0);
            assert_eq!(px(&p, x, y), white, "scale {scale}: the texture is not repeated");
            // A 10 × 10 pt right triangle covers 50 pt²; the sampler counts the pixels whose
            // corner lies inside it (36 at scale 1, nearer 50 as the scale grows), and nothing
            // beyond its own bounding box is touched.
            let area = inked(&p) as f32 / (scale * scale);
            assert!((30.0..=60.0).contains(&area), "scale {scale}: {area} pt² inked");
        }
    }

    /// The alpha of the graphics state still applies: red at 50% over white is pink.
    #[test]
    fn a_mesh_keeps_the_graphics_state_alpha() {
        let p = render_at(fixture("/Half gs /Sh0 sh", (100, 100, 110, 110)), 1.0);
        let [r, g, b, a] = px(&p, 102, 97);
        assert!(r == 255 && (120..=136).contains(&g) && g == b && a == 255, "{:?}", [r, g, b, a]);
    }

    /// A mesh whose triangle lies off the canvas paints nothing, and costs no texture.
    #[test]
    fn a_mesh_off_the_canvas_paints_nothing() {
        let p = render_at(fixture("/Sh0 sh", (300, 300, 310, 310)), 1.0);
        assert_eq!(inked(&p), 0);
        // One straddling the edge paints its part on the canvas.
        let p = render_at(fixture("/Sh0 sh", (195, 100, 205, 110)), 1.0);
        assert_eq!(px(&p, 196, 98), [255, 0, 0, 255]);
        assert_eq!(px(&p, 150, 50), [255, 255, 255, 255]);
    }
}
