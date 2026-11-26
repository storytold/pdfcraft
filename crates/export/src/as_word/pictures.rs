//! RTF repeats encoded image data at every placement. Bound that expansion before copying,
//! and bake rotations because its inline picture writer does not apply DrawingML transforms.
use super::Image;
use image::{ImageEncoder, RgbaImage};
use std::io::{self, Write};

const MAX_ENCODED: usize = 512 << 20;
const MAX_PIXELS: u64 = 16 << 20;
const MAX_RASTER_WORK: u64 = 64 << 20;
const MAX_PROBE_INPUT: usize = 512 << 20;

#[derive(Default)]
pub(super) struct Budget {
    encoded: usize,
    raster: u64,
    probe_input: usize,
}

fn rtf_bytes(bytes: usize) -> Option<usize> {
    bytes.checked_mul(2)?.checked_add(bytes.div_ceil(64))?.checked_add(256)
}

impl Budget {
    pub(super) fn prepare(&mut self, im: &Image) -> Option<(Vec<u8>, &'static str)> {
        let rotation = im.rotation.rem_euclid(360.0);
        if !rotation.is_finite() || !matches!(im.ext, "png" | "jpg" | "jpeg") {
            return None;
        }
        let remaining = MAX_ENCODED.checked_sub(self.encoded)?;
        if im.bytes.is_empty() || rtf_bytes(im.bytes.len())? > remaining {
            return None;
        }
        let (bytes, ext) = if rotation == 0.0 {
            (im.bytes.to_vec(), im.ext)
        } else {
            // JPEG dimension probes copy the input. Charge every attempt before probing,
            // including failures, so repeated rejected placements cannot multiply that work.
            let probe_input = self.probe_input.checked_add(im.bytes.len())?;
            if probe_input > MAX_PROBE_INPUT {
                return None;
            }
            self.probe_input = probe_input;
            let (w, h) = image::ImageReader::new(io::Cursor::new(&im.bytes)).with_guessed_format().ok()?.into_dimensions().ok()?;
            let pixels = u64::from(w).checked_mul(u64::from(h))?;
            if w == 0 || h == 0 || w > 4096 || h > 4096 || pixels > MAX_PIXELS {
                return None;
            }
            let [pw, ph] = im.size;
            if !pw.is_finite() || !ph.is_finite() || pw <= 0.0 || ph <= 0.0 {
                return None;
            }
            let (sin, cos) = rotation.to_radians().sin_cos();
            let bw = pw * cos.abs() + ph * sin.abs();
            let bh = pw * sin.abs() + ph * cos.abs();
            let density = (f64::from(w) / pw).min(f64::from(h) / ph);
            // Exact quarter turns preserve every source pixel even when the PDF stretches
            // the image anisotropically. Its physical aspect remains in the displayed rect.
            let (ow, oh) = if rotation == 90.0 || rotation == 270.0 {
                (f64::from(h), f64::from(w))
            } else if rotation == 180.0 {
                (f64::from(w), f64::from(h))
            } else {
                // Arbitrary angles use bounded nearest-neighbour resampling at the lower
                // source density; unlike quarter turns this can lose detail when stretched.
                ((bw * density).round().max(1.0), (bh * density).round().max(1.0))
            };
            if !ow.is_finite() || !oh.is_finite() || ow > 4096.0 || oh > 4096.0 {
                return None;
            }
            let (ow, oh) = (ow as u32, oh as u32);
            let output_pixels = u64::from(ow) * u64::from(oh);
            if output_pixels > MAX_PIXELS {
                return None;
            }
            let raster = self.raster.checked_add(pixels)?.checked_add(output_pixels)?;
            if raster > MAX_RASTER_WORK {
                return None;
            }
            // Charge only accepted work, but retain the charge if decoding later fails.
            self.raster = raster;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(4096);
            limits.max_image_height = Some(4096);
            limits.max_alloc = Some(64 << 20);
            let mut reader = image::ImageReader::new(io::Cursor::new(&im.bytes)).with_guessed_format().ok()?;
            reader.limits(limits);
            let source = reader.decode().ok()?.into_rgba8();
            let mut output = RgbaImage::new(ow, oh);
            for (x, y, pixel) in output.enumerate_pixels_mut() {
                let exact = if rotation == 90.0 {
                    Some((w - 1 - y, x))
                } else if rotation == 180.0 {
                    Some((w - 1 - x, h - 1 - y))
                } else if rotation == 270.0 {
                    Some((y, h - 1 - x))
                } else {
                    None
                };
                if let Some((sx, sy)) = exact {
                    *pixel = *source.get_pixel(sx, sy);
                    continue;
                }
                let dx = (f64::from(x) + 0.5) * bw / f64::from(ow) - bw / 2.0;
                let dy = (f64::from(y) + 0.5) * bh / f64::from(oh) - bh / 2.0;
                // PDF rotation is counterclockwise in y-up space; raster rows are y-down.
                let sx = (cos * dx - sin * dy + pw / 2.0) * f64::from(w) / pw;
                let sy = (sin * dx + cos * dy + ph / 2.0) * f64::from(h) / ph;
                if sx >= 0.0 && sy >= 0.0 && sx < f64::from(w) && sy < f64::from(h) {
                    *pixel = *source.get_pixel(sx as u32, sy as u32);
                }
            }
            let mut sink = Limited { bytes: Vec::new(), cap: remaining.saturating_sub(256) / 3 };
            image::codecs::png::PngEncoder::new(&mut sink).write_image(output.as_raw(), ow, oh, image::ExtendedColorType::Rgba8).ok()?;
            (sink.bytes, "png")
        };
        self.encoded = self.encoded.checked_add(rtf_bytes(bytes.len())?)?;
        if self.encoded > MAX_ENCODED {
            return None;
        }
        Some((bytes, ext))
    }
}

