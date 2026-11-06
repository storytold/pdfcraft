//! Shared bounded Type 3 cluster fonts for page content and appearance writers.
use crate::{GlyphOutline, ShapedCluster, ShapedScript};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};
use std::collections::HashMap;

const MAX_POINTS: usize = 1_000_000;
/// Bound coordinates before scaling to PDF glyph space, also bounding each emitted number.
fn coordinate(v: f64) -> bool {
    v.is_finite() && v.abs() <= 1_000_000.0
}

/// Validate outline geometry and count path commands within the shared point budget.
fn outline_costs(outlines: &HashMap<(ShapedScript, u32), GlyphOutline>) -> Result<HashMap<(ShapedScript, u32), usize>, String> {
    let mut total = 0usize;
    let mut costs = HashMap::new();
    for (key, outline) in outlines {
        if !outline.bbox.iter().copied().all(coordinate) || !coordinate(outline.width) {
            return Err("a shaped outline has invalid geometry".into());
        }
        let mut cost = 0usize;
        for contour in &outline.contours {
            // Count close-path commands too, including empty contours.
            cost = cost.saturating_add(contour.len()).saturating_add(1);
            if cost > MAX_POINTS || !contour.iter().flatten().copied().all(coordinate) {
                return Err("a shaped outline exceeds its point budget or has invalid coordinates".into());
            }
        }
        total = total.saturating_add(cost);
        if total > MAX_POINTS {
            return Err("the supplied outlines exceed their point budget".into());
        }
        costs.insert(*key, cost);
    }
    Ok(costs)
}

/// Reject invalid cluster geometry and cap emitted outline cost, including repeated glyphs.
fn validate_clusters(clusters: &[&ShapedCluster], outlines: &HashMap<(ShapedScript, u32), GlyphOutline>) -> Result<(), String> {
    if clusters.is_empty() || clusters.len() > 240 {
        return Err("a shaped font needs between one and 240 clusters".into());
    }
    let costs = outline_costs(outlines)?;
    let (mut text_bytes, mut emitted) = (0usize, 0usize);
    for cluster in clusters {
        text_bytes = text_bytes.saturating_add(cluster.text.len());
        if text_bytes > 262_144 || cluster.glyphs.len() > 4096 {
            return Err("a shaped font exceeds its text or glyph budget".into());
        }
        if !cluster.advance.is_finite() || !(0.0..=128.0).contains(&cluster.advance) {
            return Err("a shaped cluster has invalid geometry".into());
        }
        for (id, xy) in &cluster.glyphs {
            let Some(cost) = costs.get(&(cluster.script, *id)) else { return Err("a shaped cluster has a missing outline".into()) };
            if !xy.iter().copied().all(coordinate) {
                return Err("a shaped cluster has invalid geometry".into());
            }
            emitted = emitted.saturating_add(*cost);
            if emitted > MAX_POINTS {
                return Err("the shaped font exceeds its emitted outline budget".into());
            }
        }
    }
    Ok(())
}

