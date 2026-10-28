//! PowerPoint presentation (.pptx, Office Open XML PresentationML): slides per page,
//! largest text as slide title, remaining paragraphs as body text, images and tables between them.

use crate::{Block, Image, Page, Table, Zip, esc, hex};

/// Items on a slide in reading order.
enum SlideBodyItem<'a> {
    Para(&'a Block),
    Table(Table),
    Img(&'a Image),
}

/// Convert points to EMU (English Metric Units: 1 pt = 12700 EMU).
///
/// Clamps to PowerPoint's allowed slide dimensions: 1 inch (914,400 EMU = 72 pt)
/// to 56 inches (51,206,400 EMU = 4032 pt).
fn emu(pt: f64, default_pt: f64) -> i64 {
    if pt.is_finite() && pt > 0.0 { (pt * 12700.0).round().clamp(914400.0, 51206400.0) as i64 } else { (default_pt * 12700.0).round() as i64 }
}

/// The largest height one item may take on a slide (about 1,100 inches). Text sizes and image
/// rectangles come from the PDF, so a hostile size (say 1e300 pt) must not push the running
/// vertical position past what an `i64` (or a DrawingML coordinate) holds.
const MAX_ITEM_EMU: i64 = 1_000_000_000;

/// Points to EMU for a height or gap, clamped to `0..=MAX_ITEM_EMU` (NaN reads as 0).
fn height_emu(pt: f64) -> i64 {
    let v = pt * 12700.0;
    // Clamped to 0..=MAX_ITEM_EMU before the cast, so it cannot truncate or wrap.
    if v.is_finite() {
        v.round().clamp(0.0, MAX_ITEM_EMU as f64) as i64
    } else if v > 0.0 {
        MAX_ITEM_EMU
    } else {
        0
    }
}

/// A formatted DrawingML text run (`<a:r>`).
fn run_drawingml(text: &str, size: f64, bold: bool, italic: bool, color: [f64; 3]) -> String {
    let mut rpr = String::new();
    let sz = if size.is_finite() { (size * 100.0).round().clamp(100.0, 400000.0) as i64 } else { 1200 };
    rpr.push_str(&format!(" sz=\"{sz}\""));
    if bold {
        rpr.push_str(" b=\"1\"");
    }
    if italic {
        rpr.push_str(" i=\"1\"");
    }
    let fill = if let Some(h) = hex(color) { format!("<a:solidFill><a:srgbClr val=\"{h}\"/></a:solidFill>") } else { String::new() };
    format!("<a:r><a:rPr{rpr}>{fill}</a:rPr><a:t>{}</a:t></a:r>", esc(text))
}

/// Generates a DrawingML table inside a `<p:graphicFrame>` with explicit cell borders.
fn pptx_table(shape_id: usize, t: &Table, x: i64, y: i64, content_w: i64) -> (String, i64) {
    let mut edges = t.cols.clone();
    edges.push(t.rect[2]);
    let widths_pt: Vec<f64> = edges.windows(2).map(|w| (w[1] - w[0]).max(20.0)).collect();
    let total_pt: f64 = widths_pt.iter().sum();
    let scale = if total_pt > 0.0 { (content_w as f64 / (total_pt * 12700.0)).min(1.0) } else { 1.0 };
    let widths: Vec<i64> = widths_pt.iter().map(|w| (w * scale * 12700.0).round().max(127000.0) as i64).collect();
    let table_w: i64 = widths.iter().fold(0i64, |sum, w| sum.saturating_add(*w));
    let row_h: i64 = height_emu(24.0);
    let table_h: i64 = i64::try_from(t.rows.len()).unwrap_or(i64::MAX).saturating_mul(row_h).min(MAX_ITEM_EMU);

    let mut grid_cols = String::new();
    for w in &widths {
        grid_cols.push_str(&format!("<a:gridCol w=\"{w}\"/>"));
    }

    const TC_PR: &str = "<a:tcPr>\
<a:lnL w=\"12700\" cmpd=\"s\"><a:solidFill><a:srgbClr val=\"999999\"/></a:solidFill></a:lnL>\
<a:lnR w=\"12700\" cmpd=\"s\"><a:solidFill><a:srgbClr val=\"999999\"/></a:solidFill></a:lnR>\
<a:lnT w=\"12700\" cmpd=\"s\"><a:solidFill><a:srgbClr val=\"999999\"/></a:solidFill></a:lnT>\
<a:lnB w=\"12700\" cmpd=\"s\"><a:solidFill><a:srgbClr val=\"999999\"/></a:solidFill></a:lnB>\
</a:tcPr>";

    let mut rows_xml = String::new();
    for row in &t.rows {
        rows_xml.push_str(&format!("<a:tr h=\"{row_h}\">"));
        for c in row {
            let span = c.span.max(1);
            let run = run_drawingml(&c.text, c.size, c.bold, c.italic, c.color);
            if span > 1 {
                rows_xml.push_str(&format!("<a:tc gridSpan=\"{span}\"><a:txBody><a:bodyPr/><a:lstStyle/><a:p>{run}</a:p></a:txBody>{TC_PR}</a:tc>"));
                for _ in 1..span {
                    rows_xml.push_str(&format!("<a:tc hMerge=\"1\"><a:txBody><a:bodyPr/><a:lstStyle/><a:p/></a:txBody>{TC_PR}</a:tc>"));
                }
            } else {
                rows_xml.push_str(&format!("<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p>{run}</a:p></a:txBody>{TC_PR}</a:tc>"));
            }
        }
        let used: usize = row.iter().map(|c| c.span).sum();
        for _ in used..t.cols.len() {
            rows_xml.push_str(&format!("<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p/></a:txBody>{TC_PR}</a:tc>"));
        }
        rows_xml.push_str("</a:tr>");
    }

    let frame = format!(
        "<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"{shape_id}\" name=\"Table {shape_id}\"/>\
<p:cNvGraphicFramePr><a:graphicFrameLocks noGrp=\"1\"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr>\
<p:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{table_w}\" cy=\"{table_h}\"/></p:xfrm>\
<a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/table\">\
<a:tbl><a:tblPr/><a:tblGrid>{grid_cols}</a:tblGrid>{rows_xml}</a:tbl>\
</a:graphicData></a:graphic></p:graphicFrame>"
    );
    (frame, table_h)
}

/// A PowerPoint presentation (.pptx, Office Open XML PresentationML): each page becomes a slide,
/// with the largest text as the title, remaining paragraphs as body text, and images between them.
///
/// When `pages` is empty, produces a presentation with a single blank slide so the resulting
/// `.pptx` file is a valid OpenXML package that PowerPoint can open without repair warnings.
pub fn pptx(pages: &[Page], title: &str) -> Vec<u8> {
    let dummy = [Page::default()];
    let pages = if pages.is_empty() { &dummy[..] } else { pages };

    let (sw, sh) = pages.first().map_or((9144000, 6858000), |p| (emu(p.width, 720.0), emu(p.height, 540.0)));
    let mx = (sw / 16).clamp(360000, 914400);
    let my = (sh / 16).clamp(360000, 914400);
    let content_w = sw - 2 * mx;

    let mut global_media: Vec<(String, &Image)> = Vec::new();
    let mut slides: Vec<(String, String)> = Vec::new(); // (slide_xml, slide_rels)

    for p in pages {
        let (tables, consumed) = crate::tables(&p.blocks);
        let title_idx = p
            .blocks
            .iter()
            .enumerate()
            .filter(|(i, b)| !consumed.get(*i).copied().unwrap_or(false) && !b.text.trim().is_empty())
            .max_by(|(_, a), (_, b)| a.size.total_cmp(&b.size))
            .map(|(i, _)| i);

        let mut items: Vec<(f64, f64, SlideBodyItem)> = Vec::new();
        for (bi, b) in p.blocks.iter().enumerate() {
            if Some(bi) == title_idx || consumed.get(bi).copied().unwrap_or(false) {
                continue;
            }
            items.push((b.rect[3], b.rect[0], SlideBodyItem::Para(b)));
        }
        for t in tables {
            items.push((t.rect[3], t.rect[0], SlideBodyItem::Table(t)));
        }
        for im in &p.images {
            items.push((im.rect[3], im.rect[0], SlideBodyItem::Img(im)));
        }
        items.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.total_cmp(&b.1)));

        let mut current_y = my;
        let mut shapes = String::new();
        let mut shape_id = 2usize;
        let mut slide_media_rels: Vec<(String, usize)> = Vec::new();

        if let Some(title_block) = title_idx.and_then(|ti| p.blocks.get(ti)) {
            let title_h = height_emu((title_block.size * 1.5).max(36.0));
            let title_run = run_drawingml(&title_block.text, title_block.size, title_block.bold, title_block.italic, title_block.color);
            shapes.push_str(&format!(
                "<p:sp><p:nvSpPr><p:cNvPr id=\"{shape_id}\" name=\"Title {shape_id}\"/><p:cNvSpPr txBox=\"1\"/><p:nvPr/></p:nvSpPr>\
<p:spPr><a:xfrm><a:off x=\"{mx}\" y=\"{current_y}\"/><a:ext cx=\"{content_w}\" cy=\"{title_h}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:noFill/></p:spPr>\
<p:txBody><a:bodyPr wrap=\"square\" rtlCol=\"0\"><a:spAutoFit/></a:bodyPr><a:lstStyle/><a:p>{title_run}</a:p></p:txBody></p:sp>"
            ));
            shape_id += 1;
            current_y = current_y.saturating_add(title_h).saturating_add(height_emu(18.0));
        }

        let mut i = 0;
        while let Some((_, _, item)) = items.get(i) {
            match item {
                SlideBodyItem::Para(_) => {
                    let mut paras_xml = String::new();
                    let mut est_h_pt = 0.0;
                    while let Some((_, _, SlideBodyItem::Para(b))) = items.get(i) {
                        let run = run_drawingml(&b.text, b.size, b.bold, b.italic, b.color);
                        paras_xml.push_str(&format!("<a:p>{run}</a:p>"));
                        let lines = (b.text.len() as f64 / 60.0).ceil().max(1.0);
                        est_h_pt += (b.size.max(10.0) * 1.3) * lines + 6.0;
                        i += 1;
                    }
                    let box_h = height_emu(est_h_pt);
                    shapes.push_str(&format!(
                        "<p:sp><p:nvSpPr><p:cNvPr id=\"{shape_id}\" name=\"TextBox {shape_id}\"/><p:cNvSpPr txBox=\"1\"/><p:nvPr/></p:nvSpPr>\
<p:spPr><a:xfrm><a:off x=\"{mx}\" y=\"{current_y}\"/><a:ext cx=\"{content_w}\" cy=\"{box_h}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:noFill/></p:spPr>\
<p:txBody><a:bodyPr wrap=\"square\" rtlCol=\"0\"><a:spAutoFit/></a:bodyPr><a:lstStyle/>{paras_xml}</p:txBody></p:sp>"
                    ));
                    shape_id += 1;
                    current_y = current_y.saturating_add(box_h).saturating_add(height_emu(12.0));
                }
                SlideBodyItem::Img(im) => {
                    let global_img_idx = global_media.len() + 1;
                    let img_name = format!("image{global_img_idx}.{}", im.ext);
                    global_media.push((img_name, *im));
                    let r_id = format!("rIdImg{}", slide_media_rels.len() + 1);
                    slide_media_rels.push((r_id.clone(), global_img_idx));

                    let (w, h) = ((im.rect[2] - im.rect[0]).max(1.0), (im.rect[3] - im.rect[1]).max(1.0));
                    let max_w = content_w as f64 / 12700.0;
                    let k = (max_w / w).min(1.0);
                    let cx = height_emu(w * k).min(content_w);
                    let cy = height_emu(h * k);
                    let x = mx + (content_w - cx) / 2;
                    let y = current_y;

                    shapes.push_str(&format!(
                        "<p:pic><p:nvPicPr><p:cNvPr id=\"{shape_id}\" name=\"Picture {shape_id}\"/><p:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr><p:nvPr/></p:nvPicPr>\
<p:blipFill><a:blip r:embed=\"{r_id}\"/><a:stretch><a:fillRect/></a:stretch></p:blipFill>\
<p:spPr><a:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:pic>"
                    ));
                    shape_id += 1;
                    current_y = current_y.saturating_add(cy).saturating_add(height_emu(12.0));
                    i += 1;
                }
                SlideBodyItem::Table(t) => {
                    let (table_xml, table_h) = pptx_table(shape_id, t, mx, current_y, content_w);
                    shapes.push_str(&table_xml);
                    shape_id += 1;
                    current_y = current_y.saturating_add(table_h).saturating_add(height_emu(12.0));
                    i += 1;
                }
            }
        }

        let slide_xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<p:sld xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><p:cSld><p:spTree>\
<p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
<p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr>\
{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"
        );

        let mut slide_rels = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rIdLayout\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout\" Target=\"../slideLayouts/slideLayout1.xml\"/>",
        );
        for (r_id, global_idx) in &slide_media_rels {
            if let Some((name, _)) = global_media.get(*global_idx - 1) {
                slide_rels.push_str(&format!(
                    "<Relationship Id=\"{r_id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"../media/{name}\"/>"
                ));
            }
        }
        slide_rels.push_str("</Relationships>");

        slides.push((slide_xml, slide_rels));
    }

    let mut types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Default Extension=\"png\" ContentType=\"image/png\"/><Default Extension=\"jpg\" ContentType=\"image/jpeg\"/><Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>\
<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
<Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml\"/>\
<Override PartName=\"/ppt/slideMasters/slideMaster1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml\"/>\
<Override PartName=\"/ppt/slideLayouts/slideLayout1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml\"/>\
<Override PartName=\"/ppt/theme/theme1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.theme+xml\"/>",
    );
    for i in 1..=slides.len() {
        types.push_str(&format!(
            "<Override PartName=\"/ppt/slides/slide{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>"
        ));
    }
    types.push_str("</Types>");

    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"ppt/presentation.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/></Relationships>";

    let core = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>{}</dc:title></cp:coreProperties>",
        esc(title)
    );

    let mut sld_id_lst = String::new();
    let mut pres_rels = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rIdMaster1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster\" Target=\"slideMasters/slideMaster1.xml\"/>",
    );
    for i in 1..=slides.len() {
        let sld_id = 255 + i;
        sld_id_lst.push_str(&format!("<p:sldId id=\"{sld_id}\" r:id=\"rIdSld{i}\"/>"));
        pres_rels.push_str(&format!(
            "<Relationship Id=\"rIdSld{i}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{i}.xml\"/>"
        ));
    }
    pres_rels.push_str("</Relationships>");

    let pres = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<p:presentation xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rIdMaster1\"/></p:sldMasterIdLst>\
<p:sldIdLst>{sld_id_lst}</p:sldIdLst><p:sldSz cx=\"{sw}\" cy=\"{sh}\"/><p:notesSz cx=\"6858000\" cy=\"9144000\"/></p:presentation>"
    );

    let slide_master = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<p:sldMaster xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
