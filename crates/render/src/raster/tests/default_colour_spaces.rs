//! #1201: the `/DefaultGray`, `/DefaultRGB` and `/DefaultCMYK` colour spaces of a resource
//! dictionary stand in for the device spaces its content selects, with colour operators, with
//! images and with shadings (ISO 32000-2 §8.6.5.6, vendored hayro-interpret patch 38). hayro
//! drew device colour as it was, so pages that calibrate their device colour this way (common
//! in PDF/A) came out in the wrong colours.
//!
//! The defaults are synthetic ICC profiles built here, each sending device colour somewhere it
//! cannot go uncalibrated: red to green (or blue), black to mid grey, black ink to blue.

use super::{ONE_PIXEL_RGB, image_cache_pdf, render_first_page, rgb_at};

/// The width in pixels of the 130 × 70 pt page `image_cache_pdf` makes, rendered at 72 dpi.
const WIDTH: usize = 130;

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const BLACK: [u8; 3] = [0, 0, 0];
/// Device black through `gray_profile`: half the luminance of white, sRGB-encoded.
const MID_GREY: [u8; 3] = [188, 188, 188];
/// Full black ink through `cmyk_profile`: CIELAB (40, 20, -60) relative to the D50 white of the
/// profile connection space, which is XYZ (0.1378, 0.1125, 0.3956) and a mid blue in sRGB.
const INK_BLUE: [u8; 3] = [67, 86, 194];

/// An s15Fixed16Number (ICC.1 §4.6).
fn s15f16(value: f64) -> [u8; 4] {
    ((value * 65536.0).round() as i32).to_be_bytes()
}

/// An ICC v2 profile (ICC.1:2001-04 §6) of device class `class` for the colour space `space`,
/// with `tags` (signature, data) in that order. The device model is left blank: hayro takes a
/// profile whose model reads `sRGB` for standard sRGB and skips its transform.
fn icc(class: &[u8; 4], space: &[u8; 4], pcs: &[u8; 4], tags: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let start = 128 + 4 + 12 * tags.len();
    let (mut table, mut data) = (Vec::new(), Vec::new());
    for (signature, tag) in tags {
        table.extend_from_slice(signature);
        table.extend_from_slice(&u32::try_from(start + data.len()).unwrap().to_be_bytes());
        table.extend_from_slice(&u32::try_from(tag.len()).unwrap().to_be_bytes());
        data.extend_from_slice(tag);
        data.resize(data.len().next_multiple_of(4), 0);
    }
    let mut profile = vec![0; 128];
    profile[0..4].copy_from_slice(&u32::try_from(start + data.len()).unwrap().to_be_bytes());
    profile[8..12].copy_from_slice(&[2, 0x10, 0, 0]);
    profile[12..16].copy_from_slice(class);
    profile[16..20].copy_from_slice(space);
    profile[20..24].copy_from_slice(pcs);
    profile[36..40].copy_from_slice(b"acsp");
    // The illuminant of the profile connection space, D50.
    for (i, value) in [0.9642, 1.0, 0.8249].into_iter().enumerate() {
        profile[68 + 4 * i..72 + 4 * i].copy_from_slice(&s15f16(value));
    }
    profile.extend_from_slice(&u32::try_from(tags.len()).unwrap().to_be_bytes());
    profile.extend(table);
    profile.extend(data);
    profile
}

/// An `XYZType` tag.
fn xyz([x, y, z]: [f64; 3]) -> Vec<u8> {
    [b"XYZ ".as_slice(), &[0; 4], &s15f16(x), &s15f16(y), &s15f16(z)].concat()
}

/// A `curveType` tag: one entry is a gamma (u8Fixed8), more are points spread evenly over 0..1.
fn curve(points: &[u16]) -> Vec<u8> {
    let mut tag = [b"curv".as_slice(), &[0; 4], &u32::try_from(points.len()).unwrap().to_be_bytes()].concat();
    for point in points {
        tag.extend_from_slice(&point.to_be_bytes());
    }
    tag
}

