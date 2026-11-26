//! PowerPoint places an effect at the sum of its enclosing time nodes' delays, so a file can list
//! effects out of time order. The BNI meeting-deck generator writes a 1 s countdown chain first,
//! then "swap to the visual at 5 s" effects placed only by their parent `p:par` delay: read in file
//! order, those all fired together when the countdown ended.

use std::io::{Cursor, Read, Write};

use deckcraft_geom::Xfrm;
use deckcraft_model::anim::AnimStart;
use deckcraft_model::{Shape, ShapeId, defaults};

/// An "appear"/"disappear" effect (preset 1) in its own `p:par`, as PowerPoint writes it.
fn eff(id: u32, class: &str, node: &str, delay: u32, spid: u32) -> String {
    let vis = if class == "entr" { "visible" } else { "hidden" };
    format!(
        r#"<p:par><p:cTn id="{id}" presetID="1" presetClass="{class}" presetSubtype="0" fill="hold" nodeType="{node}"><p:stCondLst><p:cond delay="{delay}"/></p:stCondLst><p:childTnLst><p:set><p:cBhvr><p:cTn id="{}" dur="1" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{spid}"/></p:tgtEl><p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst></p:cBhvr><p:to><p:strVal val="{vis}"/></p:to></p:set></p:childTnLst></p:cTn></p:par>"#,
        id + 1000
    )
}

/// A group `p:par` starting `delay` ms into the click, holding `inner`.
fn group(id: u32, delay: u32, inner: &str) -> String {
    format!(
        r#"<p:par><p:cTn id="{id}" fill="hold"><p:stCondLst><p:cond delay="{delay}"/></p:stCondLst><p:childTnLst>{inner}</p:childTnLst></p:cTn></p:par>"#
    )
}

