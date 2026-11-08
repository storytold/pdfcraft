use crate::layout::{DocumentLayout, ScriptType};
use anyhow::{Context, Result};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;
use zip::ZipWriter;

fn is_xml_valid_char(c: char) -> bool {
    matches!(
        c,
        '\u{0009}' | '\u{000A}' | '\u{000D}' | '\u{0020}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}'
    )
}

/// Escapes special XML characters and discards XML-forbidden control characters.
fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if !is_xml_valid_char(c) {
            continue;
        }
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Exports a `DocumentLayout` to a Microsoft PowerPoint presentation (.pptx).
pub fn export_pptx(doc: &DocumentLayout, output_path: &Path) -> Result<()> {
    let file = File::create(output_path).with_context(|| format!("Failed to create PPTX output file at {}", output_path.display()))?;

    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let num_slides = doc.pages.len().max(1);

    // 1. [Content_Types].xml
    let mut content_types = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="png" ContentType="image/png"/>
  <Default Extension="jpeg" ContentType="image/jpeg"/>
  <Default Extension="jpg" ContentType="image/jpeg"/>
  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
  <Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/>
  <Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/>
  <Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>
"#,
    );

    for i in 1..=num_slides {
        content_types.push_str(&format!(
            r#"  <Override PartName="/ppt/slides/slide{}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
"#,
            i
        ));
    }
    content_types.push_str("</Types>");

    zip.start_file("[Content_Types].xml", options)?;
    zip.write_all(content_types.as_bytes())?;

    // 2. _rels/.rels
    let root_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#;

    zip.start_file("_rels/.rels", options)?;
    zip.write_all(root_rels.as_bytes())?;

    // 3. ppt/_rels/presentation.xml.rels
    let mut pres_rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdMaster1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/>
"#,
    );

    for i in 1..=num_slides {
        pres_rels.push_str(&format!(
            r#"  <Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{}.xml"/>
"#,
            i, i
        ));
    }
    pres_rels.push_str("</Relationships>");

    zip.start_file("ppt/_rels/presentation.xml.rels", options)?;
    zip.write_all(pres_rels.as_bytes())?;

    // Minimal Slide Master, Layout, and Theme
    let theme_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office Theme">
  <a:themeElements>
    <a:clrScheme name="Office">
      <a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>
      <a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
      <a:dk2><a:srgbClr val="1F497D"/></a:dk2>
      <a:lt2><a:srgbClr val="EEECE1"/></a:lt2>
      <a:accent1><a:srgbClr val="4F81BD"/></a:accent1>
      <a:accent2><a:srgbClr val="C0504D"/></a:accent2>
      <a:accent3><a:srgbClr val="9BBB59"/></a:accent3>
      <a:accent4><a:srgbClr val="8064A2"/></a:accent4>
      <a:accent5><a:srgbClr val="4BACC6"/></a:accent5>
      <a:accent6><a:srgbClr val="F79646"/></a:accent6>
      <a:hlink><a:srgbClr val="0000FF"/></a:hlink>
      <a:folHlink><a:srgbClr val="800080"/></a:folHlink>
    </a:clrScheme>
    <a:fontScheme name="Office">
      <a:majorFont><a:latin typeface="Calibri"/></a:majorFont>
      <a:minorFont><a:latin typeface="Calibri"/></a:minorFont>
    </a:fontScheme>
    <a:fmtScheme name="Office">
      <a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst>
      <a:lnStyleLst><a:ln w="9525"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst>
      <a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>
      <a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst>
    </a:fmtScheme>
  </a:themeElements>
</a:theme>"#;
    zip.start_file("ppt/theme/theme1.xml", options)?;
    zip.write_all(theme_xml.as_bytes())?;

    let slide_master_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld>
    <p:spTree>
      <p:nvGrpSpPr>
        <p:cNvPr id="1" name=""/>
        <p:cNvGrpSpPr/>
        <p:nvPr/>
      </p:nvGrpSpPr>
      <p:grpSpPr>
        <a:xfrm>
          <a:off x="0" y="0"/>
          <a:ext cx="0" cy="0"/>
          <a:chOff x="0" y="0"/>
          <a:chExt cx="0" cy="0"/>
        </a:xfrm>
      </p:grpSpPr>
    </p:spTree>
  </p:cSld>
  <p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>
  <p:sldLayoutIdLst>
    <p:sldLayoutId id="2147483649" r:id="rId1"/>
  </p:sldLayoutIdLst>
</p:sldMaster>"#;
    zip.start_file("ppt/slideMasters/slideMaster1.xml", options)?;
    zip.write_all(slide_master_xml.as_bytes())?;

    let slide_master_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/>