/// sRGB's red, green and blue colorants, adapted to D50 as sRGB profiles record them.
const SRGB_COLORANTS: [[f64; 3]; 3] = [[0.4361, 0.2225, 0.0139], [0.3851, 0.7169, 0.0971], [0.1431, 0.0606, 0.7141]];

/// A linear RGB profile whose red, green and blue take the sRGB colorants `primaries` names:
/// with `[1, 2, 0]` red is drawn as sRGB green, green as blue and blue as red.
fn rgb_profile(primaries: [usize; 3]) -> Vec<u8> {
    let mut tags = Vec::new();
    for (signature, primary) in [*b"rXYZ", *b"gXYZ", *b"bXYZ"].into_iter().zip(primaries) {
        tags.push((signature, xyz(SRGB_COLORANTS[primary])));
    }
    for signature in [*b"rTRC", *b"gTRC", *b"bTRC"] {
        tags.push((signature, curve(&[0x0100])));
    }
    icc(b"mntr", b"RGB ", b"XYZ ", &tags)
}

/// A gray profile whose tone curve rises from half the luminance of white to all of it, so
/// device black (`0 g`) is drawn mid grey.
fn gray_profile() -> Vec<u8> {
    icc(b"mntr", b"GRAY", b"XYZ ", &[(*b"kTRC", curve(&[0x8000, 0xffff]))])
}

/// A CMYK profile with an 8-bit lookup table into CIELAB (`lut8Type`, ICC.1 §6.5.7) that draws
/// full black ink as L*a*b* (40, 20, -60) and no black ink as white, whatever the other inks.
fn cmyk_profile() -> Vec<u8> {
    let mut lut = [b"mft1".as_slice(), &[0; 4], &[4, 3, 2, 0]].concat();
    // The matrix, only used for XYZ input: identity.
    for value in [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
        lut.extend_from_slice(&s15f16(value));
    }
    // An identity input table per ink.
    for _ in 0..4 {
        lut.extend(0..=255_u8);
    }
    // The grid, two points per ink, black ink varying fastest. Lab is encoded as L* · 2.55,
    // a* + 128 and b* + 128.
    for point in 0..16 {
        lut.extend_from_slice(if point % 2 == 1 { &[102, 148, 68] } else { &[255, 128, 128] });
    }
    // An identity output table per component.
    for _ in 0..3 {
        lut.extend(0..=255_u8);
    }
    icc(b"prtr", b"CMYK", b"Lab ", &[(*b"A2B0", lut)])
}

/// An `/ICCBased` profile stream of `n` components.
fn profile_stream(n: u8, profile: Vec<u8>) -> (String, Vec<u8>) {
    (format!("<< /N {n} /Length {} >>", profile.len()), profile)
}

/// A stream with the entries `dict` and the content `content`.
fn stream(dict: &str, content: &str) -> (String, Vec<u8>) {
    (format!("<< {dict} /Length {} >>", content.len()), content.as_bytes().to_vec())
}

/// A form XObject covering the page, with the dictionary entries `extra`.
fn form(extra: &str, content: &str) -> (String, Vec<u8>) {
    stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 130 70] {extra}"), content)
}

/// Draw `content` with `resources` on `image_cache_pdf`'s page, whose objects 5, 6, … are the
/// streams `objects`, and give the colour at each of `points` (from the top left).
fn colours(content: &str, resources: &str, objects: &[(String, Vec<u8>)], points: &[(usize, usize)]) -> Vec<[u8; 3]> {
    let objects = objects.iter().map(|(dict, data)| (dict.as_str(), data.as_slice())).collect::<Vec<_>>();
    let page = render_first_page(image_cache_pdf(content, resources, &objects));
    assert_eq!((page.width as usize, page.height), (WIDTH, 70));
    points.iter().map(|&(x, y)| rgb_at(&page.rgba, WIDTH, x, y)).collect()
}