/// Format a PDF number to three decimals, trimming zeros and normalizing negative zero.
fn n(v: f64) -> String {
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// A Type 3 font with `clusters` as codes 1…, each drawn from its script face's glyph `outlines`.
pub fn embed_shaped_clusters(
    doc: &mut Document,
    clusters: &[&ShapedCluster],
    outlines: &HashMap<(ShapedScript, u32), GlyphOutline>,
) -> Result<ObjRef, String> {
    // Validate multiplied output costs before adding any objects. With bounded numbers,
    // this also caps the generated path bytes even when a glyph is repeated many times.
    validate_clusters(clusters, outlines)?;

    let mut charprocs = Dict::new();
    let mut differences = vec![Object::Int(1)];
    let mut widths = Vec::with_capacity(clusters.len());
    let mut bbox = [0.0f64; 4];
    let mut to_unicode = Vec::new();
    for (i, c) in clusters.iter().enumerate() {
        let code = i + 1;
        // Glyph space is 1000 units per em.
        let mut paths = Vec::new();
        let mut ink: Option<[f64; 4]> = None;
        for (id, [dx, dy]) in &c.glyphs {
            let Some(g) = outlines.get(&(c.script, *id)) else { continue };
            for contour in &g.contours {
                for (j, p) in contour.iter().enumerate() {
                    paths.extend(format!("{} {} {}\n", n((p[0] + dx) * 1000.0), n((p[1] + dy) * 1000.0), if j == 0 { "m" } else { "l" }).bytes());
                }
                paths.extend_from_slice(b"h\n");
                let b = [g.bbox[0] + dx, g.bbox[1] + dy, g.bbox[2] + dx, g.bbox[3] + dy];
                ink = Some(ink.map_or(b, |a| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]));
            }
        }
        // Rounded outwards so no ink is clipped.
        let [x0, y0, x1, y1] =
            ink.map_or([0.0; 4], |b| [(b[0] * 1000.0).floor(), (b[1] * 1000.0).floor(), (b[2] * 1000.0).ceil(), (b[3] * 1000.0).ceil()]);
        bbox = [bbox[0].min(x0), bbox[1].min(y0), bbox[2].max(x1), bbox[3].max(y1)];
        let mut proc = format!("{} 0 {} {} {} {} d1\n", n(c.advance * 1000.0), n(x0), n(y0), n(x1), n(y1)).into_bytes();
        if !paths.is_empty() {
            proc.extend(paths);
            proc.extend_from_slice(b"f\n");
        }
        let glyph = format!("g{code:02X}");
        charprocs.set(glyph.clone().into_bytes(), Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), &proc)))));
        differences.push(Object::name(&glyph));
        widths.push(Object::Real((c.advance * 1000.0).round()));
        if !c.text.is_empty() {
            let hex: String = c.text.encode_utf16().map(|u| format!("{u:04X}")).collect();
            to_unicode.push(format!("<{code:02X}> <{hex}>\n"));
        }
    }
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
    );
    // A CMap block holds at most 100 entries.
    for block in to_unicode.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n{}endbfchar\n", block.len(), block.concat()));
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let cmap = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));
    // The face's family when every glyph comes from one script; without the face, the script's
    // name (as Arabic items were always named).
    let family = match clusters.first().map(|c| c.script).filter(|s| clusters.iter().all(|c| c.script == *s)) {
        Some(script) => script.font().map_or(
            match script {
                ShapedScript::Arabic => "Arabic",
                ShapedScript::Hebrew => "Hebrew",
                ShapedScript::Thai => "Thai",
                ShapedScript::Syriac => "Syriac",
                ShapedScript::SyriacEastern => "SyriacEastern",
                ShapedScript::SyriacWestern => "SyriacWestern",
            },
            |f| f.family,
        ),
        None => "PdfCraft Shaped Text",
    };
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(&family.replace(' ', "")));
    descriptor.set(b"FontFamily".to_vec(), Object::String(PdfString::literal(family.as_bytes().to_vec())));
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
    let mut encoding = Dict::new();
    encoding.set(b"Type".to_vec(), Object::name("Encoding"));
    encoding.set(b"Differences".to_vec(), Object::Array(differences));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type3"));
    font.set(b"FontDescriptor".to_vec(), Object::Ref(doc.add(Object::Dict(descriptor))));
    font.set(b"FontBBox".to_vec(), Object::Array(bbox.iter().map(|v| Object::Real(*v)).collect()));
    font.set(
        b"FontMatrix".to_vec(),
        Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
    );
    font.set(b"FirstChar".to_vec(), Object::Int(1));
    font.set(b"LastChar".to_vec(), Object::Int(clusters.len() as i64));
    font.set(b"Widths".to_vec(), Object::Array(widths));
    font.set(b"Encoding".to_vec(), Object::Dict(encoding));
    font.set(b"CharProcs".to_vec(), Object::Dict(charprocs));
    font.set(b"ToUnicode".to_vec(), Object::Ref(cmap));
    Ok(doc.add(Object::Dict(font)))
}
