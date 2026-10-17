//! SpreadsheetML and EPUB 3.3 writers. Content is reflowed, not a facsimile.
//! https://learn.microsoft.com/office/open-xml/spreadsheet/structure-of-a-spreadsheetml-document
//! https://www.w3.org/TR/epub-33/

use super::{Item, Page, Zip, esc, hex, items};

const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PKG_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const SHEET: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";
const MAX_BYTES: usize = 256 * 1024 * 1024;

/// Bounds the legacy ZIP32 writer before every addition, including generated XML.
struct Archive {
    zip: Zip,
    size: usize,
    entries: usize,
}

impl Archive {
    fn new() -> Self {
        Self { zip: Zip::default(), size: 0, entries: 0 }
    }
    fn add(&mut self, name: &str, data: &[u8], compress: bool) -> Result<(), String> {
        self.size = self.size.saturating_add(data.len()).saturating_add(name.len() * 2 + 128);
        self.entries += 1;
        if self.size > MAX_BYTES || self.entries > 60_000 {
            return Err("export exceeds the 256 MiB or 60000-file archive limit; export fewer pages".into());
        }
        self.zip.add(name, data, compress);
        Ok(())
    }
}

fn validate(pages: &[Page], title: &str) -> Result<(), String> {
    if pages.is_empty() || pages.len() > 10_000 || title.len() > 16_384 {
        return Err("export needs 1..10000 pages and a title of at most 16384 bytes".into());
    }
    let mut bytes = 0usize;
    for page in pages {
        if page.blocks.len() > 100_000 || page.images.len() > 10_000 {
            return Err("page exceeds the export block/image limit".into());
        }
        for block in &page.blocks {
            if block.text.len() > 8 * 1024 * 1024 {
                return Err("text block exceeds the 8 MiB export limit".into());
            }
            bytes = bytes.saturating_add(block.text.len());
        }
        for image in &page.images {
            if !matches!(image.ext, "png" | "jpg") {
                return Err("export supports PNG and JPEG images only".into());
            }
            bytes = bytes.saturating_add(image.bytes.len());
        }
        if bytes > MAX_BYTES / 8 {
            return Err("export input exceeds 32 MiB; export fewer pages".into());
        }
    }
    Ok(())
}

fn relationships(body: &str) -> String {
    format!("{XML}<Relationships xmlns=\"{PKG_REL}\">{body}</Relationships>")
}

fn relationship(id: &str, kind: &str, target: &str) -> String {
    format!("<Relationship Id=\"{id}\" Type=\"{REL}/{kind}\" Target=\"{target}\"/>")
}

