//! An image that takes the general decode path (not 8 bits a sample, a `/Decode` array or a
//! transfer function) holds no more than its RGB result and one band of samples while it decodes
//! (#7282). A sample depth other than 8 or 16 bits is padded to the declared size, so one byte of
//! data can declare the whole image: 4096 × 4096 five-bit `/DeviceCMYK` samples held at once,
//! as `u16`, again as `f32` and once more as bytes for the CMYK conversion, came to about
//! 500 MiB; at the 2^28-pixel limit the same file allocated about 7 GiB, and a failed allocation
//! ends the process rather than panicking.
//!
//! The peak is measured in a child process of this test binary, which renders the page alone and
//! reports its peak resident size (`VmHWM`, Linux only), so the other tests running in this one
//! do not count.

use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage, RequestKind};

/// Set in the child process: render the page and print the peak resident size.
#[cfg(target_os = "linux")]
const PROBE: &str = "PDFCRAFT_IMAGE_DECODE_MEMORY_PROBE";
/// The RGB result alone is 48 MiB; the samples held whole came to 128 MiB as `u16`, another
/// 256 MiB as `f32` and 64 MiB as CMYK bytes.
#[cfg(target_os = "linux")]
const PEAK_LIMIT_KIB: u64 = 256 * 1024;

/// An image of `width` × `height` whose dictionary also holds `entries`, and whose stream is
/// `data`.
fn image(width: u32, height: u32, entries: &str, data: &[u8]) -> Vec<u8> {
    let mut image = format!(
        "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} {entries} \
         /Length {} >>\nstream\n",
        data.len()
    )
    .into_bytes();
    image.extend_from_slice(data);
    image.extend_from_slice(b"\nendstream");
    image
}

/// A 200 × 200 page covered by an image of `width` × `height` whose dictionary also holds
/// `entries`, and whose stream is `data`.
fn pdf(width: u32, height: u32, entries: &str, data: &[u8]) -> Vec<u8> {
    pdf_with(image(width, height, entries, data), &[])
}

/// A 200 × 200 page covered by `image`, with `extra` objects numbered from 6.
fn pdf_with(image: Vec<u8>, extra: &[Vec<u8>]) -> Vec<u8> {
    let content = "q 200 0 0 200 0 0 cm /Im Do Q";
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R \
           /Resources << /XObject << /Im 5 0 R >> >> >>"
            .to_vec(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes(),
        image,
    ];
    objs.extend_from_slice(extra);
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let size = objs.len() + 1;
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

fn render(bytes: Vec<u8>) -> RenderedPage {
    let page = PageRenderer::new(Arc::new(bytes), RenderConfig::default()).render(RenderRequest {
        page: 0,
        kind: RequestKind::Pixels,
        tile: None,
        scale: 1.0,
        tag: 0,
        preview: false,
    });
    assert!(page.error.is_none(), "{:?}", page.error);
    assert_eq!((page.width, page.height), (200, 200));
    page
}

/// The RGB of the page's pixel at (`x`, `y`).
fn pixel(page: &RenderedPage, x: usize, y: usize) -> [u8; 3] {
    let at = (y * page.width as usize + x) * 4;
    let px = page.rgba.get(at..at + 3).expect("pixel inside the page");
    [px[0], px[1], px[2]]
}

/// This process's peak resident size, in KiB.
#[cfg(target_os = "linux")]
fn peak_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|l| l.starts_with("VmHWM:")).unwrap();
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

/// The child half of [`a_padded_image_decodes_without_holding_its_samples_whole`]: it runs alone,
/// in its own process. Without the variable it is a plain no-op.
#[cfg(target_os = "linux")]
#[test]
fn stand_in_for_an_image_decode() {
    if std::env::var_os(PROBE).is_none() {
        return;
    }
    // 4096 × 4096 five-bit CMYK from one byte. The inverted `/Decode` makes the zero padding full
    // ink, so a drawn image shows black.
    let page = render(pdf(4096, 4096, "/BitsPerComponent 5 /ColorSpace /DeviceCMYK /Decode [1 0 1 0 1 0 1 0]", &[0]));
    let centre = pixel(&page, 100, 100);
    assert!(centre.iter().all(|c| *c < 64), "the image is drawn: {centre:?}");
    println!("peak-kib={}", peak_kib());
}