#[track_caller]
fn assert_near(got: [u8; 3], want: [u8; 3], what: &str) {
    assert!(got.iter().zip(want).all(|(got, want)| got.abs_diff(want) <= 6), "{what}: got {got:?}, expected about {want:?}");
}

/// Device CMYK black ink through hayro's own CMYK profile: near black.
#[track_caller]
fn assert_dark(got: [u8; 3], what: &str) {
    assert!(got.iter().all(|c| *c < 90), "{what}: got {got:?}, expected near black");
}

/// `rg`, `RG`, `/DeviceRGB cs` and a `/ColorSpace` resource naming DeviceRGB all select
/// DeviceRGB, so all draw in the page's DefaultRGB. Without one they draw device red as before.
#[test]
fn device_rgb_operators_draw_in_default_rgb() {
    let content = "1 0 0 rg 0 0 30 70 re f /DeviceRGB cs 1 0 0 sc 32 0 30 70 re f /CS0 cs 1 0 0 sc 64 0 30 70 re f \
                   1 0 0 RG 20 w 111 0 m 111 70 l S";
    let points = [(15, 35), (47, 35), (79, 35), (111, 35)];
    let what = ["rg", "/DeviceRGB cs", "/CS0 cs (/DeviceRGB)", "RG"];
    let objects = [profile_stream(3, rgb_profile([1, 2, 0]))];
    let calibrated = colours(content, "<< /ColorSpace << /DefaultRGB [/ICCBased 5 0 R] /CS0 /DeviceRGB >> >>", &objects, &points);
    for (got, what) in calibrated.into_iter().zip(what) {
        assert_near(got, GREEN, &format!("{what} with DefaultRGB"));
    }
    let device = colours(content, "<< /ColorSpace << /CS0 /DeviceRGB >> >>", &objects, &points);
    for (got, what) in device.into_iter().zip(what) {
        assert_eq!(got, RED, "{what} without DefaultRGB");
    }
}

/// `k`, `K` and `/DeviceCMYK cs` draw in the page's DefaultCMYK.
#[test]
fn device_cmyk_operators_draw_in_default_cmyk() {
    let content = "0 0 0 1 k 0 0 40 70 re f /DeviceCMYK cs 0 0 0 1 sc 45 0 40 70 re f 0 0 0 1 K 20 w 110 0 m 110 70 l S";
    let points = [(20, 35), (65, 35), (110, 35)];
    let what = ["k", "/DeviceCMYK cs", "K"];
    let objects = [profile_stream(4, cmyk_profile())];
    let calibrated = colours(content, "<< /ColorSpace << /DefaultCMYK [/ICCBased 5 0 R] >> >>", &objects, &points);
    for (got, what) in calibrated.into_iter().zip(what) {
        assert_near(got, INK_BLUE, &format!("{what} with DefaultCMYK"));
    }
    for (got, what) in colours(content, "<< >>", &objects, &points).into_iter().zip(what) {
        assert_dark(got, &format!("{what} without DefaultCMYK"));
    }
}

/// `g`, `G` and `/DeviceGray cs` draw in the page's DefaultGray.
#[test]
fn device_gray_operators_draw_in_default_gray() {
    let content = "0 g 0 0 40 70 re f /DeviceGray cs 0 sc 45 0 40 70 re f 0 G 20 w 110 0 m 110 70 l S";
    let points = [(20, 35), (65, 35), (110, 35)];
    let what = ["g", "/DeviceGray cs", "G"];
    let objects = [profile_stream(1, gray_profile())];
    let calibrated = colours(content, "<< /ColorSpace << /DefaultGray [/ICCBased 5 0 R] >> >>", &objects, &points);
    for (got, what) in calibrated.into_iter().zip(what) {
        assert_near(got, MID_GREY, &format!("{what} with DefaultGray"));
    }
    for (got, what) in colours(content, "<< >>", &objects, &points).into_iter().zip(what) {
        assert_eq!(got, BLACK, "{what} without DefaultGray");
    }
}

