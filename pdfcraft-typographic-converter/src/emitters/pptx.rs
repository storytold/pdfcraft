use crate::layout::{DocumentLayout, ScriptType};
use anyhow::{Context, Result};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;
use zip::ZipWriter;

/// Escapes special XML characters.
fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
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
    let file = File::create(output_path)
        .with_context(|| format!("Failed to create PPTX output file at {}", output_path.display()))?;

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
  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
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

    // Determine presentation dimensions in EMUs (1 pt = 12700 EMUs)
    let first_page = doc.pages.first();
    let width_emu = first_page
        .map(|p| (p.width * 12700.0).round() as i64)
        .unwrap_or(9144000);
    let height_emu = first_page
        .map(|p| (p.height * 12700.0).round() as i64)
        .unwrap_or(6858000);

    // 4. ppt/presentation.xml
    let mut pres_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
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
        vec![crate::layout::PageLayout {
            page_index: 0,
            width: 792.0,
            height: 612.0,
            lines: Vec::new(),
            images: Vec::new(),
        }]
    } else {
        doc.pages.clone()
    };

    for (page_idx, page) in pages_to_emit.iter().enumerate() {
        let slide_num = page_idx + 1;
        let slide_path = format!("ppt/slides/slide{}.xml", slide_num);

        let mut slide_xml = String::from(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld>
    <p:spTree>
      <p:nvGrpSpPr>
        <p:cNvPr id="1" name=""/>
        <p:cNvGrpSpPr/>
        <p:grpSpPr/>
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

            // Coordinates in EMUs
            let x = (line.bounds.0 * 12700.0).round() as i64;
            // Flip PDF bottom-up Y-axis to top-down
            let y = ((page.height - line.bounds.3) * 12700.0).round() as i64;
            let cx = ((line.bounds.2 - line.bounds.0) * 12700.0)
                .max(100000.0)
                .round() as i64;
            let cy = ((line.bounds.3 - line.bounds.1) * 12700.0)
                .max(100000.0)
                .round() as i64;

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
                let sz = (run.font_size * 100.0).round() as i32;
                let b_attr = if run.is_bold { r#" b="1""# } else { "" };
                let i_attr = if run.is_italic { r#" i="1""# } else { "" };
                let baseline_attr = match run.script {
                    ScriptType::Superscript => r#" baseline="30000""#,
                    ScriptType::Subscript => r#" baseline="-25000""#,
                    ScriptType::Normal => "",
                };

                let escaped_font = escape_xml(&run.font_name);
                let escaped_text = escape_xml(&run.text);

                slide_xml.push_str(&format!(
                    r#"            <a:r>
              <a:rPr sz="{sz}"{b_attr}{i_attr}{baseline_attr}>
                <a:latin typeface="{escaped_font}"/>
              </a:rPr>
              <a:t>{escaped_text}</a:t>
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

        // Emit images (including barcodes and QR codes)
        if !page.images.is_empty() {
            // Write slide relationships file
            let rels_path = format!("ppt/slides/_rels/slide{}.xml.rels", slide_num);
            let mut rels_xml = String::from(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
"#,
            );

            for (img_idx, img) in page.images.iter().enumerate() {
                let img_filename = format!("image_{}_{}.png", slide_num, img_idx + 1);
                let media_path = format!("ppt/media/{}", img_filename);

                // Write PNG image bytes to archive
                zip.start_file(&media_path, options)?;
                zip.write_all(&img.bytes)?;

                rels_xml.push_str(&format!(
                    r#"  <Relationship Id="rIdImg{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/{}"/>
"#,
                    img_idx + 1,
                    img_filename
                ));

                let shape_id = page.lines.len() + 2 + img_idx;
                let x = (img.bounds.0 * 12700.0).round() as i64;
                let y = ((page.height - img.bounds.3) * 12700.0).round() as i64;
                let cx = ((img.bounds.2 - img.bounds.0) * 12700.0)
                    .max(100000.0)
                    .round() as i64;
                let cy = ((img.bounds.3 - img.bounds.1) * 12700.0)
                    .max(100000.0)
                    .round() as i64;

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
        }

        slide_xml.push_str(
            r#"    </p:spTree>
  </p:cSld>
</p:sld>"#,
        );

        zip.start_file(slide_path, options)?;
        zip.write_all(slide_xml.as_bytes())?;
    }

    zip.finish()
        .with_context(|| format!("Failed to finalize PPTX zip archive at {}", output_path.display()))?;

    Ok(())
}