</Relationships>"#;
    zip.start_file("ppt/slideMasters/_rels/slideMaster1.xml.rels", options)?;
    zip.write_all(slide_master_rels.as_bytes())?;

    let slide_layout_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" type="blank" preserve="1">
  <p:cSld>
    <p:spTree>
      <p:nvGrpSpPr>
        <p:cNvPr id="1" name=""/>
        <p:cNvGrpSpPr/>
        <p:nvPr/>
      </p:nvGrpSpPr>
      <p:grpSpPr>
        <a:xfrm>
          <a:off x="0" y="0"/>
          <a:ext cx="0" cy="0"/>
          <a:chOff x="0" y="0"/>
          <a:chExt cx="0" cy="0"/>
        </a:xfrm>
      </p:grpSpPr>
    </p:spTree>
  </p:cSld>
  <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>
</p:sldLayout>"#;
    zip.start_file("ppt/slideLayouts/slideLayout1.xml", options)?;
    zip.write_all(slide_layout_xml.as_bytes())?;

    let slide_layout_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/>
</Relationships>"#;
    zip.start_file("ppt/slideLayouts/_rels/slideLayout1.xml.rels", options)?;
    zip.write_all(slide_layout_rels.as_bytes())?;

    // Determine presentation dimensions in EMUs (1 pt = 12700 EMUs), accommodating every page
    let max_width = doc.pages.iter().map(|p| p.width as f64).fold(0.0, f64::max);
    let max_height = doc.pages.iter().map(|p| p.height as f64).fold(0.0, f64::max);
    let width_emu = if max_width > 0.0 { (max_width * 12700.0).round() as i64 } else { 9144000 };
    let height_emu = if max_height > 0.0 { (max_height * 12700.0).round() as i64 } else { 6858000 };

    // 4. ppt/presentation.xml
    let mut pres_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:sldMasterIdLst>
    <p:sldMasterId id="2147483648" r:id="rIdMaster1"/>
  </p:sldMasterIdLst>
  <p:sldIdLst>