fn column(mut index: usize) -> String {
    let mut chars = Vec::new();
    loop {
        chars.push(char::from(b'A' + (index % 26) as u8));
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    chars.into_iter().rev().collect()
}

/// Escape SpreadsheetML's literal `_xHHHH_` sequences before XML escaping.
fn sheet_text(text: &str) -> String {
    let mut out = String::new();
    for (offset, c) in text.char_indices() {
        if c == '_'
            && text
                .get(offset..offset.saturating_add(7))
                .is_some_and(|s| s.starts_with("_x") && s.ends_with('_') && s.get(2..6).is_some_and(|hex| hex.bytes().all(|c| c.is_ascii_hexdigit())))
        {
            out.push_str("_x005F_");
        } else {
            out.push(c);
        }
    }
    esc(&out)
}

fn cell(text: &str, col: usize, row: usize, bold: bool, italic: bool, color: [f64; 3]) -> Result<String, String> {
    // Excel's limit is UTF-16 code units, not bytes or Unicode scalar values.
    if text.encode_utf16().count() > 32_767 {
        return Err("a spreadsheet cell exceeds Excel's 32767-character limit; use another output format".into());
    }
    let mut properties = String::new();
    if bold {
        properties.push_str("<b/>");
    }
    if italic {
        properties.push_str("<i/>");
    }
    if let Some(color) = hex(color) {
        properties.push_str(&format!("<color rgb=\"FF{color}\"/>"));
    }
    // Always strings: OCR must not silently change identifiers, dates or formulas.
    Ok(format!(
        "<c r=\"{}{row}\" t=\"inlineStr\" s=\"0\"><is><r><rPr>{properties}</rPr><t xml:space=\"preserve\">{}</t></r></is></c>",
        column(col),
        sheet_text(text)
    ))
}

/// One worksheet per PDF page. Detected tables become cells/merged ranges; other
/// paragraphs become rows. Images stay at their position in the reading order.
/// Text is never inferred to be a number or an executable spreadsheet formula.
pub fn xlsx(pages: &[Page], title: &str) -> Result<Vec<u8>, String> {
    validate(pages, title)?;
    let mut archive = Archive::new();
    let mut types = String::from(
        "<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Default Extension=\"png\" ContentType=\"image/png\"/><Default Extension=\"jpg\" ContentType=\"image/jpeg\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>",
    );
    let mut sheets = String::new();
    let mut workbook_rels = relationship("styles", "styles", "styles.xml");
    for (index, page) in pages.iter().enumerate() {
        let number = index + 1;
        sheets.push_str(&format!("<sheet name=\"Page {number}\" sheetId=\"{number}\" r:id=\"s{number}\"/>"));
        workbook_rels.push_str(&relationship(&format!("s{number}"), "worksheet", &format!("worksheets/sheet{number}.xml")));
        types.push_str(&format!("<Override PartName=\"/xl/worksheets/sheet{number}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"));
        let mut rows = String::new();
        let mut merges = Vec::new();
        let mut drawing = String::new();
        let mut drawing_rels = String::new();
        let mut row = 1usize;
        let mut image_number = 0usize;
        for item in items(std::slice::from_ref(page)) {
            match item {
                Item::Para(block, _) => {
                    rows.push_str(&format!("<row r=\"{row}\">{}</row>", cell(&block.text, 0, row, block.bold, block.italic, block.color)?));
                    row += 1;
                }
                Item::Table(table) => {
                    for cells in table.rows {
                        rows.push_str(&format!("<row r=\"{row}\">"));
                        let mut col = 0;
                        for c in cells {
                            rows.push_str(&cell(&c.text, col, row, c.bold, c.italic, c.color)?);
                            if c.span > 1 {
                                merges.push(format!("<mergeCell ref=\"{}{row}:{}{row}\"/>", column(col), column(col + c.span - 1)));
                            }
                            col += c.span;
                        }
                        rows.push_str("</row>");
                        row += 1;
                    }
                }
                Item::Img(image) => {
                    image_number += 1;
                    let name = format!("page{number}-image{image_number}.{}", image.ext);
                    archive.add(&format!("xl/media/{name}"), &image.bytes, false)?;
                    let width = finite_size(image.rect[2] - image.rect[0]);
                    let height = finite_size(image.rect[3] - image.rect[1]);
                    let scale = (480.0 / width).min(600.0 / height).min(1.0);
                    let cx = (width * scale * 12700.0).round() as u64;
                    let cy = (height * scale * 12700.0).round() as u64;
                    drawing.push_str(&format!("<xdr:oneCellAnchor><xdr:from><xdr:col>0</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:ext cx=\"{cx}\" cy=\"{cy}\"/><xdr:pic><xdr:nvPicPr><xdr:cNvPr id=\"{image_number}\" name=\"Image {image_number}\"/><xdr:cNvPicPr/></xdr:nvPicPr><xdr:blipFill><a:blip r:embed=\"i{image_number}\"/><a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic><xdr:clientData/></xdr:oneCellAnchor>", row - 1));
                    drawing_rels.push_str(&relationship(&format!("i{image_number}"), "image", &format!("../media/{name}")));
                    row += (height * scale / 15.0).ceil() as usize + 1;
                }
                Item::PageBreak => {}
            }
            if row > 1_048_576 || rows.len() > MAX_BYTES / 2 {
                return Err("worksheet exceeds Excel's row limit or the export size limit".into());
            }
        }
        let merges =
            if merges.is_empty() { String::new() } else { format!("<mergeCells count=\"{}\">{}</mergeCells>", merges.len(), merges.concat()) };
        let has_images = image_number > 0;
        if has_images {
            types.push_str(&format!(
                "<Override PartName=\"/xl/drawings/drawing{number}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.drawing+xml\"/>"
            ));
            archive.add(&format!("xl/drawings/drawing{number}.xml"), format!("{XML}<xdr:wsDr xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"{REL}\">{drawing}</xdr:wsDr>").as_bytes(), true)?;
            archive.add(&format!("xl/drawings/_rels/drawing{number}.xml.rels"), relationships(&drawing_rels).as_bytes(), true)?;
            archive.add(
                &format!("xl/worksheets/_rels/sheet{number}.xml.rels"),
                relationships(&relationship("drawing", "drawing", &format!("../drawings/drawing{number}.xml"))).as_bytes(),
                true,
            )?;
        }
        let drawing_ref = if has_images { "<drawing r:id=\"drawing\"/>" } else { "" };
        archive.add(&format!("xl/worksheets/sheet{number}.xml"), format!("{XML}<worksheet xmlns=\"{SHEET}\" xmlns:r=\"{REL}\"><sheetFormatPr defaultColWidth=\"20\" defaultRowHeight=\"15\"/><sheetData>{rows}</sheetData>{merges}{drawing_ref}</worksheet>").as_bytes(), true)?;
    }
    archive.add("xl/styles.xml", format!("{XML}<styleSheet xmlns=\"{SHEET}\"><fonts count=\"1\"><font><sz val=\"11\"/><name val=\"Arial\"/></font></fonts><fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill></fills><borders count=\"1\"><border/></borders><cellStyleXfs count=\"1\"><xf/></cellStyleXfs><cellXfs count=\"1\"><xf xfId=\"0\" applyAlignment=\"1\"><alignment vertical=\"top\" wrapText=\"1\"/></xf></cellXfs><cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles></styleSheet>").as_bytes(), true)?;
    archive.add(
        "xl/workbook.xml",
        format!("{XML}<workbook xmlns=\"{SHEET}\" xmlns:r=\"{REL}\"><sheets>{sheets}</sheets></workbook>").as_bytes(),
        true,
    )?;
    archive.add("xl/_rels/workbook.xml.rels", relationships(&workbook_rels).as_bytes(), true)?;
    archive.add("_rels/.rels", relationships(&relationship("workbook", "officeDocument", "xl/workbook.xml")).as_bytes(), true)?;
    archive.add(
        "[Content_Types].xml",
        format!("{XML}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">{types}</Types>").as_bytes(),
        true,
    )?;
    Ok(archive.zip.finish())
}

fn finite_size(value: f64) -> f64 {
    if value.is_finite() { value.clamp(1.0, 14400.0) } else { 72.0 }
}

fn rich(text: &str, bold: bool, italic: bool) -> String {
    let mut text = esc(text);
    if italic {
        text = format!("<em>{text}</em>");
    }
    if bold {
        text = format!("<strong>{text}</strong>");
    }
    text
}

/// EPUB 3.3 with a navigation document, one reflowable XHTML chapter per page,
/// real tables and packaged images. `language` is a BCP-47 tag, or `und` if unknown.
/// Reproducible exports use the ZIP epoch for the required modification timestamp.
pub fn epub(pages: &[Page], title: &str, language: &str) -> Result<Vec<u8>, String> {
    validate(pages, title)?;
    if language.split('-').next().is_none_or(|primary| !(2..=8).contains(&primary.len()) || !primary.bytes().all(|c| c.is_ascii_alphabetic()))
        || language.len() > 64
        || !language.split('-').all(|part| !part.is_empty() && part.len() <= 8 && part.bytes().all(|c| c.is_ascii_alphanumeric()))
    {
        return Err("EPUB language must be a BCP-47 language tag (use und when unknown)".into());
    }
    let mut archive = Archive::new();
    archive.add("mimetype", b"application/epub+zip", false)?;
    archive.add("META-INF/container.xml", format!("{XML}<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\"><rootfiles><rootfile full-path=\"EPUB/package.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>").as_bytes(), true)?;
    let mut manifest = String::from("<item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>");
    let mut spine = String::new();
    let mut nav = String::new();
    let mut fingerprint = 14695981039346656037u64;
    for byte in title.as_bytes() {
        fingerprint = (fingerprint ^ u64::from(*byte)).wrapping_mul(1099511628211);
    }
    for (index, page) in pages.iter().enumerate() {
        let number = index + 1;
        let mut body = String::new();
        let mut image_number = 0;
        for item in items(std::slice::from_ref(page)) {
            match item {
                Item::Para(block, level) => {
                    let tag = if level == 0 { "p".to_owned() } else { format!("h{level}") };
                    body.push_str(&format!("<{tag}>{}</{tag}>", rich(&block.text, block.bold, block.italic)));
                }
                Item::Table(table) => {
                    body.push_str("<table>");
                    for row in table.rows {
                        body.push_str("<tr>");
                        for cell in row {
                            body.push_str(&format!("<td colspan=\"{}\">{}</td>", cell.span, rich(&cell.text, cell.bold, cell.italic)));
                        }
                        body.push_str("</tr>");
                    }
                    body.push_str("</table>");
                }
                Item::Img(image) => {
                    image_number += 1;
                    let name = format!("images/page{number}-image{image_number}.{}", image.ext);
                    let mime = if image.ext == "jpg" { "image/jpeg" } else { "image/png" };
                    archive.add(&format!("EPUB/{name}"), &image.bytes, false)?;
                    manifest.push_str(&format!("<item id=\"p{number}i{image_number}\" href=\"{name}\" media-type=\"{mime}\"/>"));
                    body.push_str(&format!("<p><img src=\"{name}\" alt=\"Image {image_number} on page {number}\"/></p>"));
                    for byte in &image.bytes {
                        fingerprint = (fingerprint ^ u64::from(*byte)).wrapping_mul(1099511628211);
                    }
                }
                Item::PageBreak => {}
            }
            if body.len() > MAX_BYTES / 2 {
                return Err("EPUB page exceeds the export size limit".into());
            }
        }
        for byte in body.as_bytes() {
            fingerprint = (fingerprint ^ u64::from(*byte)).wrapping_mul(1099511628211);
        }
        archive.add(&format!("EPUB/page{number}.xhtml"), xhtml(&format!("{title} — {number}"), language, &body).as_bytes(), true)?;
        manifest.push_str(&format!("<item id=\"p{number}\" href=\"page{number}.xhtml\" media-type=\"application/xhtml+xml\"/>"));
        spine.push_str(&format!("<itemref idref=\"p{number}\"/>"));
        nav.push_str(&format!("<li><a href=\"page{number}.xhtml\">{number}</a></li>"));
    }
    archive.add(
        "EPUB/nav.xhtml",
        xhtml(title, language, &format!("<nav epub:type=\"toc\" id=\"toc\"><h1>{}</h1><ol>{nav}</ol></nav>", esc(title))).as_bytes(),
        true,
    )?;
    archive.add("EPUB/package.opf", format!("{XML}<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"book-id\"><metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:identifier id=\"book-id\">urn:pdfcraft:{fingerprint:016x}</dc:identifier><dc:title>{}</dc:title><dc:language>{language}</dc:language><meta property=\"dcterms:modified\">1980-01-01T00:00:00Z</meta></metadata><manifest>{manifest}</manifest><spine>{spine}</spine></package>", esc(if title.trim().is_empty() { "PdfCraft document" } else { title })).as_bytes(), true)?;
    Ok(archive.zip.finish())
}

fn xhtml(title: &str, language: &str, body: &str) -> String {
    format!(
        "{XML}<!DOCTYPE html><html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" lang=\"{language}\" xml:lang=\"{language}\"><head><title>{}</title><style>body{{line-height:1.45}}img{{max-width:100%;height:auto}}table{{border-collapse:collapse}}td{{border:1px solid;padding:.2em;vertical-align:top}}p,td{{white-space:pre-wrap}}</style></head><body>{body}</body></html>",
        esc(title)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Block, Image};
    use std::{collections::BTreeMap, io::Read};

    fn parts(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
        let mut parts = BTreeMap::new();
        let mut offset = 0;
        while bytes[offset..].starts_with(b"PK\x03\x04") {
            let word = |at: usize, len: usize| bytes[at..at + len].iter().rev().fold(0usize, |n, b| n * 256 + usize::from(*b));
            let method = word(offset + 8, 2);
            let size = word(offset + 18, 4);
            let name_len = word(offset + 26, 2);
            let extra_len = word(offset + 28, 2);
            let name = String::from_utf8(bytes[offset + 30..offset + 30 + name_len].to_vec()).unwrap();
            let start = offset + 30 + name_len + extra_len;
            let body = &bytes[start..start + size];
            let data = if method == 0 {
                body.to_vec()
            } else {
                let mut data = Vec::new();
                flate2::read::DeflateDecoder::new(body).read_to_end(&mut data).unwrap();
                data
            };
            assert!(parts.insert(name, data).is_none());
            offset = start + size;
        }
        parts
    }

    fn fixture() -> Page {
        let mut page = Page { width: 600.0, height: 800.0, ..Page::default() };
        for (row, texts) in [["=1+1", "00123"], ["日本語", "français"], ["_x0041_", "<&>\" — ß"]].iter().enumerate() {
            for (col, text) in texts.iter().enumerate() {
                let x = 40.0 + col as f64 * 200.0;
                let y = 600.0 - row as f64 * 30.0;
                page.blocks.push(Block {
                    text: (*text).into(),
                    rect: [x, y, x + 100.0, y + 12.0],
                    size: 12.0,
                    bold: row == 0,
                    italic: false,
                    color: [0.0; 3],
                    rotation: 0.0,
                });
            }
        }
        // Original synthetic 1x1 PNG. Binary data is generated by the fixture code,
        // not a downloaded/committed document asset.
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let chunk = |out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]| {
            out.extend((data.len() as u32).to_be_bytes());
            out.extend(kind);
            out.extend(data);
            let mut crc = 0xffff_ffffu32;
            for byte in kind.iter().chain(data) {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb8_8320 } else { 0 };
                }
            }
            out.extend((!crc).to_be_bytes());
        };
        chunk(&mut png, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]);
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut z, &[0, 30, 90, 170]).unwrap();
        chunk(&mut png, b"IDAT", &z.finish().unwrap());
        chunk(&mut png, b"IEND", &[]);
        page.images.push(Image { ext: "png", bytes: png, rect: [40.0, 400.0, 140.0, 500.0], size: [100.0, 100.0], rotation: 0.0 });
        page
    }

    #[test]
    fn spreadsheets_preserve_text_tables_and_images_without_formulas() {
        let bytes = xlsx(&[fixture(), fixture()], "Report").unwrap();
        let parts = parts(&bytes);
        let sheet = String::from_utf8(parts["xl/worksheets/sheet1.xml"].clone()).unwrap();
        for expected in
            ["r=\"A1\" t=\"inlineStr\"", "r=\"B1\" t=\"inlineStr\"", "=1+1", "00123", "日本語", "français", "_x005F_x0041_", "&lt;&amp;&gt;&quot;"]
        {
            assert!(sheet.contains(expected), "{expected}");
        }
        assert!(!sheet.contains("<f>"));
        assert!(parts.contains_key("xl/worksheets/sheet2.xml"));
        assert_eq!(parts["xl/media/page1-image1.png"], fixture().images[0].bytes);
        assert!(String::from_utf8(parts["xl/drawings/drawing1.xml"].clone()).unwrap().contains("r:embed=\"i1\""));
    }

    #[test]
    fn epub_has_stored_first_mimetype_nav_spine_and_real_images() {
        let bytes = epub(&[fixture(), fixture()], "Report & notes", "de-AT").unwrap();
        assert_eq!(&bytes[8..10], &[0, 0]);
        assert_eq!(&bytes[28..30], &[0, 0]);
        assert_eq!(&bytes[30..38], b"mimetype");
        assert_eq!(&bytes[38..58], b"application/epub+zip");
        let parts = parts(&bytes);
        let opf = String::from_utf8(parts["EPUB/package.opf"].clone()).unwrap();
        for expected in ["Report &amp; notes", "<dc:language>de-AT</dc:language>", "idref=\"p1\"", "idref=\"p2\"", "properties=\"nav\""] {
            assert!(opf.contains(expected));
        }
        let chapter = String::from_utf8(parts["EPUB/page1.xhtml"].clone()).unwrap();
        assert!(chapter.contains("<table>"));
        assert!(chapter.contains("日本語"));
        assert_eq!(parts["EPUB/images/page1-image1.png"], fixture().images[0].bytes);
        assert_eq!(bytes, epub(&[fixture(), fixture()], "Report & notes", "de-AT").unwrap());
    }

    #[test]
    fn limits_fail_before_truncating_or_creating_invalid_documents() {
        assert!(xlsx(&[], "Empty").is_err());
        assert!(epub(&[fixture()], "Title", "en\" bad").is_err());
        assert!(epub(&[fixture()], "Title", "123").is_err());
        let mut page = fixture();
        page.blocks[0].text = "😀".repeat(16_384);
        assert!(xlsx(&[page], "Too long").unwrap_err().contains("32767"));
        assert_eq!(column(26), "AA");
        assert_eq!(column(16_383), "XFD");
    }

    #[test]
    fn external_validation_fixture_when_requested() {
        if let Ok(directory) = std::env::var("PDFCRAFT_EXPORT_VALIDATION_DIR") {
            std::fs::write(std::path::Path::new(&directory).join("export.xlsx"), xlsx(&[fixture(), fixture()], "Test report").unwrap()).unwrap();
            std::fs::write(std::path::Path::new(&directory).join("export.epub"), epub(&[fixture(), fixture()], "Test report", "de").unwrap())
                .unwrap();
        }
    }
}
