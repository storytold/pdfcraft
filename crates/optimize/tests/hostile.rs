//! The optimizer on absurd image sizes, drawing matrices and settings: it may refuse or leave
//! things alone, but never panics, hangs or allocates without bound.

use pdfcraft_cos::{Dict, Document, Object, Stream};
use pdfcraft_optimize::{Compression, ImageSettings, Settings, optimize};

fn image_dict(w: i64, h: i64, bpc: i64, cs: &str) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w));
    d.set(b"Height".to_vec(), Object::Int(h));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(bpc));
    d.set(b"ColorSpace".to_vec(), Object::name(cs));
    d
}

fn doc_with(img: Stream, content: &str) -> Document {
    let mut doc = Document::new_empty();
    let im = doc.add(Object::Stream(img));
    let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
    let mut x = Dict::new();
    x.set(b"Im".to_vec(), Object::Ref(im));
    let mut res = Dict::new();
    res.set(b"XObject".to_vec(), Object::Dict(x));
    let c = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content.as_bytes().to_vec())));
    let mut p = Dict::new();
    p.set(b"Type".to_vec(), Object::name("Page"));
    p.set(b"Parent".to_vec(), Object::Ref(pages));
    p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
    p.set(b"Resources".to_vec(), Object::Dict(res));
    p.set(b"Contents".to_vec(), Object::Ref(c));
    let r = doc.add(Object::Dict(p));
    doc.update_dict(pages, |d| {
        d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(r)]));
        d.set(b"Count".to_vec(), Object::Int(1));
    })
    .unwrap();
    doc
}

fn settings(target: f64, above: f64, compression: Compression) -> Settings {
    let s = ImageSettings { downsample: true, target_ppi: target, above_ppi: above, compression };
    Settings { color: s, gray: s, ..Settings::default() }
}

#[test]
fn absurd_image_sizes_never_panic() {
    let dims =
        [(0, 0), (-1, -1), (1, 0), (0, 1), (i64::MAX, 1), (1, i64::MAX), (i64::MAX, i64::MAX), (1_000_000_000, 1_000_000_000), (100_000, 100_000)];
    for (w, h) in dims {
        for cs in ["DeviceGray", "DeviceRGB", "DeviceCMYK"] {
            for bpc in [0, 1, 8, 16, 99, -8] {
                let img = Stream::flate(image_dict(w, h, bpc, cs), &[1, 2, 3, 4]);
                let mut doc = doc_with(img, "q 72 0 0 72 0 0 cm /Im Do Q");
                let _ = optimize(&mut doc, &Settings::default());
            }
        }
    }
}

#[test]
fn absurd_drawing_matrices_never_panic() {
    let ctms = [
        "0 0 0 0 0 0 cm",
        "1e300 0 0 1e300 0 0 cm",
        "1e-300 0 0 1e-300 0 0 cm",
        "-1e300 0 0 1e300 1e300 1e300 cm",
        "1e999 0 0 1e999 0 0 cm",
        "1 1e300 1e300 1 0 0 cm",
        "72 0 0 0 0 0 cm",
    ];
    for ctm in ctms {
        let mut px = Vec::new();
        px.extend(std::iter::repeat_n(7u8, 64 * 64 * 3));
        let img = Stream::flate(image_dict(64, 64, 8, "DeviceRGB"), &px);
        let mut doc = doc_with(img, &format!("q {ctm} /Im Do Q"));
        let _ = optimize(&mut doc, &Settings::default());
    }
}

#[test]
fn absurd_settings_never_panic() {
    let nums = [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, f64::MIN_POSITIVE];
    let comps = [Compression::Jpeg(0), Compression::Jpeg(255), Compression::Flate, Compression::Retain];
    for t in nums {
        for a in nums {
            for c in comps {
                let img = Stream::flate(image_dict(300, 300, 8, "DeviceRGB"), &vec![9u8; 300 * 300 * 3]);
                let mut doc = doc_with(img, "q 72 0 0 72 0 0 cm /Im Do Q");
                let _ = optimize(&mut doc, &settings(t, a, c));
            }
        }
    }
}