/// Images whose `/ColorSpace` is a device space are drawn in the default for it too: image
/// XObjects, inline images (whose `/RGB` and `/G` abbreviate the device spaces) and an inline
/// image naming a `/ColorSpace` resource that is DeviceRGB.
#[test]
fn device_images_draw_in_the_default_colour_spaces() {
    let content = "q 20 0 0 70 0 0 cm /R Do Q q 20 0 0 70 22 0 cm /G Do Q q 20 0 0 70 44 0 cm /K Do Q \
                   q 20 0 0 70 66 0 cm BI /W 1 /H 1 /CS /RGB /BPC 8 /F /AHx ID ff0000> EI Q \
                   q 20 0 0 70 88 0 cm BI /W 1 /H 1 /CS /CS0 /BPC 8 /F /AHx ID ff0000> EI Q \
                   q 20 0 0 70 110 0 cm BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 00> EI Q";
    let points = [(10, 35), (32, 35), (54, 35), (76, 35), (98, 35), (120, 35)];
    let image = |space: &str, data: &[u8]| {
        let dict = format!("<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /{space} /BitsPerComponent 8 /Length {} >>", data.len());
        (dict, data.to_vec())
    };
    let objects = [
        profile_stream(3, rgb_profile([1, 2, 0])),
        profile_stream(1, gray_profile()),
        profile_stream(4, cmyk_profile()),
        image("DeviceRGB", &[255, 0, 0]),
        image("DeviceGray", &[0]),
        image("DeviceCMYK", &[0, 0, 0, 255]),
    ];
    let x_objects = "/XObject << /R 8 0 R /G 9 0 R /K 10 0 R >>";
    let calibrated = colours(
        content,
        &format!(
            "<< /ColorSpace << /DefaultRGB [/ICCBased 5 0 R] /DefaultGray [/ICCBased 6 0 R] /DefaultCMYK [/ICCBased 7 0 R] /CS0 /DeviceRGB >> {x_objects} >>"
        ),
        &objects,
        &points,
    );
    assert_near(calibrated[0], GREEN, "DeviceRGB image");
    assert_near(calibrated[1], MID_GREY, "DeviceGray image");
    assert_near(calibrated[2], INK_BLUE, "DeviceCMYK image");
    assert_near(calibrated[3], GREEN, "inline /RGB image");
    assert_near(calibrated[4], GREEN, "inline image in /CS0 (/DeviceRGB)");
    assert_near(calibrated[5], MID_GREY, "inline /G image");

    let device = colours(content, &format!("<< /ColorSpace << /CS0 /DeviceRGB >> {x_objects} >>"), &objects, &points);
    assert_eq!(device[0], RED, "DeviceRGB image without DefaultRGB");
    assert_eq!(device[1], BLACK, "DeviceGray image without DefaultGray");
    assert_dark(device[2], "DeviceCMYK image without DefaultCMYK");
    assert_eq!(device[3], RED, "inline /RGB image without DefaultRGB");
    assert_eq!(device[4], RED, "inline /CS0 image without DefaultRGB");
    assert_eq!(device[5], BLACK, "inline /G image without DefaultGray");
}

