//! DrawingML paths share the pictures' stacking model on pages containing both kinds of art.

use super::{ArtBudget, Shape, emu, layout_number, normalized, point_bounds, rgb_attr, round_outline};

pub(super) fn shape_xml(shape: &Shape, height: f64, id: &mut usize, budget: &mut ArtBudget) -> Option<String> {
    shape_xml_with_fill(shape, height, id, budget, shape.fill)
}

fn shape_xml_with_fill(shape: &Shape, height: f64, id: &mut usize, budget: &mut ArtBudget, fill: Option<[f64; 3]>) -> Option<String> {
    if shape.stroke.is_none() && fill.is_none() {
        return None;
    }
    if fill.is_some() && (shape.paths.len() > 1 || shape.even_odd) {
        let mut xml = super::compound::xml(shape, height, id, 0, budget, true);
        if shape.stroke.is_some() {
            xml.push_str(&shape_xml_with_fill(shape, height, id, budget, None).unwrap_or_default());
        }
        return (!xml.is_empty()).then_some(xml);
    }
    // Filling closes a subpath; stroking does not. Separate those two paints for open B/B* paths.
    if fill.is_some() && shape.stroke.is_some() && shape.paths.iter().any(|(_, closed)| !closed) {
        let mut output = String::new();
        for (points, closed) in &shape.paths {
            let mut path = Shape { even_odd: false, paths: vec![(points.clone(), *closed)], stroke: shape.stroke, fill };
            if !closed {
                path.stroke = None;
                output.push_str(&shape_xml(&path, height, id, budget).unwrap_or_default());
                path.fill = None;
                path.stroke = shape.stroke;
            }
            output.push_str(&shape_xml(&path, height, id, budget).unwrap_or_default());
        }
        return (!output.is_empty()).then_some(output);
    }
    let mut output = String::new();
    for (points, closed) in &shape.paths {
        if points.len() < 2 || !points.iter().flatten().all(|n| n.is_finite()) {
            continue;
        }
        let (points, closed) = normalized(points, *closed, fill.is_some());
        if !budget.take(points.len()) {
            continue;
        }
        let Some(bounds) = point_bounds(points.iter()) else { continue };
        if shape.stroke.is_some()
            && let Some(outline) = round_outline(&points, closed)
        {
            budget.outlines.push(outline);
        }
        *id = id.saturating_add(1);
        let doc_id = (2usize << 20).saturating_add(*id);
        let x = emu(layout_number(bounds[0]));
        let y = emu(layout_number(height - bounds[3]));
        let cx = emu(layout_number(bounds[2] - bounds[0]).max(0.01));
        let cy = emu(layout_number(bounds[3] - bounds[1]).max(0.01));
        let mut path = String::new();
        for (index, point) in points.iter().enumerate() {
            let op = if index == 0 { "moveTo" } else { "lnTo" };
            path.push_str(&format!(
                "<a:{op}><a:pt x=\"{}\" y=\"{}\"/></a:{op}>",
                emu(layout_number(point[0] - bounds[0])),
                emu(layout_number(bounds[3] - point[1]))
            ));
        }
        if closed {
            path.push_str("<a:close/>");
        }
        let fill = fill.map_or_else(
            || "<a:noFill/>".into(),
            |c| format!("<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>", rgb_attr(c).trim_start_matches('#')),
        );
        let stroke = shape.stroke.map_or_else(
            || "<a:ln><a:noFill/></a:ln>".into(),
            |stroke| {
                let width = if stroke.width.is_finite() { stroke.width.clamp(0.25, 100.0) } else { 0.25 };
                let dash = if stroke.dashed { "<a:prstDash val=\"dash\"/>" } else { "" };
                format!(
                    "<a:ln w=\"{}\"><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>{dash}</a:ln>",
                    emu(width),
                    rgb_attr(stroke.color).trim_start_matches('#')
                )
            },
        );
        output.push_str(&format!(
            "<wps:wsp><wps:cNvPr id=\"{doc_id}\" name=\"Path {id}\"/><wps:cNvSpPr/><wps:spPr>\
<a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l=\"0\" t=\"0\" r=\"r\" b=\"b\"/><a:pathLst><a:path w=\"{cx}\" h=\"{cy}\">{path}</a:path></a:pathLst></a:custGeom>{fill}{stroke}</wps:spPr><wps:bodyPr/></wps:wsp>"
        ));
    }
    (!output.is_empty()).then_some(output)
}

/// A shared group keeps picture/path order in readers that otherwise put pictures above
/// shapes even when their individual anchors declare the opposite relativeHeight.
pub(super) fn group_xml(children: &str, page: &super::Page, id: usize) -> String {
    let doc_id = (3usize << 20).saturating_add(id);
    let cx = emu(layout_number(page.width).max(1.0));
    let cy = emu(layout_number(page.height).max(1.0));
    // LibreOffice fits a group's child bounds to its extent even with chOff/chExt set.
    // A nonpainting page-sized child keeps partial-page artwork at its original scale.
    let extent_id = (4usize << 20).saturating_add(id);
    let extent = format!(
        "<wps:wsp><wps:cNvPr id=\"{extent_id}\" name=\"Page extent\"/><wps:cNvSpPr/><wps:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:noFill/><a:ln><a:noFill/></a:ln></wps:spPr><wps:bodyPr/></wps:wsp>"
    );
    format!(
        "<w:r><w:drawing><wp:anchor distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\" simplePos=\"0\" relativeHeight=\"1\" behindDoc=\"1\" locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\">\
<wp:simplePos x=\"0\" y=\"0\"/><wp:positionH relativeFrom=\"page\"><wp:posOffset>0</wp:posOffset></wp:positionH><wp:positionV relativeFrom=\"page\"><wp:posOffset>0</wp:posOffset></wp:positionV>\
<wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/><wp:wrapNone/><wp:docPr id=\"{doc_id}\" name=\"Page artwork {id}\"/><wp:cNvGraphicFramePr/>\
<a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><a:graphicData uri=\"http://schemas.microsoft.com/office/word/2010/wordprocessingGroup\">\
<wpg:wgp xmlns:wpg=\"http://schemas.microsoft.com/office/word/2010/wordprocessingGroup\"><wpg:cNvGrpSpPr/><wpg:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm></wpg:grpSpPr>{extent}{children}</wpg:wgp></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r>"
    )
}