"#.to_string();

    for i in 1..=num_slides {
        pres_xml.push_str(&format!(
            r#"    <p:sldId id="{}" r:id="rId{}"/>
"#,
            255 + i,
            i
        ));
    }

    pres_xml.push_str(&format!(
        r#"  </p:sldIdLst>
  <p:sldSz cx="{}" cy="{}"/>
  <p:notesSz cx="6858000" cy="9144000"/>
</p:presentation>"#,
        width_emu, height_emu
    ));

    zip.start_file("ppt/presentation.xml", options)?;
    zip.write_all(pres_xml.as_bytes())?;

    // 5. ppt/slides/slide{N}.xml
    let pages_to_emit: Vec<_> = if doc.pages.is_empty() {
        // Fallback for empty document layout: emit a single blank slide
        vec![crate::layout::PageLayout { page_index: 0, width: 792.0, height: 612.0, lines: Vec::new(), images: Vec::new() }]
    } else {
        doc.pages.clone()
    };

    for (page_idx, page) in pages_to_emit.iter().enumerate() {
        let slide_num = page_idx + 1;
        let slide_path = format!("ppt/slides/slide{}.xml", slide_num);

        let scale = if page.width > 0.0 && page.height > 0.0 && max_width > 0.0 && max_height > 0.0 {
            (max_width / page.width as f64).min(max_height / page.height as f64)
        } else {
            1.0
        };

        let mut slide_xml = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld>
    <p:spTree>
      <p:nvGrpSpPr>
        <p:cNvPr id="1" name=""/>
        <p:cNvGrpSpPr/>
        <p:nvPr/>
      </p:nvGrpSpPr>
      <p:grpSpPr>
        <a:xfrm>
          <a:off x="0" y="0"/>
          <a:ext cx="0" cy="0"/>
          <a:chOff x="0" y="0"/>
          <a:chExt cx="0" cy="0"/>
        </a:xfrm>
      </p:grpSpPr>
"#,
        );

        for (line_idx, line) in page.lines.iter().enumerate() {
            let shape_id = line_idx + 2;

            // Coordinates in EMUs fitted to slide dimensions
            let x = (line.bounds.0 as f64 * scale * 12700.0).round() as i64;
            // Flip PDF bottom-up Y-axis to top-down
            let y = ((max_height - line.bounds.3 as f64 * scale) * 12700.0).round() as i64;
            let cx = ((line.bounds.2 - line.bounds.0) as f64 * scale * 12700.0).max(100000.0).round() as i64;
            let cy = ((line.bounds.3 - line.bounds.1) as f64 * scale * 12700.0).max(100000.0).round() as i64;

            slide_xml.push_str(&format!(
                r#"      <p:sp>
        <p:nvSpPr>
          <p:cNvPr id="{shape_id}" name="TextBox {shape_id}"/>
          <p:cNvSpPr txBox="1"/>
          <p:nvPr/>
        </p:nvSpPr>
        <p:spPr>
          <a:xfrm>
            <a:off x="{x}" y="{y}"/>
            <a:ext cx="{cx}" cy="{cy}"/>
          </a:xfrm>
          <a:prstGeom prst="rect">
            <a:avLst/>
          </a:prstGeom>
          <a:noFill/>
        </p:spPr>
        <p:txBody>
          <a:bodyPr wrap="none" rtlCol="0">
            <a:spAutoFit/>
          </a:bodyPr>
          <a:lstStyle/>
          <a:p>
"#
            ));

            for run in &line.runs {
                let sz = (((run.font_size as f64 * scale) * 100.0).round() as i32).clamp(100, 400000);
                let b_attr = if run.is_bold { r#" b="1""# } else { "" };
                let i_attr = if run.is_italic { r#" i="1""# } else { "" };
                let baseline_attr = match run.script {
                    ScriptType::Superscript => r#" baseline="30000""#,
                    ScriptType::Subscript => r#" baseline="-25000""#,
                    ScriptType::Normal => "",
                };

                let escaped_font = escape_xml(&run.font_name);
                let escaped_text = escape_xml(&run.text);
                let text_content = if run.has_leading_tab { format!("\t{}", escaped_text) } else { escaped_text };

                slide_xml.push_str(&format!(
                    r#"            <a:r>
              <a:rPr sz="{sz}"{b_attr}{i_attr}{baseline_attr}>
                <a:latin typeface="{escaped_font}"/>
              </a:rPr>
              <a:t>{text_content}</a:t>
            </a:r>
"#
                ));
            }

            if line.runs.is_empty() {
                slide_xml.push_str("            <a:endParaRPr/>\n");
            }

            slide_xml.push_str(
                r#"          </a:p>
        </p:txBody>
      </p:sp>
"#,
            );
        }

        // Slide relationships file (referencing slideLayout1 and any embedded images)
        let rels_path = format!("ppt/slides/_rels/slide{}.xml.rels", slide_num);
        let mut rels_xml = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdLayout1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
"#,
        );

        // Emit images (including barcodes and QR codes)
        for (img_idx, img) in page.images.iter().enumerate() {
            let is_jpeg = img.bytes.starts_with(&[0xFF, 0xD8]);
            let ext = if is_jpeg { "jpeg" } else { "png" };
            let img_filename = format!("image_{}_{}.{}", slide_num, img_idx + 1, ext);
            let media_path = format!("ppt/media/{}", img_filename);

            // Write image bytes to archive
            zip.start_file(&media_path, options)?;
            zip.write_all(&img.bytes)?;

            rels_xml.push_str(&format!(
                r#"  <Relationship Id="rIdImg{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/{}"/>
"#,
                img_idx + 1,
                img_filename
            ));

            let shape_id = page.lines.len() + 2 + img_idx;
            let min_x = img.bounds.0.min(img.bounds.2);
            let max_x = img.bounds.0.max(img.bounds.2);
            let min_y = img.bounds.1.min(img.bounds.3);
            let max_y = img.bounds.1.max(img.bounds.3);
            let x = (min_x as f64 * scale * 12700.0).round() as i64;
            let y = ((max_height - max_y as f64 * scale) * 12700.0).round() as i64;
            let cx = ((max_x - min_x) as f64 * scale * 12700.0).max(100000.0).round() as i64;
            let cy = ((max_y - min_y) as f64 * scale * 12700.0).max(100000.0).round() as i64;

            slide_xml.push_str(&format!(
                r#"      <p:pic>
    <p:nvPicPr>
      <p:cNvPr id="{shape_id}" name="Picture {shape_id}"/>
      <p:cNvPicPr>
        <a:picLocks noChangeAspect="1"/>
      </p:cNvPicPr>
      <p:nvPr/>
    </p:nvPicPr>
    <p:blipFill>
      <a:blip r:embed="rIdImg{}"/>
      <a:stretch>
        <a:fillRect/>
      </a:stretch>
    </p:blipFill>
    <p:spPr>
      <a:xfrm>
        <a:off x="{x}" y="{y}"/>
        <a:ext cx="{cx}" cy="{cy}"/>
      </a:xfrm>
      <a:prstGeom prst="rect">
        <a:avLst/>
      </a:prstGeom>
    </p:spPr>
  </p:pic>
"#,
                img_idx + 1
            ));
        }

        rels_xml.push_str("</Relationships>");
        zip.start_file(rels_path, options)?;
        zip.write_all(rels_xml.as_bytes())?;

        slide_xml.push_str(
            r#"    </p:spTree>
  </p:cSld>
</p:sld>"#,
        );

        zip.start_file(slide_path, options)?;
        zip.write_all(slide_xml.as_bytes())?;
    }

    zip.finish().with_context(|| format!("Failed to finalize PPTX zip archive at {}", output_path.display()))?;

    Ok(())
}
