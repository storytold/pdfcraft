//! `theme1.xml`: colour scheme and font scheme.
//!
//! In the file the colour scheme is ordered dk1, lt1, dk2, lt2, accent1–6, hlink, folHlink,
//! while cell colour `theme` indices (and the model's `Theme.colors`) use lt1, dk1, lt2, dk2, ….

use std::fmt::Write as _;

use gridcraft_model::Theme;

use crate::xml::{El, esc_attr};

const MAX_SCRIPT_FONTS: usize = 64;
const MAX_SCRIPT_FONT_CHARS: usize = 256;

const SLOTS: [(&str, usize); 12] = [
    ("dk1", 1),
    ("lt1", 0),
    ("dk2", 3),
    ("lt2", 2),
    ("accent1", 4),
    ("accent2", 5),
    ("accent3", 6),
    ("accent4", 7),
    ("accent5", 8),
    ("accent6", 9),
    ("hlink", 10),
    ("folHlink", 11),
];

fn scheme_rgb(e: &El) -> Option<u32> {
    if let Some(c) = e.child("srgbClr").and_then(|c| c.attr("val")) {
        return u32::from_str_radix(c.trim(), 16).ok().map(|v| v & 0xFFFFFF);
    }
    if let Some(c) = e.child("sysClr") {
        if let Some(v) = c.attr("lastClr").and_then(|v| u32::from_str_radix(v.trim(), 16).ok()) {
            return Some(v & 0xFFFFFF);
        }
        return match c.attr("val") {
            Some("windowText") => Some(0x000000),
            Some("window") => Some(0xFFFFFF),
            _ => None,
        };
    }
    None
}

/// The typeface of `<fontScheme>/<font>/<kind>` (`latin`, `ea` or `cs`), `None` when absent or
/// empty (an empty typeface means "not set" and must not overwrite the default).
fn typeface<'a>(fs: &'a El, font: &str, kind: &str) -> Option<&'a str> {
    fs.path(&[font, kind]).and_then(|e| e.attr("typeface")).filter(|f| !f.is_empty())
}

pub fn read_theme(root: &El) -> Theme {
    let mut t = Theme::default();
    if let Some(n) = root.attr("name") {
        t.name = n.to_string();
    }
    let Some(el) = root.child("themeElements") else { return t };
    if let Some(cs) = el.child("clrScheme") {
        for (name, idx) in SLOTS {
            if let Some(v) = cs.child(name).and_then(scheme_rgb)
                && let Some(slot) = t.colors.get_mut(idx)
            {
                *slot = v;
            }
        }
    }
    if let Some(fs) = el.child("fontScheme") {
        if let Some(f) = typeface(fs, "majorFont", "latin") {
            t.major_font = f.to_string();
        }
        if let Some(f) = typeface(fs, "minorFont", "latin") {
            t.minor_font = f.to_string();
        }
        if let Some(f) = typeface(fs, "majorFont", "ea") {
            t.major_font_ea = f.to_string();
        }
        if let Some(f) = typeface(fs, "minorFont", "ea") {
            t.minor_font_ea = f.to_string();
        }
        if let Some(f) = typeface(fs, "majorFont", "cs") {
            t.major_font_cs = f.to_string();
        }
        if let Some(f) = typeface(fs, "minorFont", "cs") {
            t.minor_font_cs = f.to_string();
        }
        t.major_font_scripts = fs.child("majorFont").map(script_fonts).unwrap_or_default();
        t.minor_font_scripts = fs.child("minorFont").map(script_fonts).unwrap_or_default();
    }
    t
}

/// The `<a:font script typeface>` entries of a font collection, in file order. A hostile file is
/// held to [`MAX_SCRIPT_FONTS`] entries of at most [`MAX_SCRIPT_FONT_CHARS`] characters each.
fn script_fonts(collection: &El) -> Vec<(String, String)> {
    let capped = |v: Option<&str>| v.unwrap_or_default().chars().take(MAX_SCRIPT_FONT_CHARS).collect::<String>();
    collection.kids("font").take(MAX_SCRIPT_FONTS).map(|e| (capped(e.attr("script")), capped(e.attr("typeface")))).collect()
}

fn script_fonts_xml(fonts: &[(String, String)]) -> String {
    fonts.iter().map(|(script, face)| format!("<a:font script=\"{}\" typeface=\"{}\"/>", esc_attr(script), esc_attr(face))).collect()
}