/// Shadings whose `/ColorSpace` is a device space are drawn in the default for it too: painted
/// by `sh` or as a shading pattern, with their colours from a function (axial) or from their
/// vertices (a Type 4 mesh).
#[test]
fn device_shadings_draw_in_the_default_colour_spaces() {
    let axial = |space: &str, colour: &str| {
        format!(
            "<< /ShadingType 2 /ColorSpace /{space} /Coords [0 0 130 0] /Extend [true true] \
             /Function << /FunctionType 2 /Domain [0 1] /C0 [{colour}] /C1 [{colour}] /N 1 >> >>"
        )
    };
    // Two red triangles over the page: a flag, x, y and red, green and blue, a byte each.
    let mut mesh: Vec<u8> = Vec::new();
    for (x, y) in [(0, 0), (255, 0), (0, 255), (255, 0), (255, 255), (0, 255)] {
        mesh.extend_from_slice(&[0, x, y, 255, 0, 0]);
    }
    let objects = [
        profile_stream(3, rgb_profile([1, 2, 0])),
        profile_stream(1, gray_profile()),
        (
            format!(
                "<< /ShadingType 4 /ColorSpace /DeviceRGB /BitsPerCoordinate 8 /BitsPerComponent 8 /BitsPerFlag 8 \
                 /Decode [0 255 0 255 0 1 0 1 0 1] /Length {} >>",
                mesh.len()
            ),
            mesh,
        ),
    ];
    let content = "q 0 0 30 70 re W n /A sh Q /Pattern cs /P scn 32 0 30 70 re f \
                   q 64 0 30 70 re W n /M sh Q q 96 0 30 70 re W n /G sh Q";
    let points = [(15, 35), (47, 35), (79, 35), (111, 35)];
    let shadings = format!(
        "/Shading << /A {} /M 7 0 R /G {} >> /Pattern << /P << /PatternType 2 /Shading {} >> >>",
        axial("DeviceRGB", "1 0 0"),
        axial("DeviceGray", "0"),
        axial("DeviceRGB", "1 0 0")
    );
    let calibrated = colours(
        content,
        &format!("<< /ColorSpace << /DefaultRGB [/ICCBased 5 0 R] /DefaultGray [/ICCBased 6 0 R] >> {shadings} >>"),
        &objects,
        &points,
    );
    assert_near(calibrated[0], GREEN, "sh, an axial shading in DeviceRGB");
    assert_near(calibrated[1], GREEN, "a shading pattern in DeviceRGB");
    assert_near(calibrated[2], GREEN, "sh, a Type 4 mesh in DeviceRGB");
    assert_near(calibrated[3], MID_GREY, "sh, an axial shading in DeviceGray");

    let device = colours(content, &format!("<< {shadings} >>"), &objects, &points);
    assert_near(device[0], RED, "sh, an axial shading without DefaultRGB");
    assert_near(device[1], RED, "a shading pattern without DefaultRGB");
    assert_near(device[2], RED, "sh, a Type 4 mesh without DefaultRGB");
    assert_near(device[3], BLACK, "sh, an axial shading without DefaultGray");
}

/// One image drawn by a page with DefaultRGB and by a form that switches it off is decoded
/// for each: the decoded-image cache keeps the two apart (hayro-interpret patch 32 shares a
/// decode between every draw of an image).
#[test]
fn an_image_drawn_with_and_without_a_default_keeps_both_colours() {
    let objects = [
        profile_stream(3, rgb_profile([1, 2, 0])),
        (ONE_PIXEL_RGB.to_owned(), vec![255, 0, 0]),
        form("/Resources << /ColorSpace << /DefaultRGB /DeviceRGB >> >>", "q 40 0 0 70 70 0 cm /R Do Q"),
    ];
    let got = colours(
        "q 40 0 0 70 0 0 cm /R Do Q /F Do",
        "<< /ColorSpace << /DefaultRGB [/ICCBased 5 0 R] >> /XObject << /R 6 0 R /F 7 0 R >> >>",
        &objects,
        &[(20, 35), (90, 35)],
    );
    assert_near(got[0], GREEN, "the image on the page, in DefaultRGB");
    assert_eq!(got[1], RED, "the same image in the form without DefaultRGB");
}

