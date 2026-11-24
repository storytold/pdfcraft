//! Shape geometry: OfficeArt shape types (`MSOSPT`, [MS-ODRAW] §2.4.24) as DrawingML preset
//! names, and freeform paths from the vertices and segment properties (§2.3.6).

use deckcraft_model::{CustomPath, Geom};

use crate::draw::{Props, prop};

/// The preset for a shape type, `None` for a freeform (`msosptNotPrimitive`) or a type with no
/// preset counterpart.
pub fn preset_name(shape_type: u16) -> Option<&'static str> {
    Some(match shape_type {
        1 | 75 | 202 | 201 => "rect",
        2 => "roundRect",
        3 => "ellipse",
        4 => "diamond",
        5 => "triangle",
        6 => "rtTriangle",
        7 => "parallelogram",
        8 => "trapezoid",
        9 => "hexagon",
        10 => "octagon",
        11 => "plus",
        12 => "star5",
        13 | 14 => "rightArrow",
        15 => "homePlate",
        16 => "cube",
        17 => "wedgeRoundRectCallout",
        18 => "star16",
        19 => "arc",
        20 => "line",
        21 => "plaque",
        22 => "can",
        23 => "donut",
        32 => "straightConnector1",
        33 => "bentConnector2",
        34 => "bentConnector3",
        35 => "bentConnector4",
        36 => "bentConnector5",
        37 => "curvedConnector2",
        38 => "curvedConnector3",
        39 => "curvedConnector4",
        40 => "curvedConnector5",
        41 | 178 => "callout1",
        42 => "callout2",
        43 => "callout3",
        44 | 179 => "accentCallout1",
        45 => "accentCallout2",
        46 => "accentCallout3",
        47 | 180 => "borderCallout1",
        48 => "borderCallout2",
        49 => "borderCallout3",
        50 | 181 => "accentBorderCallout1",
        51 => "accentBorderCallout2",
        52 => "accentBorderCallout3",
        53 => "ribbon",
        54 => "ribbon2",
        55 => "chevron",
        56 => "pentagon",
        57 => "noSmoking",
        58 => "star8",
        59 => "star16",
        60 => "star32",
        61 => "wedgeRectCallout",
        62 => "wedgeRoundRectCallout",
        63 => "wedgeEllipseCallout",
        64 => "wave",
        65 => "foldedCorner",
        66 => "leftArrow",
        67 => "downArrow",
        68 => "upArrow",
        69 => "leftRightArrow",
        70 => "upDownArrow",
        71 => "irregularSeal1",
        72 => "irregularSeal2",
        73 => "lightningBolt",
        74 => "heart",
        76 => "quadArrow",
        77 => "leftArrowCallout",
        78 => "rightArrowCallout",
        79 => "upArrowCallout",
        80 => "downArrowCallout",
        81 => "leftRightArrowCallout",
        82 => "upDownArrowCallout",
        83 => "quadArrowCallout",
        84 => "bevel",
        85 => "leftBracket",
        86 => "rightBracket",
        87 => "leftBrace",
        88 => "rightBrace",
        89 => "leftUpArrow",
        90 => "bentUpArrow",
        91 => "bentArrow",
        92 => "star24",
        93 => "stripedRightArrow",
        94 => "notchedRightArrow",
        95 => "blockArc",
        96 => "smileyFace",
        97 => "verticalScroll",
        98 => "horizontalScroll",
        99 | 100 => "circularArrow",
        101 => "uturnArrow",
        102 => "curvedRightArrow",
        103 => "curvedLeftArrow",
        104 => "curvedUpArrow",
        105 => "curvedDownArrow",
        106 => "cloudCallout",
        107 => "ellipseRibbon",
        108 => "ellipseRibbon2",
        109 => "flowChartProcess",
        110 => "flowChartDecision",
        111 => "flowChartInputOutput",
        112 => "flowChartPredefinedProcess",
        113 => "flowChartInternalStorage",
        114 => "flowChartDocument",
        115 => "flowChartMultidocument",
        116 => "flowChartTerminator",
        117 => "flowChartPreparation",
        118 => "flowChartManualInput",
        119 => "flowChartManualOperation",
        120 => "flowChartConnector",
        121 => "flowChartPunchedCard",
        122 => "flowChartPunchedTape",
        123 => "flowChartSummingJunction",
        124 => "flowChartOr",
        125 => "flowChartCollate",
        126 => "flowChartSort",
        127 => "flowChartExtract",
        128 => "flowChartMerge",
        129 => "flowChartOfflineStorage",
        130 => "flowChartOnlineStorage",
        131 => "flowChartMagneticTape",
        132 => "flowChartMagneticDisk",
        133 => "flowChartMagneticDrum",
        134 => "flowChartDisplay",
        135 => "flowChartDelay",
        176 => "flowChartAlternateProcess",
        177 => "flowChartOffpageConnector",
        182 => "leftRightUpArrow",
        183 => "sun",
        184 => "moon",
        185 => "bracketPair",
        186 => "bracePair",
        187 => "star4",
        188 => "doubleWave",
        189 => "actionButtonBlank",
        190 => "actionButtonHome",
        191 => "actionButtonHelp",
        192 => "actionButtonInformation",
        193 => "actionButtonForwardNext",
        194 => "actionButtonBackPrevious",
        195 => "actionButtonEnd",
        196 => "actionButtonBeginning",
        197 => "actionButtonReturn",
        198 => "actionButtonDocument",
        199 => "actionButtonSound",
        200 => "actionButtonMovie",
        // WordArt text shapes (24–31, 136–175) draw their text as plain text in a rectangle.
        24..=31 | 136..=175 => "rect",
        _ => return None,
    })
}