#[cfg(target_os = "linux")]
#[test]
fn a_padded_image_decodes_without_holding_its_samples_whole() {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "image_decode_memory::stand_in_for_an_image_decode", "--nocapture", "--test-threads=1"])
        .env(PROBE, "1")
        .output()
        .expect("run this test's own binary as a child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "the child failed: {stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    // The harness prints the test's name on the same line, before what the test prints.
    let peak: u64 = stdout
        .split("peak-kib=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("the child ran the decode: {stdout}"))
        .parse()
        .unwrap();
    println!("the child's peak resident size: {peak} KiB");
    assert!(peak < PEAK_LIMIT_KIB, "peak resident size {peak} KiB, over {PEAK_LIMIT_KIB} KiB");
}

/// Sixteen-bit data of an odd length reads its last byte as the high byte of a sample whose low
/// byte is zero. Upstream indexed the missing byte and panicked, and the page lost the image.
#[test]
fn sixteen_bit_data_of_an_odd_length_is_drawn() {
    // 2 × 1 gray, inverted: 0xFFFF is black, 0x80 then a missing byte is 0x8000, mid grey.
    let page = render(pdf(2, 1, "/BitsPerComponent 16 /ColorSpace /DeviceGray /Decode [1 0]", &[0xFF, 0xFF, 0x80]));
    assert!(page.warnings.is_empty(), "{:?}", page.warnings);
    assert_eq!(pixel(&page, 50, 100), [0, 0, 0]);
    let grey = pixel(&page, 150, 100);
    assert!(grey.iter().all(|c| (120..=135).contains(c)), "{grey:?}");
}

// What follows draws as it did before the samples were read one at a time.

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLACK: [u8; 3] = [0, 0, 0];
const WHITE: [u8; 3] = [255, 255, 255];

/// A four-bit colour key hides the pixel whose samples all lie in its ranges.
#[test]
fn a_colour_key_hides_the_pixels_it_keys() {
    // Red, then green, four bits a sample: F 0 0 0 F 0. The key matches red only.
    let page = render(pdf(2, 1, "/BitsPerComponent 4 /ColorSpace /DeviceRGB /Mask [15 15 0 0 0 0]", &[0xF0, 0x00, 0xF0]));
    assert_eq!([pixel(&page, 50, 100), pixel(&page, 150, 100)], [WHITE, GREEN]);
}

/// A four-bit image without a key draws both its pixels.
#[test]
fn a_four_bit_image_draws_its_pixels() {
    let page = render(pdf(2, 1, "/BitsPerComponent 4 /ColorSpace /DeviceRGB", &[0xF0, 0x00, 0xF0]));
    assert_eq!([pixel(&page, 50, 100), pixel(&page, 150, 100)], [RED, GREEN]);
}

/// A `/Decode` array shorter than the pixel's samples decodes only the samples it has a range
/// for, and the values it gives are laid out as pixels, the rest padded with zeros.
#[test]
fn a_short_decode_array_keeps_its_layout() {
    // One range, inverted: red's first sample 15 gives 0, green's 0 gives 1. The two values fill
    // the first pixel's red and green.
    let page = render(pdf(2, 1, "/BitsPerComponent 4 /ColorSpace /DeviceRGB /Decode [1 0]", &[0xF0, 0x00, 0xF0]));
    assert_eq!([pixel(&page, 50, 100), pixel(&page, 150, 100)], [GREEN, BLACK]);
}

/// A one-bit image with more pixels than possible values goes through the lookup table.
#[test]
fn a_one_bit_image_draws_through_its_lookup() {
    // 1 0 1, inverted: black, white, black.
    let page = render(pdf(3, 1, "/BitsPerComponent 1 /ColorSpace /DeviceGray /Decode [1 0]", &[0xA0]));
    assert_eq!([pixel(&page, 33, 100), pixel(&page, 100, 100), pixel(&page, 166, 100)], [BLACK, WHITE, BLACK]);
}

/// A four-bit soft mask shows the image where it is opaque and the page where it is clear.
#[test]
fn a_four_bit_soft_mask_shows_and_hides() {
    let mask = image(2, 1, "/BitsPerComponent 4 /ColorSpace /DeviceGray", &[0xF0]);
    let page = render(pdf_with(image(2, 1, "/BitsPerComponent 8 /ColorSpace /DeviceGray /SMask 6 0 R", &[0, 0]), &[mask]));
    assert_eq!([pixel(&page, 50, 100), pixel(&page, 150, 100)], [BLACK, WHITE]);
}