fn import_with_timing(build: impl Fn(&[u32]) -> String) -> Vec<(String, AnimStart, u32)> {
    let mut p = defaults::new_presentation(None);
    let shapes: Vec<Shape> = ["Three", "Two", "One", "Visual"]
        .into_iter()
        .map(|name| Shape {
            id: ShapeId(p.alloc_id().expect("id")),
            name: name.into(),
            xfrm: Some(Xfrm::new(10.0, 10.0, 100.0, 50.0)),
            ..Default::default()
        })
        .collect();
    std::sync::Arc::make_mut(&mut p.slides[0]).shapes = shapes;
    let mut z = zip::ZipArchive::new(Cursor::new(deckcraft_pptx::export(&p).expect("export"))).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let mut f = z.by_index(i).expect("entry");
        let name = f.name().to_string();
        let mut d = vec![];
        f.read_to_end(&mut d).expect("read");
        if name == "ppt/slides/slide1.xml" {
            let xml = String::from_utf8(d).expect("utf8");
            let spids: Vec<u32> = ["Three", "Two", "One", "Visual"]
                .iter()
                .map(|n| {
                    let at = xml.find(&format!(r#"name="{n}""#)).expect("shape written");
                    let open = xml[..at].rfind("<p:cNvPr ").expect("cNvPr");
                    let tag = &xml[open..at];
                    let id = tag.split(r#"id=""#).nth(1).expect("id").split('"').next().expect("id");
                    id.parse().expect("number")
                })
                .collect();
            let timing = format!(
                r#"<p:timing><p:tnLst><p:par><p:cTn id="1" dur="indefinite" restart="never" nodeType="tmRoot"><p:childTnLst><p:seq concurrent="1" nextAc="seek"><p:cTn id="2" dur="indefinite" nodeType="mainSeq"><p:childTnLst><p:par><p:cTn id="3" fill="hold"><p:stCondLst><p:cond delay="indefinite"/></p:stCondLst><p:childTnLst>{}</p:childTnLst></p:cTn></p:par></p:childTnLst></p:cTn><p:prevCondLst><p:cond evt="onPrev" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:prevCondLst><p:nextCondLst><p:cond evt="onNext" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:nextCondLst></p:seq></p:childTnLst></p:cTn></p:par></p:tnLst></p:timing>"#,
                build(&spids)
            );
            let xml = match (xml.find("<p:timing>"), xml.find("</p:timing>")) {
                (Some(a), Some(b)) => format!("{}{}{}", &xml[..a], timing, &xml[b + "</p:timing>".len()..]),
                _ => xml.replacen("</p:sld>", &format!("{timing}</p:sld>"), 1),
            };
            d = xml.into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::default()).expect("start");
        out.write_all(&d).expect("write");
    }
    let q = deckcraft_pptx::import(&out.finish().expect("finish").into_inner()).expect("import");
    let s = &q.slides[0];
    s.animations
        .iter()
        .map(|a| {
            let name = s.shapes.iter().find(|x| x.id == a.shape).map(|x| x.name.clone()).unwrap_or_default();
            (name, a.start, a.delay_ms)
        })
        .collect()
}

/// Start (ms after the click) of each effect when the flat list is played effect after effect.
fn flat(effects: &[(String, AnimStart, u32)]) -> Vec<(String, u32)> {
    let (mut begin, mut end) = (0u32, 0u32);
    effects
        .iter()
        .map(|(n, start, delay)| {
            if *start == AnimStart::AfterPrevious {
                begin = end;
            }
            let t = begin + delay;
            end = end.max(t);
            (n.clone(), t)
        })
        .collect()
}

#[test]
fn effects_placed_by_parent_delay_play_when_powerpoint_plays_them() {
    // Click: "Three" goes, then "Two" after 1 s, "One" after 1 s more (a countdown chain), then —
    // written last — the visual appears at 1 s, placed only by its parent's delay.
    let fx = import_with_timing(|s| {
        [
            group(10, 0, &eff(11, "exit", "clickEffect", 0, s[0])),
            group(20, 0, &eff(21, "exit", "afterEffect", 1000, s[1])),
            group(30, 1000, &eff(31, "exit", "afterEffect", 1000, s[2])),
            group(40, 1000, &eff(41, "entr", "afterEffect", 0, s[3])),
        ]
        .concat()
    });
    let got = flat(&fx);
    let at = |n: &str| got.iter().find(|(x, _)| x == n).map(|(_, t)| *t);
    assert_eq!((at("Three"), at("Two"), at("Visual"), at("One")), (Some(0), Some(1000), Some(1000), Some(2000)), "{fx:?}");
}

#[test]
fn a_timeline_already_in_order_is_read_unchanged() {
    let fx = import_with_timing(|s| {
        [
            group(10, 0, &[eff(11, "exit", "clickEffect", 0, s[0]), eff(12, "entr", "withEffect", 500, s[3])].concat()),
            group(20, 500, &eff(21, "exit", "afterEffect", 1000, s[1])),
        ]
        .concat()
    });
    let starts: Vec<_> = fx.iter().map(|(n, st, d)| (n.as_str(), *st, *d)).collect();
    assert_eq!(starts, [("Three", AnimStart::OnClick, 0), ("Visual", AnimStart::WithPrevious, 500), ("Two", AnimStart::AfterPrevious, 1000)]);
}

#[test]
fn a_delayed_click_effect_sorted_after_a_with_effect_keeps_the_click_on_the_group() {
    // The click effect waits 500 ms; a with-effect in the same click plays at once, and a later
    // effect is placed only by its parent's delay. Sorted, the with-effect plays first and takes
    // the click, so the group still waits for one.
    let fx = import_with_timing(|s| {
        [
            group(10, 0, &[eff(11, "exit", "clickEffect", 500, s[0]), eff(12, "entr", "withEffect", 0, s[3])].concat()),
            group(20, 1000, &eff(21, "exit", "afterEffect", 0, s[1])),
        ]
        .concat()
    });
    let starts: Vec<_> = fx.iter().map(|(n, st, _)| (n.as_str(), *st)).collect();
    assert_eq!(starts[0], ("Visual", AnimStart::OnClick), "{fx:?}");
    assert_eq!(starts.iter().filter(|(_, st)| *st == AnimStart::OnClick).count(), 1, "{fx:?}");
}