<p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr></p:spTree></p:cSld>\
<p:clrMap bg1=\"lt1\" tx1=\"dk1\" bg2=\"lt2\" tx2=\"dk2\" accent1=\"accent1\" accent2=\"accent2\" accent3=\"accent3\" accent4=\"accent4\" accent5=\"accent5\" accent6=\"accent6\" hlink=\"hlink\" folHlink=\"folHlink\"/>\
<p:sldLayoutIdLst><p:sldLayoutId id=\"2147483649\" r:id=\"rIdLayout1\"/></p:sldLayoutIdLst></p:sldMaster>";

    let slide_master_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rIdLayout1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout\" Target=\"../slideLayouts/slideLayout1.xml\"/>\
<Relationship Id=\"rIdTheme1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme\" Target=\"../theme/theme1.xml\"/></Relationships>";

    let slide_layout = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<p:sldLayout xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" type=\"blank\"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
<p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr></p:spTree></p:cSld></p:sldLayout>";

    let slide_layout_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster\" Target=\"../slideMasters/slideMaster1.xml\"/></Relationships>";

    let theme = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"Office Theme\"><a:themeElements>\
<a:clrScheme name=\"Office\"><a:dk1><a:sysClr val=\"windowText\" lastClr=\"000000\"/></a:dk1><a:lt1><a:sysClr val=\"window\" lastClr=\"FFFFFF\"/></a:lt1>\
<a:dk2><a:srgbClr val=\"1F497D\"/></a:dk2><a:lt2><a:srgbClr val=\"EEECE1\"/></a:lt2><a:accent1><a:srgbClr val=\"4F81BD\"/></a:accent1>\
<a:accent2><a:srgbClr val=\"C0504D\"/></a:accent2><a:accent3><a:srgbClr val=\"9BBB59\"/></a:accent3><a:accent4><a:srgbClr val=\"8064A2\"/></a:accent4>\
<a:accent5><a:srgbClr val=\"4BACC6\"/></a:accent5><a:accent6><a:srgbClr val=\"F79646\"/></a:accent6><a:hlink><a:srgbClr val=\"0000FF\"/></a:hlink>\
<a:folHlink><a:srgbClr val=\"800080\"/></a:folHlink></a:clrScheme>\
<a:fontScheme name=\"Office\"><a:majorFont><a:latin typeface=\"Calibri\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:majorFont><a:minorFont><a:latin typeface=\"Calibri\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:minorFont></a:fontScheme>\
<a:fmtScheme name=\"Office\"><a:fillStyleLst><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:fillStyleLst>\
<a:lnStyleLst><a:ln w=\"9525\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln><a:ln w=\"25400\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln><a:ln w=\"38100\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln></a:lnStyleLst>\
<a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>\
<a:bgFillStyleLst><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme>\
</a:themeElements></a:theme>";

    let mut z = Zip::default();
    z.add("[Content_Types].xml", types.as_bytes(), true);
    z.add("_rels/.rels", rels.as_bytes(), true);
    z.add("docProps/core.xml", core.as_bytes(), true);
    z.add("ppt/presentation.xml", pres.as_bytes(), true);
    z.add("ppt/_rels/presentation.xml.rels", pres_rels.as_bytes(), true);
    z.add("ppt/slideMasters/slideMaster1.xml", slide_master.as_bytes(), true);
    z.add("ppt/slideMasters/_rels/slideMaster1.xml.rels", slide_master_rels.as_bytes(), true);
    z.add("ppt/slideLayouts/slideLayout1.xml", slide_layout.as_bytes(), true);
    z.add("ppt/slideLayouts/_rels/slideLayout1.xml.rels", slide_layout_rels.as_bytes(), true);
    z.add("ppt/theme/theme1.xml", theme.as_bytes(), true);
    for (i, (s_xml, s_rels)) in slides.iter().enumerate() {
        let num = i + 1;
        z.add(&format!("ppt/slides/slide{num}.xml"), s_xml.as_bytes(), true);
        z.add(&format!("ppt/slides/_rels/slide{num}.xml.rels"), s_rels.as_bytes(), true);
    }
    for (name, im) in &global_media {
        z.add(&format!("ppt/media/{name}"), &im.bytes, false);
    }
    z.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::part;

    fn sample_page() -> Page {
        Page {
            width: 720.0,
            height: 540.0,
            blocks: vec![
                Block {
                    text: "Q3 Business Update".into(),
                    rect: [72.0, 460.0, 500.0, 500.0],
                    size: 28.0,
                    bold: true,
                    italic: false,
                    color: [0.1, 0.2, 0.3],
                    rotation: 0.0,
                },
                Block {
                    text: "Revenue grew 25% year over year across all regions.".into(),
                    rect: [72.0, 380.0, 500.0, 410.0],
                    size: 14.0,
                    bold: false,
                    italic: false,
                    color: [0.0; 3],
                    rotation: 0.0,
                },
                Block {
                    text: "Operational expenses remained flat.".into(),
                    rect: [72.0, 340.0, 500.0, 370.0],
                    size: 14.0,
                    bold: false,
                    italic: true,
                    color: [0.0; 3],
                    rotation: 0.0,
                },
            ],
            images: vec![Image {
                ext: "png",
                bytes: b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x02\0\0\0\x03".to_vec(),
                rect: [72.0, 150.0, 300.0, 300.0],
                size: [228.0, 150.0],
                rotation: 0.0,
            }],
            background: None,
        }
    }

    #[test]
    pub fn pptx_is_a_presentationml_package() {
        let p = sample_page();
        let bytes = pptx(&[p], "Executive Deck");
        assert!(bytes.starts_with(b"PK"));
        for part_name in [
            "[Content_Types].xml",
            "_rels/.rels",
            "docProps/core.xml",
            "ppt/presentation.xml",
            "ppt/_rels/presentation.xml.rels",
            "ppt/slideMasters/slideMaster1.xml",
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
            "ppt/slideLayouts/slideLayout1.xml",
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
            "ppt/theme/theme1.xml",
            "ppt/slides/slide1.xml",
            "ppt/slides/_rels/slide1.xml.rels",
            "ppt/media/image1.png",
        ] {
            assert!(bytes.windows(part_name.len()).any(|w| w == part_name.as_bytes()), "missing {part_name}");
        }
    }

    #[test]
    pub fn pptx_slide_has_title_body_and_images_in_order() {
        let p = sample_page();
        let bytes = pptx(&[p], "Deck");
        let slide_xml = part(&bytes, "ppt/slides/slide1.xml");
        assert!(slide_xml.contains("name=\"Title 2\""));
        assert!(slide_xml.contains("txBox=\"1\""));
        assert!(slide_xml.contains("Q3 Business Update"));
        assert!(slide_xml.contains("name=\"TextBox 3\""));
        assert!(slide_xml.contains("Revenue grew 25%"));
        assert!(slide_xml.contains("<p:pic>"));
        assert!(slide_xml.contains("<a:blip r:embed=\"rIdImg1\"/>"));
        assert!(!slide_xml.contains("<p:ph"), "pure text boxes avoid placeholder inheritance repair warnings: {slide_xml}");

        let rels_xml = part(&bytes, "ppt/slides/_rels/slide1.xml.rels");
        assert!(rels_xml.contains("Target=\"../media/image1.png\""));
        assert!(rels_xml.contains("Target=\"../slideLayouts/slideLayout1.xml\""));
    }

    #[test]
    pub fn pptx_text_colour_and_formatting() {
        let p = sample_page();
        let bytes = pptx(&[p], "Deck");
        let slide_xml = part(&bytes, "ppt/slides/slide1.xml");
        assert!(slide_xml.contains("sz=\"2800\""));
        assert!(slide_xml.contains("b=\"1\""));
        assert!(slide_xml.contains("<a:srgbClr val=\"1A334D\"/>"));
        assert!(slide_xml.contains("i=\"1\""));
    }

    /// Every entry of the package in order: (name, uncompressed bytes), read from the local
    /// headers the way an unzipper walks them, so a broken structure fails here.
    fn entries(zip: &[u8]) -> Vec<(String, Vec<u8>)> {
        let at = |i: usize, n: usize| zip[i..i + n].iter().rev().fold(0usize, |v, b| v << 8 | *b as usize);
        let mut out = Vec::new();
        let mut i = 0;
        while zip[i..].starts_with(b"PK\x03\x04") {
            let (method, size, raw, name_len, extra) = (at(i + 8, 2), at(i + 18, 4), at(i + 22, 4), at(i + 26, 2), at(i + 28, 2));
            let data = i + 30 + name_len + extra;
            let name = String::from_utf8(zip[i + 30..i + 30 + name_len].to_vec()).expect("UTF-8 name");
            let body = &zip[data..data + size];
            let bytes = match method {
                0 => body.to_vec(),
                8 => {
                    let mut v = Vec::new();
                    std::io::Read::read_to_end(&mut flate2::read::DeflateDecoder::new(body), &mut v).expect("inflates");
                    v
                }
                m => panic!("{name}: unexpected method {m}"),
            };
            assert_eq!(bytes.len(), raw, "{name}: uncompressed size");
            out.push((name, bytes));
            i = data + size;
        }
        assert!(zip[i..].starts_with(b"PK\x01\x02"), "the central directory follows the entries");
        out
    }

    /// Checks that every start tag is closed by the matching end tag (text is escaped, so a
    /// `<` always starts markup). Catches a typo such as `<a:lnR>…</a:lnL>`.
    fn assert_tags_balance(name: &str, xml: &str) {
        let mut stack: Vec<&str> = Vec::new();
        for chunk in xml.split('<').skip(1) {
            let tag = chunk.split('>').next().unwrap_or_default();
            if tag.starts_with('?') {
                continue;
            }
            if let Some(close) = tag.strip_prefix('/') {
                assert_eq!(stack.pop(), Some(close.trim()), "{name}: </{close}> closes the wrong element");
            } else if !tag.ends_with('/') {
                stack.push(tag.split_whitespace().next().unwrap_or_default());
            }
        }
        assert!(stack.is_empty(), "{name}: unclosed {stack:?}");
    }

    #[test]
    fn pptx_package_parts_are_complete_and_consistent() {
        let cell = |t: &str, x: f64, y: f64| Block {
            text: t.into(),
            rect: [x, y, x + 100.0, y + 14.0],
            size: 12.0,
            bold: false,
            italic: false,
            color: [0.0; 3],
            rotation: 0.0,
        };
        let table_page = Page {
            width: 612.0,
            height: 792.0,
            blocks: vec![
                cell("Metric", 72.0, 400.0),
                cell("Value", 200.0, 400.0),
                cell("Growth", 72.0, 360.0),
                cell("+9%", 200.0, 360.0),
                cell("Target", 72.0, 320.0),
                cell("100%", 200.0, 320.0),
            ],
            images: Vec::new(),
            background: None,
        };
        let bytes = pptx(&[sample_page(), table_page], "Two");
        let parts = entries(&bytes);
        assert_eq!(parts.first().map(|p| p.0.as_str()), Some("[Content_Types].xml"), "content types come first");
        let names: std::collections::HashSet<&str> = parts.iter().map(|p| p.0.as_str()).collect();
        assert_eq!(names.len(), parts.len(), "no duplicate parts");
        for required in [
            "[Content_Types].xml",
            "_rels/.rels",
            "ppt/presentation.xml",
            "ppt/_rels/presentation.xml.rels",
            "ppt/slideMasters/slideMaster1.xml",
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
            "ppt/slideLayouts/slideLayout1.xml",
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
            "ppt/theme/theme1.xml",
            "ppt/slides/slide1.xml",
            "ppt/slides/_rels/slide1.xml.rels",
            "ppt/slides/slide2.xml",
            "ppt/slides/_rels/slide2.xml.rels",
            "ppt/media/image1.png",
        ] {
            assert!(names.contains(required), "missing {required}");
        }
        let text = |n: &str| String::from_utf8(parts.iter().find(|p| p.0 == n).map(|p| p.1.clone()).unwrap_or_default()).expect("UTF-8 XML");
        for (name, body) in &parts {
            if name.ends_with(".xml") || name.ends_with(".rels") {
                assert!(body.starts_with(b"<?xml"), "{name} is XML");
                assert_tags_balance(name, &text(name));
            }
        }
        assert!(text("ppt/slides/slide2.xml").contains("<a:tbl>"), "the second slide carries the table");
        // Text boxes, not placeholders: the blank layout defines none to inherit from.
        for slide in ["ppt/slides/slide1.xml", "ppt/slides/slide2.xml"] {
            assert!(!text(slide).contains("<p:ph"), "{slide} uses no placeholders");
        }
        // Every override names a part that exists, and every XML part has a content type.
        let types = text("[Content_Types].xml");
        for chunk in types.split("PartName=\"/").skip(1) {
            let part = chunk.split('"').next().unwrap_or_default();
            assert!(names.contains(part), "override for missing part {part}");
        }
        for slide in ["ppt/slides/slide1.xml", "ppt/slides/slide2.xml", "ppt/presentation.xml", "ppt/theme/theme1.xml"] {
            assert!(types.contains(&format!("PartName=\"/{slide}\"")), "{slide} has a content type");
        }
        // Relationship targets resolve to parts in the package.
        for (rels, base) in [
            ("_rels/.rels", ""),
            ("ppt/_rels/presentation.xml.rels", "ppt/"),
            ("ppt/slides/_rels/slide1.xml.rels", "ppt/slides/"),
            ("ppt/slides/_rels/slide2.xml.rels", "ppt/slides/"),
            ("ppt/slideMasters/_rels/slideMaster1.xml.rels", "ppt/slideMasters/"),
            ("ppt/slideLayouts/_rels/slideLayout1.xml.rels", "ppt/slideLayouts/"),
        ] {
            for chunk in text(rels).split("Target=\"").skip(1) {
                let target = chunk.split('"').next().unwrap_or_default();
                let mut path: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
                for seg in target.split('/') {
                    if seg == ".." {
                        path.pop();
                    } else {
                        path.push(seg);
                    }
                }
                let resolved = path.join("/");
                assert!(names.contains(resolved.as_str()), "{rels}: target {target} is missing");
            }
        }
        // The theme's font scheme carries the schema-required latin, ea and cs faces.
        let theme = text("ppt/theme/theme1.xml");
        for font in ["majorFont", "minorFont"] {
            let inner = theme.split(&format!("<a:{font}>")).nth(1).and_then(|r| r.split(&format!("</a:{font}>")).next()).unwrap_or_default();
            assert!(inner.contains("<a:latin ") && inner.contains("<a:ea ") && inner.contains("<a:cs "), "{font}: {inner}");
        }
        assert!(text("ppt/presentation.xml").contains("r:id=\"rIdSld2\""));
    }

    #[test]
    fn hostile_sizes_do_not_overflow_the_slide_position() {
        let huge = |text: &str, size: f64, y: f64| Block {
            text: text.into(),
            rect: [72.0, y, 500.0, y + 10.0],
            size,
            bold: false,
            italic: false,
            color: [0.0; 3],
            rotation: 0.0,
        };
        let mut blocks = vec![huge("Title", f64::MAX, 500.0)];
        for k in 0..40 {
            blocks.push(huge(&"word ".repeat(10_000), 1e300, 400.0 - k as f64));
            blocks.push(huge("nan", f64::NAN, 300.0 - k as f64));
            blocks.push(huge("inf", f64::INFINITY, 200.0 - k as f64));
        }
        let images = (0..40)
            .map(|k| Image { ext: "png", bytes: vec![0; 4], rect: [0.0, -1e300, 1e-300, 1e300 - k as f64], size: [1.0, 1.0], rotation: 0.0 })
            .collect();
        let p = Page { width: f64::MAX, height: f64::NAN, blocks, images, background: None };
        let bytes = pptx(&[p], "Hostile");
        let slide = part(&bytes, "ppt/slides/slide1.xml");
        assert!(slide.contains("Title") && slide.contains("<p:pic>"), "the slide is still written");
        assert!(!slide.contains("y=\"-"), "positions never wrap negative");
        assert!(!slide.contains("<p:ph"), "text boxes, not placeholders");
    }

    #[test]
    pub fn pptx_empty_pages_produce_valid_slide() {
        let bytes = pptx(&[], "Blank");
        assert!(bytes.starts_with(b"PK"));
        let pres_xml = part(&bytes, "ppt/presentation.xml");
        assert!(pres_xml.contains("<p:sldId id=\"256\" r:id=\"rIdSld1\"/>"));
        let slide_xml = part(&bytes, "ppt/slides/slide1.xml");
        assert!(slide_xml.contains("<p:spTree>"));
    }

    #[test]
    fn pptx_slide_dimensions_and_clamping() {
        // Known dimensions: 720x540 pt (10x7.5 in) = 9144000x6858000 EMU
        let p = Page { width: 720.0, height: 540.0, ..Page::default() };
        let bytes = pptx(&[p], "Known Dimensions");
        let pres_xml = part(&bytes, "ppt/presentation.xml");
        assert!(pres_xml.contains("<p:sldSz cx=\"9144000\" cy=\"6858000\"/>"), "{pres_xml}");

        // Tiny page (10x10 pt) clamps to PowerPoint minimum of 1 inch (914400 EMU)
        let tiny = Page { width: 10.0, height: 10.0, ..Page::default() };
        let tiny_bytes = pptx(&[tiny], "Tiny");
        let tiny_pres = part(&tiny_bytes, "ppt/presentation.xml");
        assert!(tiny_pres.contains("<p:sldSz cx=\"914400\" cy=\"914400\"/>"), "{tiny_pres}");

        // Poster page (10000x10000 pt) clamps to PowerPoint maximum of 56 inches (51206400 EMU)
        let poster = Page { width: 10_000.0, height: 10_000.0, ..Page::default() };
        let poster_bytes = pptx(&[poster], "Poster");
        let poster_pres = part(&poster_bytes, "ppt/presentation.xml");
        assert!(poster_pres.contains("<p:sldSz cx=\"51206400\" cy=\"51206400\"/>"), "{poster_pres}");

        // Non-finite dimensions fall back to default
        let nan_page = Page { width: f64::NAN, height: -1.0, ..Page::default() };
        let nan_bytes = pptx(&[nan_page], "NaN");
        let nan_pres = part(&nan_bytes, "ppt/presentation.xml");
        assert!(nan_pres.contains("<p:sldSz cx=\"9144000\" cy=\"6858000\"/>"), "{nan_pres}");
    }

    #[test]
    fn pptx_table_cell_with_largest_font_stays_in_table_and_not_in_title() {
        let cell = |t: &str, x: f64, y: f64, w: f64, sz: f64| Block {
            text: t.into(),
            rect: [x, y, x + w, y + 14.0],
            size: sz,
            bold: false,
            italic: false,
            color: [0.0; 3],
            rotation: 0.0,
        };
        let p = Page {
            width: 720.0,
            height: 540.0,
            blocks: vec![
                // Heading outside the table (short width, does not span table columns)
                Block {
                    text: "Monthly Summary".into(),
                    rect: [50.0, 480.0, 150.0, 500.0],
                    size: 20.0,
                    bold: true,
                    italic: false,
                    color: [0.0; 3],
                    rotation: 0.0,
                },
                // Table where one cell has a very large number (e.g. 36 pt callout in the table)
                cell("Metric", 72.0, 400.0, 100.0, 12.0),
                cell("Value", 200.0, 400.0, 100.0, 12.0),
                cell("Growth", 72.0, 360.0, 100.0, 12.0),
                cell("+99%", 200.0, 360.0, 100.0, 36.0),
                cell("Target", 72.0, 320.0, 100.0, 12.0),
                cell("100%", 200.0, 320.0, 100.0, 12.0),
            ],
            images: Vec::new(),
            background: None,
        };
        let bytes = pptx(&[p], "Table Check");
        let slide_xml = part(&bytes, "ppt/slides/slide1.xml");
        // Title should be the block outside the table ("Monthly Summary"), not the "+99%" cell
        assert!(slide_xml.contains("Monthly Summary"));
        assert!(slide_xml.contains("name=\"Title 2\""));
        // The table must contain both the cell and explicit borders
        assert!(slide_xml.contains("<a:tbl>"));
        assert!(slide_xml.contains("+99%"));
        assert!(slide_xml.contains("<a:tcPr><a:lnL w=\"12700\" cmpd=\"s\">"));
    }

    #[test]
    fn pptx_opens_in_libreoffice() {
        let p = sample_page();
        let bytes = pptx(&[p], "LibreOffice Check");
        let dir = std::env::temp_dir().join(format!("pdfcraft-pptx-lo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let pptx_path = dir.join("test.pptx");
        std::fs::write(&pptx_path, &bytes).unwrap();

        for cmd in ["libreoffice", "soffice"] {
            let status = std::process::Command::new(cmd).args(["--headless", "--convert-to", "pdf", "--outdir"]).arg(&dir).arg(&pptx_path).status();
            if status.is_ok_and(|st| st.success()) {
                assert!(dir.join("test.pdf").is_file(), "LibreOffice produced test.pdf");
                break;
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