/// Whether a shape type is a connector.
pub fn is_connector(shape_type: u16) -> bool {
    (32..=40).contains(&shape_type)
}

/// The geometry of a shape: its preset (checked against the presets DeckCraft draws, with a
/// rounded rectangle's corner carried over), else its freeform path, else a rectangle.
pub fn geometry(shape_type: u16, props: &Props) -> Geom {
    if let Some(name) = preset_name(shape_type).filter(|n| deckcraft_geom::preset::build(n, 100.0, 100.0, &[]).is_some()) {
        let mut adj = vec![];
        // A rounded rectangle's corner: a 21600 coordinate in OfficeArt, a 100000 fraction of the
        // shorter side in DrawingML.
        if shape_type == 2
            && let Some(v) = props.get(prop::ADJUST_VALUE)
        {
            adj.push(f64::from(v as i32).clamp(0.0, 10800.0) / 21600.0 * 100_000.0);
        }
        return Geom::Preset { name: name.into(), adj };
    }
    custom_path(props).map(|paths| Geom::Custom { paths }).unwrap_or_default()
}

/// Segments followed at most in one path.
const MAX_SEGMENTS: usize = 1 << 16;

/// A freeform's path in its geometry box.
fn custom_path(props: &Props) -> Option<Vec<CustomPath>> {
    let (size, verts) = props.array(prop::VERTICES)?;
    let points: Vec<(f64, f64)> = match size {
        8 => verts
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| (f64::from(i32::from_le_bytes([c[0], c[1], c[2], c[3]])), f64::from(i32::from_le_bytes([c[4], c[5], c[6], c[7]]))))
            .collect(),
        4 => verts
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| (f64::from(i16::from_le_bytes([c[0], c[1]])), f64::from(i16::from_le_bytes([c[2], c[3]]))))
            .collect(),
        _ => return None,
    };
    if points.is_empty() {
        return None;
    }
    let left = f64::from(props.get(prop::GEO_LEFT).unwrap_or(0) as i32);
    let top = f64::from(props.get(prop::GEO_TOP).unwrap_or(0) as i32);
    let right = f64::from(props.get(prop::GEO_RIGHT).unwrap_or(21600) as i32);
    let bottom = f64::from(props.get(prop::GEO_BOTTOM).unwrap_or(21600) as i32);
    let (w, h) = ((right - left).abs().max(1.0), (bottom - top).abs().max(1.0));
    let mut pts = points.iter().map(|(x, y)| (x - left, y - top));
    let mut d = String::new();
    let mut push = |cmd: &str, ps: &[(f64, f64)]| {
        d.push_str(cmd);
        for (x, y) in ps {
            d.push_str(&format!(" {x} {y}"));
        }
        d.push(' ');
    };
    match props.array(prop::SEGMENT_INFO) {
        Some((2, segs)) => {
            for c in segs.as_chunks::<2>().0.iter().take(MAX_SEGMENTS) {
                let v = u16::from_le_bytes(*c);
                let count = usize::from(v & 0x1FFF);
                match v >> 13 {
                    // lineTo
                    0 => {
                        for _ in 0..count.max(1) {
                            let Some(p) = pts.next() else { break };
                            push("L", &[p]);
                        }
                    }
                    // curveTo: three points per segment
                    1 => {
                        for _ in 0..count.max(1) {
                            let ps: Vec<_> = pts.by_ref().take(3).collect();
                            if ps.len() < 3 {
                                break;
                            }
                            push("C", &ps);
                        }
                    }
                    2 => {
                        let Some(p) = pts.next() else { break };
                        push("M", &[p]);
                    }
                    3 => push("Z", &[]),
                    4 => break,
                    // Escapes (arcs, fill/stroke switches) carry no points we draw.
                    _ => {}
                }
            }
        }
        // No segments: a polyline through every vertex.
        _ => {
            if let Some(p) = pts.next() {
                push("M", &[p]);
            }
            for p in pts {
                push("L", &[p]);
            }
        }
    }
    let d = d.trim_end().to_string();
    if !d.starts_with('M') {
        return None;
    }
    Some(vec![CustomPath { w, h, d, ..Default::default() }])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::record::Rec;
    use crate::record::build::atom;

    fn props(entries: &[(u16, u32)], complex: &[u8]) -> Vec<u8> {
        let mut b = vec![];
        for (id, op) in entries {
            b.extend_from_slice(&id.to_le_bytes());
            b.extend_from_slice(&op.to_le_bytes());
        }
        b.extend_from_slice(complex);
        atom(crate::doc::rt::FOPT, entries.len() as u16, &b)
    }

    fn array(size: u16, elems: &[u8]) -> Vec<u8> {
        let n = (elems.len() / usize::from(size)) as u16;
        [&n.to_le_bytes()[..], &n.to_le_bytes(), &size.to_le_bytes(), elems].concat()
    }

    #[test]
    fn every_mapped_preset_is_one_deckcraft_draws() {
        for t in 0..=255u16 {
            if let Some(name) = preset_name(t) {
                assert!(deckcraft_geom::preset::build(name, 100.0, 100.0, &[]).is_some(), "type {t}: {name}");
            }
        }
    }

    #[test]
    fn reads_a_freeform_triangle() {
        let verts: Vec<u8> = [(0i16, 100i16), (50, 0), (100, 100)].iter().flat_map(|(x, y)| [x.to_le_bytes(), y.to_le_bytes()].concat()).collect();
        // moveTo, lineTo ×2, close, end
        let segs: Vec<u8> = [0x4000u16, 0x0002, 0x6001, 0x8000].iter().flat_map(|v| v.to_le_bytes()).collect();
        let v = array(4, &verts);
        let s = array(2, &segs);
        let t = props(
            &[
                (prop::GEO_RIGHT, 100),
                (prop::GEO_BOTTOM, 100),
                (0x8000 | prop::VERTICES, v.len() as u32),
                (0x8000 | prop::SEGMENT_INFO, s.len() as u32),
            ],
            &[v, s].concat(),
        );
        let mut p = Props::default();
        p.add_table(&Rec::at(&t, 0).unwrap());
        let Geom::Custom { paths } = geometry(0, &p) else { panic!("not custom") };
        assert_eq!(paths[0].d, "M 0 100 L 50 0 L 100 100 Z");
        assert_eq!((paths[0].w, paths[0].h), (100.0, 100.0));
    }

    #[test]
    fn a_rounded_rectangle_keeps_its_corner() {
        let t = props(&[(prop::ADJUST_VALUE, 5400)], &[]);
        let mut p = Props::default();
        p.add_table(&Rec::at(&t, 0).unwrap());
        assert_eq!(geometry(2, &p), Geom::Preset { name: "roundRect".into(), adj: vec![25000.0] });
        assert_eq!(geometry(9999, &Props::default()), Geom::default());
    }
}
