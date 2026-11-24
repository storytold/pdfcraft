//! DrawingML colours for DeckCraft: `Rgba`, scheme slots, colour schemes and colour transforms.
//! They live in craft-libs (`craft_drawingml::color`, craft-libs#778 and #792), shared with the
//! PPTX reader and writer; this crate re-exports them under the name the app uses.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub use craft_drawingml::color::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_hex_rejects_multiple_hashes() {
        assert_eq!(Rgba::from_hex("##FF0000"), None);
        assert_eq!(Rgba::from_hex("####FF0000"), None);
        assert_eq!(Rgba::from_hex("#FF0000"), Some(Rgba::rgb(255, 0, 0)));
        assert_eq!(Rgba::from_hex("FF0000"), Some(Rgba::rgb(255, 0, 0)));
        assert_eq!(Rgba::from_hex("#FF000080"), Some(Rgba::rgba(255, 0, 0, 128)));
    }

    #[test]
    fn from_hex_rejects_signs() {
        assert_eq!(Rgba::from_hex("+12345"), None);
        assert_eq!(Rgba::from_hex("#+1+2+3"), None);
        assert_eq!(Rgba::from_hex("+1+2+3+4"), None);
        assert_eq!(Rgba::from_hex("+12"), None);
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(Rgba::from_hex("#156082"), Some(Rgba::rgb(0x15, 0x60, 0x82)));
        assert_eq!(Rgba::rgb(0x15, 0x60, 0x82).hex(), "156082");
        assert_eq!(Rgba::from_hex("fff"), Some(Rgba::WHITE));
        assert_eq!(Rgba::from_hex("zz0000"), None);
        assert_eq!(Rgba::from_hex("ééé"), None);
        assert_eq!(Rgba::from_hex(""), None);
    }

    #[test]
    fn hsl_roundtrip() {
        for c in [Rgba::rgb(10, 200, 30), Rgba::rgb(255, 0, 0), Rgba::rgb(128, 128, 128), Rgba::rgb(21, 96, 130)] {
            let (h, s, l) = c.to_hsl();
            let d = Rgba::from_hsl(h, s, l, 255);
            assert!(
                (d.r as i32 - c.r as i32).abs() <= 1 && (d.g as i32 - c.g as i32).abs() <= 1 && (d.b as i32 - c.b as i32).abs() <= 1,
                "{c:?} {d:?}"
            );
        }
    }

    #[test]
    fn transforms() {
        let c = Rgba::rgb(0x40, 0x80, 0xC0);
        assert_eq!(apply(c, &[ColorTransform::Alpha(50000)]).a, 128);
        let lighter = apply(c, &[ColorTransform::LumMod(20000), ColorTransform::LumOff(80000)]);
        assert!(lighter.luminance() > c.luminance());
        let darker = apply(c, &[ColorTransform::LumMod(50000)]);
        assert!(darker.luminance() < c.luminance());
        assert_eq!(apply(Rgba::WHITE, &[ColorTransform::Shade(0)]), Rgba::BLACK);
        assert_eq!(apply(Rgba::BLACK, &[ColorTransform::Tint(0)]), Rgba::WHITE);
        assert_eq!(apply(c, &[ColorTransform::Inv]), Rgba::rgb(0xBF, 0x7F, 0x3F));
        // hostile values never panic
        let _ = apply(c, &[ColorTransform::LumMod(i32::MAX), ColorTransform::HueOff(i32::MIN), ColorTransform::AlphaMod(i32::MIN)]);
    }

    #[test]
    fn contrast_ratio() {
        assert!((Rgba::BLACK.contrast(Rgba::WHITE) - 21.0).abs() < 0.01);
        assert!((Rgba::WHITE.contrast(Rgba::WHITE) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn scheme_slots() {
        let mut s = ColorScheme::new("t", [Rgba::BLACK; 12]);
        s.set(SchemeSlot::Accent3, Rgba::WHITE);
        assert_eq!(s.get(SchemeSlot::Accent3), Rgba::WHITE);
        assert_eq!(s.get(SchemeSlot::Tx1), Rgba::BLACK);
        for slot in SchemeSlot::THEME {
            assert_eq!(SchemeSlot::from_xml(slot.xml_name()), Some(slot));
        }
    }
}