struct Limited {
    bytes: Vec<u8>,
    cap: usize,
}
impl Write for Limited {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.len() > self.cap.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("RTF picture byte budget"));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(rotation: f64) -> Image {
        let pixels = RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).expect("two pixels");
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes).write_image(pixels.as_raw(), 2, 1, image::ExtendedColorType::Rgba8).expect("encode tiny PNG");
        Image { ext: "png", bytes: bytes.into(), rect: [0.0, 0.0, 1.0, 2.0], size: [2.0, 1.0], rotation }
    }

    #[test]
    fn pdf_counterclockwise_rotations_are_baked_into_asymmetric_pixels() {
        for (angle, dims, first, last) in [
            (90.0, (1, 2), [0, 0, 255, 255], [255, 0, 0, 255]),
            (180.0, (2, 1), [0, 0, 255, 255], [255, 0, 0, 255]),
            (270.0, (1, 2), [255, 0, 0, 255], [0, 0, 255, 255]),
            (-90.0, (1, 2), [255, 0, 0, 255], [0, 0, 255, 255]),
        ] {
            let (bytes, ext) = Budget::default().prepare(&sample(angle)).expect("rotated picture");
            assert_eq!(ext, "png");
            let pixels = image::load_from_memory(&bytes).expect("PNG").into_rgba8();
            assert_eq!(pixels.dimensions(), dims);
            assert_eq!(pixels.get_pixel(0, 0).0, first);
            assert_eq!(pixels.get_pixel(dims.0 - 1, dims.1 - 1).0, last);
        }
    }

    #[test]
    fn quarter_turns_preserve_pixels_when_physical_size_is_stretched() {
        for (angle, dims, first, last) in [
            (90.0, (1, 2), [0, 0, 255, 255], [255, 0, 0, 255]),
            (180.0, (2, 1), [0, 0, 255, 255], [255, 0, 0, 255]),
            (270.0, (1, 2), [255, 0, 0, 255], [0, 0, 255, 255]),
        ] {
            let mut im = sample(angle);
            im.size = [1.0, 1.0];
            let (bytes, _) = Budget::default().prepare(&im).expect("stretched rotated picture");
            let pixels = image::load_from_memory(&bytes).expect("PNG").into_rgba8();
            assert_eq!(pixels.dimensions(), dims);
            assert_eq!(pixels.get_pixel(0, 0).0, first);
            assert_eq!(pixels.get_pixel(dims.0 - 1, dims.1 - 1).0, last);
        }
    }

    #[test]
    fn flow_document_embeds_rotated_pixels_not_original_bytes() {
        let page =
            crate::layout::Page { width: 10.0, height: 10.0, blocks: Vec::new(), images: vec![sample(90.0)], background: None, rules: Vec::new() };
        let (doc, _) = super::super::flow_doc(&[page], &super::super::Meta { title: "", language: None, font: "Helvetica" });
        let bytes = doc.media.values().next().expect("picture retained");
        let pixels = image::load_from_memory(bytes).expect("PNG").into_rgba8();
        assert_eq!(pixels.dimensions(), (1, 2));
        assert_eq!(pixels.get_pixel(0, 0).0, [0, 0, 255, 255]);
        assert_eq!(pixels.get_pixel(0, 1).0, [255, 0, 0, 255]);
    }

    #[test]
    fn repeated_allocations_pay_each_hex_expansion_before_copying() {
        let im = sample(0.0);
        let cost = rtf_bytes(im.bytes.len()).expect("small");
        let mut budget = Budget { encoded: MAX_ENCODED - cost * 2, raster: 0, ..Default::default() };
        assert_eq!(budget.prepare(&im).expect("first").0, im.bytes.as_ref());
        assert!(budget.prepare(&im).is_some());
        assert!(budget.prepare(&im).is_none());
        assert_eq!(budget.encoded, MAX_ENCODED);
        assert!(rtf_bytes(usize::MAX).is_none());
    }

    #[test]
    fn flow_budget_counts_reused_media_at_each_placement() {
        let im = sample(0.0);
        let cost = rtf_bytes(im.bytes.len()).expect("small");
        let page = crate::layout::Page { width: 10.0, height: 10.0, blocks: Vec::new(), images: vec![im; 3], background: None, rules: Vec::new() };
        let (doc, omitted) = super::super::flow_doc_with_budget(
            &[page],
            &super::super::Meta { title: "", language: None, font: "Helvetica" },
            Budget { encoded: MAX_ENCODED - 2 * cost, raster: 0, ..Default::default() },
        );
        assert_eq!(omitted, [0]);
        let count: usize = doc
            .body
            .iter()
            .map(|block| match &**block {
                craft_wordmodel::Block::Para(p) => p.objects.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(count, 2);
        let rtf = craft_rtf::doc::export_with_generator(&doc, "PdfCraft");
        assert_eq!(rtf.matches("\\pngblip").count(), 2);
    }

    #[test]
    fn rejected_raster_charge_leaves_room_for_a_smaller_picture() {
        let mut budget = Budget { encoded: 0, raster: MAX_RASTER_WORK - 4, ..Default::default() };
        assert!(budget.prepare(&sample(45.0)).is_none());
        assert_eq!(budget.raster, MAX_RASTER_WORK - 4);
        assert!(budget.prepare(&sample(90.0)).is_some());
        assert_eq!(budget.raster, MAX_RASTER_WORK);
    }

    #[test]
    fn rejected_probes_pay_input_budget_before_retrying() {
        let mut im = sample(90.0);
        im.ext = "jpg";
        im.bytes = vec![0xff, 0xd8, 0xff, 0xe0, 0, 2, 0xff, 0xd9].into();
        let cost = im.bytes.len();
        let mut budget = Budget { probe_input: MAX_PROBE_INPUT - 2 * cost, ..Default::default() };
        assert!(budget.prepare(&im).is_none());
        assert_eq!(budget.probe_input, MAX_PROBE_INPUT - cost);
        assert!(budget.prepare(&im).is_none());
        assert_eq!(budget.probe_input, MAX_PROBE_INPUT);
        assert!(budget.prepare(&im).is_none());
        assert_eq!(budget.probe_input, MAX_PROBE_INPUT);
        assert_eq!(budget.encoded, 0);
        assert_eq!(budget.raster, 0);
        // Exhausting rotated probes must not suppress unrotated byte-preserving pictures.
        assert!(budget.prepare(&sample(0.0)).is_some());
    }

    #[test]
    fn picture_fits_both_text_area_dimensions() {
        let mut im = sample(0.0);
        im.rect = [0.0, 0.0, 100.0, 1000.0];
        let section = craft_wordmodel::SectionProps {
            page_w: 600.0,
            page_h: 400.0,
            margin_top: 50.0,
            margin_bottom: 50.0,
            margin_left: 50.0,
            margin_right: 50.0,
            ..Default::default()
        };
        let p = super::super::picture(String::new(), &im, &section);
        assert!(matches!(p.objects.first(), Some(craft_wordmodel::InlineObject::Image { w, h, .. }) if *w == 30.0 && *h == 300.0));
    }

    #[test]
    fn raster_work_and_encoded_writer_are_bounded() {
        assert!(Budget { encoded: 0, raster: MAX_RASTER_WORK, ..Default::default() }.prepare(&sample(45.0)).is_none());
        assert!(Budget::default().prepare(&sample(f64::NAN)).is_none());
        let mut bad = sample(45.0);
        bad.size[0] = f64::INFINITY;
        assert!(Budget::default().prepare(&bad).is_none());
        let mut sink = Limited { bytes: Vec::new(), cap: 3 };
        assert!(sink.write_all(&[1, 2, 3]).is_ok());
        assert!(sink.write_all(&[4]).is_err());
        assert_eq!(sink.bytes, [1, 2, 3]);
    }
}