/// A form XObject draws device colour in the default of the resources it is drawn with: its
/// own when it has one, else the page's (with or without a `/Resources` of its own, with a
/// `/ColorSpace` of its own that has no default, and through a form nested in it). Poppler and
/// MuPDF draw these forms in the page's default too, though §8.6.5.6 names only the current
/// resource dictionary. A form whose own default is a device space or not a usable default
/// draws device colour, not the page's default.
#[test]
fn forms_inherit_the_default_colour_spaces_they_do_not_override() {
    let blue = rgb_profile([2, 0, 1]);
    let objects = [
        profile_stream(3, rgb_profile([1, 2, 0])),
        // hayro caches a profile by its stream's dictionary, which here would be byte for byte
        // the first profile's, so this one says more.
        (format!("<< /N 3 /Alternate /DeviceRGB /Length {} >>", blue.len()), blue),
        form("", "1 0 0 rg 22 37 20 33 re f"),
        form("/Resources << /ProcSet [/PDF] >>", "1 0 0 rg 44 37 20 33 re f"),
        form("/Resources << /ColorSpace << /DefaultRGB [/ICCBased 6 0 R] >> /XObject << /N 12 0 R >> >>", "1 0 0 rg 66 37 20 33 re f /N Do"),
        form("/Resources << /ColorSpace << /DefaultRGB /DeviceRGB >> >>", "1 0 0 rg 88 37 20 33 re f"),
        form("/Resources << /ColorSpace << /DefaultRGB [/Lab << /WhitePoint [0.9642 1 0.8249] >>] >> >>", "1 0 0 rg 110 37 20 33 re f"),
        form("", "1 0 0 rg 0 0 20 33 re f"),
        form("/Resources << /ColorSpace << /CS1 /DeviceGray >> >>", "1 0 0 rg 22 0 20 33 re f"),
    ];
    let got = colours(
        "1 0 0 rg 0 37 20 33 re f /A Do /B Do /C Do /D Do /E Do /F Do",
        "<< /ColorSpace << /DefaultRGB [/ICCBased 5 0 R] >> /XObject << /A 7 0 R /B 8 0 R /C 9 0 R /D 10 0 R /E 11 0 R /F 13 0 R >> >>",
        &objects,
        &[(10, 17), (32, 17), (54, 17), (76, 17), (98, 17), (120, 17), (10, 53), (32, 53)],
    );
    assert_near(got[0], GREEN, "the page");
    assert_near(got[1], GREEN, "a form without /Resources");
    assert_near(got[2], GREEN, "a form whose /Resources has no /ColorSpace");
    assert_near(got[3], BLUE, "a form with a DefaultRGB of its own");
    assert_eq!(got[4], RED, "a form whose DefaultRGB is /DeviceRGB");
    assert_eq!(got[5], RED, "a form whose DefaultRGB is a Lab space");
    assert_near(got[6], BLUE, "a form nested in the one with its own DefaultRGB");
    assert_near(got[7], GREEN, "a form whose /ColorSpace has no DefaultRGB");
}

/// A default that cannot stand in for its device space (ISO 32000-2 §8.6.5.6: a Lab, Indexed
/// or Pattern space, another device space, one with the wrong number of components, or no
/// colour space at all) leaves device colour as it is, and the dictionary's other defaults
/// still apply.
#[test]
fn unusable_defaults_leave_device_colour_alone() {
    let objects =
        [profile_stream(1, gray_profile()), profile_stream(4, cmyk_profile()), (String::from("<< /N 3 /Length 13 >>"), b"not a profile".to_vec())];
    for default in [
        "[/Lab << /WhitePoint [0.9642 1 0.8249] >>]",
        "[/CalGray << /WhitePoint [0.9642 1 0.8249] >>]",
        "[/Indexed /DeviceRGB 0 <00FF00>]",
        "/Pattern",
        "[/Pattern /DeviceRGB]",
        "/DeviceCMYK",
        "/DeviceGray",
        "[/ICCBased 6 0 R]",
        "[/ICCBased 7 0 R]",
        "42",
        "[/NoSuchSpace]",
        "/DefaultRGB",
    ] {
        let got = colours(
            "1 0 0 rg 0 0 60 70 re f 0 g 70 0 60 70 re f",
            &format!("<< /ColorSpace << /DefaultRGB {default} /DefaultGray [/ICCBased 5 0 R] >> >>"),
            &objects,
            &[(30, 35), (100, 35)],
        );
        assert_eq!(got[0], RED, "DefaultRGB {default}: device RGB is drawn as it is");
        assert_near(got[1], MID_GREY, &format!("DefaultRGB {default}: DefaultGray still applies"));
    }
}