/// A minimal, schema-complete theme part for `t`.
pub fn write_theme(t: &Theme) -> String {
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    let name = if t.name.is_empty() { "Craft" } else { &t.name };
    let _ = write!(s, "<a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"{}\"><a:themeElements>", esc_attr(name));
    let _ = write!(s, "<a:clrScheme name=\"{}\">", esc_attr(name));
    for (slot, idx) in SLOTS {
        let rgb = t.colors.get(idx).copied().unwrap_or(0) & 0xFFFFFF;
        let _ = write!(s, "<a:{slot}><a:srgbClr val=\"{rgb:06X}\"/></a:{slot}>");
    }
    s.push_str("</a:clrScheme>");
    let _ = write!(
        s,
        "<a:fontScheme name=\"{}\"><a:majorFont><a:latin typeface=\"{}\"/><a:ea typeface=\"{}\"/><a:cs typeface=\"{}\"/>{}</a:majorFont><a:minorFont><a:latin typeface=\"{}\"/><a:ea typeface=\"{}\"/><a:cs typeface=\"{}\"/>{}</a:minorFont></a:fontScheme>",
        esc_attr(name),
        esc_attr(&t.major_font),
        esc_attr(&t.major_font_ea),
        esc_attr(&t.major_font_cs),
        script_fonts_xml(&t.major_font_scripts),
        esc_attr(&t.minor_font),
        esc_attr(&t.minor_font_ea),
        esc_attr(&t.minor_font_cs),
        script_fonts_xml(&t.minor_font_scripts)
    );
    // Format scheme: three entries per list, as the schema requires.
    s.push_str("<a:fmtScheme name=\"Craft\"><a:fillStyleLst>");
    for _ in 0..3 {
        s.push_str("<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>");
    }
    s.push_str("</a:fillStyleLst><a:lnStyleLst>");
    for w in [6350, 12700, 19050] {
        let _ = write!(
            s,
            "<a:ln w=\"{w}\" cap=\"flat\" cmpd=\"sng\" algn=\"ctr\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:prstDash val=\"solid\"/></a:ln>"
        );
    }
    s.push_str("</a:lnStyleLst><a:effectStyleLst>");
    for _ in 0..3 {
        s.push_str("<a:effectStyle><a:effectLst/></a:effectStyle>");
    }
    s.push_str("</a:effectStyleLst><a:bgFillStyleLst>");
    for _ in 0..3 {
        s.push_str("<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>");
    }
    s.push_str("</a:bgFillStyleLst></a:fmtScheme></a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Japanese Office theme: the heading font sets latin, ea and cs; the body font only ea.
    const EA_THEME: &str = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office Theme"><a:themeElements><a:clrScheme name="Office"/><a:fontScheme name="Office"><a:majorFont><a:latin typeface="Calibri"/><a:ea typeface="游ゴシック"/><a:cs typeface="Arial"/></a:majorFont><a:minorFont><a:ea typeface="ＭＳ Ｐゴシック"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"#;

    #[test]
    fn roundtrip() {
        let mut t = Theme::default();
        t.colors[0] = 0xFEFEFE;
        t.colors[1] = 0x101010;
        t.colors[9] = 0x123456;
        t.major_font = "Cambria".into();
        t.name = "Mine".into();
        let x = write_theme(&t);
        let back = read_theme(&crate::xml::parse(x.as_bytes()).unwrap());
        assert_eq!(back, t);
    }

    #[test]
    fn ea_and_cs_typefaces_roundtrip() {
        let t = read_theme(&crate::xml::parse(EA_THEME.as_bytes()).unwrap());
        assert_eq!(t.major_font, "Calibri");
        assert_eq!(t.major_font_ea, "游ゴシック");
        assert_eq!(t.major_font_cs, "Arial");
        // No `<a:latin>` under the minor font: the default stays.
        assert_eq!(t.minor_font, Theme::default().minor_font);
        assert_eq!(t.minor_font_ea, "ＭＳ Ｐゴシック");
        assert!(t.minor_font_cs.is_empty());

        let x = write_theme(&t);
        assert!(
            x.contains(r#"<a:majorFont><a:latin typeface="Calibri"/><a:ea typeface="游ゴシック"/><a:cs typeface="Arial"/></a:majorFont>"#),
            "{x}"
        );
        assert!(
            x.contains(r#"<a:minorFont><a:latin typeface="Calibri"/><a:ea typeface="ＭＳ Ｐゴシック"/><a:cs typeface=""/></a:minorFont>"#),
            "{x}"
        );
        assert_eq!(read_theme(&crate::xml::parse(x.as_bytes()).unwrap()), t);
    }

    #[test]
    fn theme_without_ea_or_cs_writes_empty_attributes() {
        let t = Theme::default();
        let x = write_theme(&t);
        assert!(x.contains(r#"<a:majorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont>"#), "{x}");
        assert!(x.contains(r#"<a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont>"#), "{x}");
    }

    #[test]
    fn explicit_empty_ea_and_cs_read_as_unset() {
        let xml = EA_THEME
            .replace(r#"<a:ea typeface="游ゴシック"/>"#, r#"<a:ea typeface=""/>"#)
            .replace(r#"<a:ea typeface="ＭＳ Ｐゴシック"/>"#, r#"<a:ea typeface=""/>"#);
        let t = read_theme(&crate::xml::parse(xml.as_bytes()).unwrap());
        assert!(t.major_font_ea.is_empty() && t.minor_font_ea.is_empty());
        assert_eq!(t.major_font_cs, "Arial");
        let x = write_theme(&t);
        assert!(x.contains(r#"<a:majorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface="Arial"/></a:majorFont>"#), "{x}");
        assert_eq!(read_theme(&crate::xml::parse(x.as_bytes()).unwrap()), t);
    }

    #[test]
    fn ea_and_cs_typefaces_are_escaped() {
        let t = Theme { major_font_ea: "游&ゴシック".into(), minor_font_cs: "A<B>\"C\"".into(), ..Theme::default() };
        let x = write_theme(&t);
        assert!(x.contains(r#"<a:ea typeface="游&amp;ゴシック"/>"#), "{x}");
        assert!(x.contains(r#"<a:cs typeface="A&lt;B&gt;&quot;C&quot;"/>"#), "{x}");
        assert_eq!(read_theme(&crate::xml::parse(x.as_bytes()).unwrap()), t);
    }

    /// An Arabic Office theme: Arabic, Hebrew and Japanese faces in the per-script lists.
    const SCRIPT_THEME: &str = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office Theme"><a:themeElements><a:fontScheme name="Office"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface="Times New Roman"/><a:font script="Arab" typeface="Times New Roman"/><a:font script="Hebr" typeface="Times New Roman"/><a:font script="Jpan" typeface="游ゴシック Light"/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface="Arial"/><a:font script="Arab" typeface="Arial"/><a:font script="Hebr" typeface="Arial"/><a:font script="Jpan" typeface="游ゴシック"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"#;

    #[test]
    fn per_script_typefaces_roundtrip_in_file_order() {
        let t = read_theme(&crate::xml::parse(SCRIPT_THEME.as_bytes()).unwrap());
        let pairs = |v: &[(String, String)]| v.iter().map(|(s, f)| (s.clone(), f.clone())).collect::<Vec<_>>();
        let expect =
            |a: &str, j: &str| vec![("Arab".to_string(), a.to_string()), ("Hebr".to_string(), a.to_string()), ("Jpan".to_string(), j.to_string())];
        assert_eq!(pairs(&t.major_font_scripts), expect("Times New Roman", "游ゴシック Light"));
        assert_eq!(pairs(&t.minor_font_scripts), expect("Arial", "游ゴシック"));
        let x = write_theme(&t);
        assert!(
            x.contains(r#"<a:cs typeface="Times New Roman"/><a:font script="Arab" typeface="Times New Roman"/><a:font script="Hebr" typeface="Times New Roman"/><a:font script="Jpan" typeface="游ゴシック Light"/></a:majorFont>"#),
            "{x}"
        );
        assert_eq!(read_theme(&crate::xml::parse(x.as_bytes()).unwrap()), t);
    }

    #[test]
    fn hostile_per_script_lists_are_capped() {
        let entries: String = (0..100).map(|i| format!(r#"<a:font script="S{i}" typeface="{}"/>"#, "x".repeat(300))).collect();
        let xml = format!(
            r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:themeElements><a:fontScheme><a:majorFont><a:latin typeface="Calibri"/>{entries}</a:majorFont></a:fontScheme></a:themeElements></a:theme>"#
        );
        let t = read_theme(&crate::xml::parse(xml.as_bytes()).unwrap());
        assert_eq!(t.major_font_scripts.len(), 64);
        assert!(t.major_font_scripts.iter().all(|(script, face)| script.chars().count() <= 256 && face.chars().count() <= 256));
    }
}
