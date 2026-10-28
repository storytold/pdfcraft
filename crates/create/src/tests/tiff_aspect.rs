use super::*;

const RGB_SAMPLES: [u8; 18] = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0, 255, 0, 255, 0, 255, 255];

#[derive(Clone, Copy)]
struct TiffResolution {
    unit: u16,
    x_n: u32,
    x_d: u32,
    y_n: u32,
    y_d: u32,
}

fn rgb_tiff(resolutions: &[TiffResolution]) -> Vec<u8> {
    use tiff::encoder::{Rational, TiffEncoder, colortype::RGB8};
    use tiff::tags::Tag;

    let mut output = std::io::Cursor::new(Vec::new());
    {
        let mut encoder = TiffEncoder::new(&mut output).unwrap();
        for resolution in resolutions {
            let mut image = encoder.new_image::<RGB8>(3, 2).unwrap();
            image.encoder().write_tag(Tag::ResolutionUnit, resolution.unit).unwrap();
            image.encoder().write_tag(Tag::XResolution, Rational { n: resolution.x_n, d: resolution.x_d }).unwrap();
            image.encoder().write_tag(Tag::YResolution, Rational { n: resolution.y_n, d: resolution.y_d }).unwrap();
            image.write_data(&RGB_SAMPLES).unwrap();
        }
    }
    output.into_inner()
}

fn assert_size(actual: &[f64], expected: (f64, f64)) {
    assert_eq!(actual.len(), 4);
    assert!(actual[0].abs() < 1e-6 && actual[1].abs() < 1e-6, "origin: {actual:?}");
    assert!((actual[2] - expected.0).abs() < 1e-6, "width: {actual:?}, expected {expected:?}");
    assert!((actual[3] - expected.1).abs() < 1e-6, "height: {actual:?}, expected {expected:?}");
}

fn image_samples(doc: &Document, page_index: usize) -> Vec<u8> {
    let page = pages(doc).remove(page_index);
    let resources = doc.resolve(page.get(b"Resources").unwrap());
    let xobjects = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
    let image = doc.resolve(xobjects.as_dict().unwrap().get(b"Im0").unwrap());
    let Object::Stream(image) = &*image else { panic!("image XObject") };
    image.decoded().unwrap()
}

#[test]
fn unitless_tiff_ratios_are_scale_invariant_and_preserve_rgb_samples() {
    let bytes = rgb_tiff(&[
        TiffResolution { unit: 1, x_n: 1, x_d: 2, y_n: 1, y_d: 4 },
        TiffResolution { unit: 1, x_n: 2, x_d: 1, y_n: 1, y_d: 1 },
        TiffResolution { unit: 1, x_n: 2000, x_d: 1, y_n: 1000, y_d: 1 },
        TiffResolution { unit: 1, x_n: 1, x_d: 1, y_n: 1, y_d: 1 },
        TiffResolution { unit: 1, x_n: 1, x_d: 1, y_n: 2, y_d: 1 },
    ]);
    let doc = reopen(&from_images(&[("unitless.tif".into(), bytes)]).unwrap());
    let pages = pages(&doc);

    assert_eq!(pages.len(), 5);
    let expected = [(3.0, 4.0), (3.0, 4.0), (3.0, 4.0), (3.0, 2.0), (6.0, 2.0)];
    for (index, (page, size)) in pages.iter().zip(expected).enumerate() {
        assert_size(&media(page), size);
        assert_eq!(image_samples(&doc, index), RGB_SAMPLES, "page {index}");
    }
}

#[test]
fn invalid_unitless_tiff_density_pairs_fall_back_to_square_pixels() {
    let bytes = rgb_tiff(&[TiffResolution { unit: 1, x_n: 0, x_d: 1, y_n: 2, y_d: 1 }, TiffResolution { unit: 1, x_n: 1, x_d: 0, y_n: 2, y_d: 1 }]);
    let doc = reopen(&from_images(&[("invalid-unitless.tif".into(), bytes)]).unwrap());
    let pages = pages(&doc);

    assert_eq!(pages.len(), 2);
    for (index, page) in pages.iter().enumerate() {
        assert_size(&media(page), (3.0, 2.0));
        assert_eq!(image_samples(&doc, index), RGB_SAMPLES, "page {index}");
    }
}

#[test]
fn unitless_tiff_resolution_is_per_ifd_and_image_xobject_uses_first_ifd() {
    let bytes = rgb_tiff(&[TiffResolution { unit: 1, x_n: 2, x_d: 1, y_n: 1, y_d: 1 }, TiffResolution { unit: 1, x_n: 1, x_d: 1, y_n: 2, y_d: 1 }]);
    let doc = reopen(&from_images(&[("per-ifd.tif".into(), bytes.clone())]).unwrap());
    let pages = pages(&doc);

    assert_eq!(pages.len(), 2);
    assert_size(&media(&pages[0]), (3.0, 4.0));
    assert_size(&media(&pages[1]), (6.0, 2.0));
    assert_eq!(image_samples(&doc, 0), RGB_SAMPLES);
    assert_eq!(image_samples(&doc, 1), RGB_SAMPLES);

    let mut destination = Document::new_empty();
    let (image, size) = image_xobject(&mut destination, "per-ifd.tif", &bytes).unwrap();
    assert!((size.0 - 3.0).abs() < 1e-6 && (size.1 - 4.0).abs() < 1e-6, "first IFD size: {size:?}");
    let Object::Stream(image) = &*destination.get(image) else { panic!("image XObject") };
    assert_eq!(image.decoded().unwrap(), RGB_SAMPLES);
}

#[test]
fn absolute_tiff_units_and_explicit_dpi_keep_their_existing_sizes() {
    let bytes = rgb_tiff(&[
        TiffResolution { unit: 2, x_n: 144, x_d: 1, y_n: 72, y_d: 1 },
        TiffResolution { unit: 3, x_n: 144, x_d: 1, y_n: 72, y_d: 1 },
        TiffResolution { unit: 1, x_n: 1, x_d: 2, y_n: 1, y_d: 4 },
    ]);
    let embedded = reopen(&from_images(&[("absolute.tif".into(), bytes.clone())]).unwrap());
    let embedded_pages = pages(&embedded);

    assert_size(&media(&embedded_pages[0]), (1.5, 2.0));
    assert_size(&media(&embedded_pages[1]), (3.0 * 72.0 / (144.0 * 2.54), 2.0 * 72.0 / (72.0 * 2.54)));

    let explicit = reopen(&from_images_with_resolution(&[("absolute.tif".into(), bytes)], ImageResolution::Dpi(144.0)).unwrap());
    for page in pages(&explicit) {
        assert_size(&media(&page), (1.5, 1.0));
    }
}
