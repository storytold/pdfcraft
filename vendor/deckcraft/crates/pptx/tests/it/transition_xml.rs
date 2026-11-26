//! Transition XML as other applications write it, spliced into an exported deck and read back.

use std::io::{Cursor, Read, Write};

use deckcraft_model::anim::Transition;
use deckcraft_model::defaults;

const P159: &str = "http://schemas.microsoft.com/office/powerpoint/2015/09/main";
const P14: &str = "http://schemas.microsoft.com/office/powerpoint/2010/main";

/// Exports a one-slide deck, replaces the slide's `mc:AlternateContent` transition block with
/// `xml` and returns the imported transition of that slide.
fn import_with(xml: &str) -> Transition {
    let mut p = defaults::new_presentation(None);
    std::sync::Arc::make_mut(&mut p.slides[0]).transition = Some(Transition { kind: "fade".into(), duration_ms: 3000, ..Default::default() });
    let bytes = deckcraft_pptx::export(&p).expect("export");
    let mut z = zip::ZipArchive::new(Cursor::new(&bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default();
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        let mut d = vec![];
        f.read_to_end(&mut d).expect("read");
        if f.name() == "ppt/slides/slide1.xml" {
            let s = String::from_utf8(d).expect("utf8");
            let t = s.find("<p:transition").expect("transition");
            let a = s[..t].rfind("<mc:AlternateContent").expect("mc start");
            let end = "</mc:AlternateContent>";
            let b = t + s[t..].find(end).expect("mc end") + end.len();
            d = format!("{}{xml}{}", &s[..a], &s[b..]).into_bytes();
        }
        out.start_file(f.name(), o).expect("start");
        out.write_all(&d).expect("write");
    }
    let bytes = out.finish().expect("finish").into_inner();
    let q = deckcraft_pptx::import(&bytes).expect("import");
    q.slides[0].transition.clone().expect("transition")
}

fn mc(choice_ns: &str, req: &str, choice: &str, fallback: &str) -> String {
    format!(
        r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice xmlns:{req}="{choice_ns}" Requires="{req}">{choice}</mc:Choice><mc:Fallback>{fallback}</mc:Fallback></mc:AlternateContent>"#
    )
}

/// PowerPoint 2021/2024 write a 0.5 s Morph with neither `spd` nor `p14:dur` (issue #117).
#[test]
fn powerpoint_half_second_morph_without_spd() {
    let xml = mc(P159, "p159", r#"<p:transition><p159:morph option="byObject"/></p:transition>"#, "<p:transition><p:fade/></p:transition>");
    let t = import_with(&xml);
    assert_eq!((t.kind.as_str(), t.duration_ms), ("morph", 500));
}

#[test]
fn missing_spd_is_fast_for_a_plain_transition() {
    let t = import_with("<p:transition><p:fade/></p:transition>");
    assert_eq!((t.kind.as_str(), t.duration_ms), ("fade", 500));
}

#[test]
fn spd_and_dur_still_apply() {
    let t = import_with(r#"<p:transition spd="med"><p:fade/></p:transition>"#);
    assert_eq!(t.duration_ms, 750);
    let xml = mc(
        P159,
        "p159",
        &format!(r#"<p:transition xmlns:p14="{P14}" spd="slow" p14:dur="1250"><p159:morph option="byObject"/></p:transition>"#),
        r#"<p:transition spd="slow"><p:fade/></p:transition>"#,
    );
    let t = import_with(&xml);
    assert_eq!((t.kind.as_str(), t.duration_ms), ("morph", 1250));
}
