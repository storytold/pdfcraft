//! Independent flat-patch oracles for the Lab Bradford follow-up to #1237.
use crate::{PageRenderer, RenderConfig, RenderRequest, RequestKind};
use std::sync::Arc;

// These frozen RGB8 values were computed outside the renderer in f64 from the
// CIE Lab inverse, the full XYZ adaptation matrix B^-1 diag(B*D65 / B*white) B,
// and the standard sRGB matrix/transfer function. No production helper is used.
// Bradford reference: https://www.color.org/chadtag/ (ICC TN 02-2003).
// White points: D50 [0.96422, 1, 0.82521], E [1, 1, 1],
// D65 [0.95047, 1, 1.08883]. D65 adaptation is the identity.
const SAMPLES: [[f64; 3]; 4] = [[50.0, 0.0, 0.0], [40.0, 20.0, -60.0], [70.0, -50.0, 30.0], [100.0, 0.0, 0.0]];
const D50: [[u8; 3]; 4] = [[119, 119, 119], [67, 86, 194], [66, 194, 113], [255, 255, 255]];
const E: [[u8; 3]; 4] = [[119, 119, 119], [50, 88, 194], [64, 194, 114], [255, 255, 255]];
const D65: [[u8; 3]; 4] = [[119, 119, 119], [19, 90, 194], [77, 193, 114], [255, 255, 255]];

fn patch(space: &str, operands: &str) -> [u8; 4] {
    let content = format!("/C cs {operands} sc 0 0 20 20 re f");
    let bytes = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 20 20] /Resources << /ColorSpace << /C {space} >> >> /Contents 4 0 R >> endobj\n\
         4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n\
         trailer << /Root 1 0 R >>\n%%EOF",
        content.len()
    )
    .into_bytes();
    let page = PageRenderer::new(Arc::new(bytes), RenderConfig::default()).render(RenderRequest {
        page: 0,
        kind: RequestKind::Pixels,
        scale: 1.0,
        ..Default::default()
    });
    assert!(page.error.is_none(), "{space} {operands}: {:?}", page.error);
    assert_eq!((page.width, page.height), (20, 20));
    page.rgba.get((10 * 20 + 10) * 4..(10 * 20 + 10) * 4 + 4).unwrap().try_into().unwrap()
}

fn assert_rgb(actual: [u8; 4], expected: [u8; 3], context: &str) {
    assert_eq!(actual[3], 255, "{context}: {actual:?}");
    for (got, want) in actual.into_iter().zip(expected) {
        assert!(got.abs_diff(want) <= 1, "{context}: got {actual:?}, expected {expected:?}");
    }
}

fn check_white(white: &str, expected: [[u8; 3]; 4]) {
    let space = format!("[/Lab << /WhitePoint {white} /Range [-100 100 -100 100] >>]");
    for (sample, expected) in SAMPLES.into_iter().zip(expected) {
        let operands = format!("{} {} {}", sample[0], sample[1], sample[2]);
        assert_rgb(patch(&space, &operands), expected, &format!("{white} Lab {sample:?}"));
    }
}

#[test]
fn lab_bradford_d50_neutral_and_chromatic_oracles() {
    check_white("[0.96422 1 0.82521]", D50);
}

#[test]
fn lab_bradford_equal_energy_neutral_and_chromatic_oracles() {
    check_white("[1 1 1]", E);
}

#[test]
fn lab_bradford_d65_identity_oracles() {
    check_white("[0.95047 1 1.08883]", D65);
}

#[test]
fn lab_bradford_indexed_matches_direct_and_oracle() {
    for (white, blue) in [("[0.96422 1 0.82521]", D50[1]), ("[1 1 1]", E[1]), ("[0.95047 1 1.08883]", D65[1])] {
        // <66 99 33> maps exactly to L*=40, a*=20, b*=-60.
        let lab = format!("[/Lab << /WhitePoint {white} /Range [-100 100 -100 100] >>]");
        let indexed = patch(&format!("[/Indexed {lab} 0 <669933>]"), "0");
        assert_rgb(indexed, blue, &format!("{white} Indexed blue"));
        assert_eq!(indexed, patch(&lab, "40 20 -60"), "{white} direct/Indexed blue");

        // A non-default Range maps <80 80 80> to L*=128/255*100, a*=b*=0.
        let lab = format!("[/Lab << /WhitePoint {white} /Range [-128 127 -128 127] >>]");
        let indexed = patch(&format!("[/Indexed {lab} 0 <808080>]"), "0");
        assert_rgb(indexed, [119, 119, 119], &format!("{white} Indexed neutral"));
        assert_eq!(indexed, patch(&lab, "50.19607843137255 0 0"), "{white} direct/Indexed neutral");
    }
}

#[test]
fn lab_bradford_degenerate_cones_fall_back_to_unadapted_xyz() {
    // In f32, B*[1, 1, 7.1964064] has a zero L cone response.
    // B*[3e38, 3e38, 3e38] overflows its L/M responses. The guard must
    // return the sample XYZ in both cases. Independent unadapted XYZ ->
    // sRGB references for Lab (50, 0, 0) are frozen below.
    for (white, expected) in [
        ("[1 1 7.1964064]", [0, 130, 255]),
        (
            "[300000000000000000000000000000000000000 300000000000000000000000000000000000000 300000000000000000000000000000000000000]",
            [255, 255, 255],
        ),
    ] {
        let space = format!("[/Lab << /WhitePoint {white} >>]");
        assert_rgb(patch(&space, "50 0 0"), expected, &format!("{white} degenerate cones"));
    }
}

#[test]
fn lab_bradford_asymmetric_chromatic_range_matches_direct_and_oracle() {
    // With a* [-40, 70] and b* [-90, 20], <80 99 33> maps to
    // L*=128/255*100, a*=26, b*=-68. Frozen independent D50 oracle:
    // RGB (98, 108, 237). The unequal, asymmetric ranges expose swaps
    // and default-Range scaling that a neutral lookup cannot detect.
    let lab = "[/Lab << /WhitePoint [0.96422 1 0.82521] /Range [-40 70 -90 20] >>]";
    let indexed = patch(&format!("[/Indexed {lab} 0 <809933>]"), "0");
    assert_rgb(indexed, [98, 108, 237], "D50 asymmetric chromatic Range");
    assert_eq!(indexed, patch(lab, "50.19607843137255 26 -68"));
}
