//! Text/vector XPS print intermediate → PdfCraft's PDF writer. Never renders a page to pixels.
//! Specification: ECMA-388; Glyphs.Indices follows Microsoft's public XPS documentation.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{Cursor, Read};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use roxmltree::Node;
use skrifa::{
    FontRef, GlyphId, MetadataProvider,
    instance::{LocationRef, Size},
};

use crate::CreateError;

type Result<T> = std::result::Result<T, CreateError>;
const MAX_PART: usize = 64 * 1024 * 1024;
const MAX_TOTAL: usize = 256 * 1024 * 1024;
const MAX_NODES: u32 = 100_000;

fn bad(message: impl Into<String>) -> CreateError {
    CreateError::Invalid(format!("XPS: {}", message.into()))
}

struct Package<'a> {
    zip: zip::ZipArchive<Cursor<&'a [u8]>>,
    parts: BTreeMap<String, Vec<usize>>,
    remaining: usize,
    image_bytes: usize,
    glyphs_left: usize,
}

impl<'a> Package<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() > MAX_TOTAL {
            return Err(bad("package exceeds 256 MiB"));
        }
        // Bound the index before the ZIP library allocates central-directory metadata.
        check_directory_budget(bytes)?;
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| bad(e.to_string()))?;
        if zip.len() > 20_000 {
            return Err(bad("too many package parts"));
        }
        let mut names = BTreeSet::new();
        let mut parts = BTreeMap::new();
        let mut pieces: BTreeMap<String, Vec<(usize, bool, usize)>> = BTreeMap::new();
        for i in 0..zip.len() {
            let part = zip.by_index(i).map_err(|e| bad(e.to_string()))?;
            let name = part.name().map_err(|e| bad(e.to_string()))?;
            if name.starts_with('/')
                || name.contains('\\')
                || name.contains(':')
                || name.split('/').any(|p| p == ".." || p == ".")
                || !names.insert(name.to_string())
            {
                return Err(bad("noncanonical or duplicate package part"));
            }
            let name = name.to_string();
            if let Some((base, leaf)) = name.rsplit_once('/')
                && leaf.ends_with(".piece")
            {
                let (digits, last) = if let Some(digits) = leaf.strip_suffix("].last.piece") {
                    (digits, true)
                } else {
                    (leaf.strip_suffix("].piece").ok_or_else(|| bad("invalid piece name"))?, false)
                };
                let digits = digits.strip_prefix('[').ok_or_else(|| bad("invalid piece name"))?;
                let number: usize = digits.parse().map_err(|_| bad("invalid piece number"))?;
                if number.to_string() != digits || base.is_empty() {
                    return Err(bad("noncanonical piece name"));
                }
                pieces.entry(base.to_owned()).or_default().push((number, last, i));
            } else {
                parts.insert(name, vec![i]);
            }
        }
        for (name, mut sequence) in pieces {
            if parts.contains_key(&name) {
                return Err(bad("part has both whole and interleaved representations"));
            }
            sequence.sort_unstable_by_key(|p| p.0);
            if sequence.iter().enumerate().any(|(i, (number, last, _))| *number != i || *last != (i + 1 == sequence.len())) {
                return Err(bad("incomplete or ambiguous interleaved part"));
            }
            parts.insert(name, sequence.into_iter().map(|p| p.2).collect());
        }
        Ok(Self { zip, parts, remaining: MAX_TOTAL, image_bytes: MAX_TOTAL, glyphs_left: 500_000 })
    }

    fn read(&mut self, name: &str) -> Result<Vec<u8>> {
        let indices = self.parts.get(name).ok_or_else(|| bad(format!("{name}: package part not found")))?;
        let limit = MAX_PART.min(self.remaining);
        let mut out = Vec::new();
        let mut chunk = [0u8; 8192];
        for index in indices {
            let mut part = self.zip.by_index(*index).map_err(|e| bad(format!("{name}: {e}")))?;
            if part.size() > limit.saturating_sub(out.len()) as u64 {
                return Err(bad("part exceeds the decode budget"));
            }
            loop {
                let count = part.read(&mut chunk).map_err(|e| bad(e.to_string()))?;
                if count == 0 {
                    break;
                }
                if out.len().checked_add(count).is_none_or(|n| n > limit) {
                    return Err(bad("part exceeds the decode budget"));
                }
                out.try_reserve(count).map_err(|_| bad("not enough memory for this package part"))?;
                out.extend_from_slice(chunk.get(..count).ok_or_else(|| bad("invalid ZIP read size"))?);
            }
        }
        self.remaining = self.remaining.saturating_sub(out.len());
        Ok(out)
    }

    fn text(&mut self, name: &str) -> Result<String> {
        let bytes = self.read(name)?;
        let (encoding, skip) = encoding_rs::Encoding::for_bom(&bytes).unwrap_or((encoding_rs::UTF_8, 0));
        let (text, errors) = encoding.decode_without_bom_handling(bytes.get(skip..).unwrap_or_default());
        if errors {
            return Err(bad(format!("{name}: invalid XML encoding")));
        }
        Ok(text.into_owned())
    }

    fn image_budget(&mut self, width: u32, height: u32) -> Result<()> {
        let bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|n| n.checked_mul(4))
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| bad("image dimensions overflow"))?;
        if bytes > MAX_PART || bytes > self.image_bytes {
            return Err(bad("image pixels exceed the print-job budget"));
        }
        self.image_bytes = self.image_bytes.saturating_sub(bytes);
        Ok(())
    }
}

fn check_directory_budget(bytes: &[u8]) -> Result<()> {
    let start = bytes.len().saturating_sub(65_557);
    let tail = bytes.get(start..).unwrap_or_default();
    let (offset, end) = tail
        .windows(22)
        .enumerate()
        .rev()
        .find(|(i, w)| {
            w.starts_with(b"PK\x05\x06")
                && w.get(20..22)
                    .and_then(|b| <[u8; 2]>::try_from(b).ok())
                    .is_some_and(|b| i.saturating_add(22).saturating_add(usize::from(u16::from_le_bytes(b))) == tail.len())
        })
        .ok_or_else(|| bad("missing ZIP directory footer"))?;
    let word = |at: usize| end.get(at..at.saturating_add(2)).and_then(|b| <[u8; 2]>::try_from(b).ok()).map(u16::from_le_bytes);
    let absolute = start.saturating_add(offset);
    if bytes.get(absolute.saturating_sub(20)..absolute.saturating_sub(16)) == Some(b"PK\x06\x07".as_slice()) || word(10).is_none_or(|n| n > 20_000) {
        return Err(bad("ZIP64 or more than 20000 parts are not supported"));
    }
    if word(4) != Some(0) || word(6) != Some(0) || word(8) != word(10) {
        return Err(bad("multi-disk packages are not supported"));
    }
    let size = end.get(12..16).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_le_bytes).ok_or_else(|| bad("invalid ZIP directory size"))?;
    if size > 16 * 1024 * 1024 {
        return Err(bad("ZIP directory exceeds 16 MiB"));
    }
    Ok(())
}

fn xml(text: &str) -> Result<roxmltree::Document<'_>> {
    roxmltree::Document::parse_with_options(text, roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES })
        .map_err(|e| bad(format!("invalid XML: {e}")))
}

/// Resolve only package-local part names. No network, filesystem extraction or external entities.
fn part_uri(base: &str, target: &str) -> Result<String> {
    if target.is_empty() || target.starts_with("//") || target.contains([':', '\\', '?', '#', '%', '\0']) {
        return Err(bad("unsupported package URI"));
    }
    let mut parts: Vec<&str> =
        if target.starts_with('/') { Vec::new() } else { base.rsplit_once('/').map_or(Vec::new(), |(d, _)| d.split('/').collect()) };
    for p in target.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(bad("package URI escapes its root"));
                }
            }
            _ => parts.push(p),
        }
    }
    if parts.is_empty() {
        return Err(bad("empty package URI"));
    }
    Ok(parts.join("/"))
}

fn required<'a>(node: Node<'a, '_>, name: &str) -> Result<&'a str> {
    node.attribute(name).ok_or_else(|| bad(format!("{} needs {name}", node.tag_name().name())))
}
fn number(text: &str) -> Result<f64> {
    text.parse::<f64>().ok().filter(|n| n.is_finite() && n.abs() <= 1e8).ok_or_else(|| bad("invalid or oversized number"))
}
fn attr(node: Node<'_, '_>, name: &str, default: f64) -> Result<f64> {
    node.attribute(name).map_or(Ok(default), number)
}
fn numbers(text: &str) -> Result<Vec<f64>> {
    if text.len() > 4096 {
        return Err(bad("numeric list is too long"));
    }
    text.split(|c: char| c == ',' || c.is_ascii_whitespace()).filter(|s| !s.is_empty()).map(number).collect()
}

/// Convert an XPS/OpenXPS package with supported text and vector content. Unsupported drawing
/// constructs refuse the conversion; callers receive no partial document or raster fallback.
pub fn from_xps(title: &str, bytes: &[u8]) -> Result<Document> {
    let mut package = Package::new(bytes)?;
    let rels = package.text("_rels/.rels")?;
    let rels = xml(&rels)?;
    let roots: Vec<_> = rels
        .descendants()
        .filter(|n| n.has_tag_name("Relationship") && n.attribute("Type").is_some_and(|t| t.ends_with("/fixedrepresentation")))
        .collect();
    let [root] = roots.as_slice() else { return Err(bad("expected one fixed document sequence")) };
    if root.attribute("TargetMode") == Some("External") {
        return Err(bad("external document sequence is not allowed"));
    }
    let sequence = part_uri("", required(*root, "Target")?)?;
    let data = package.text(&sequence)?;
    let data = xml(&data)?;
    let mut output = Document::new_empty();
    let mut count = 0usize;
    let mut content_bytes = 0usize;
    for reference in data.root_element().children().filter(Node::is_element) {
        if !reference.has_tag_name("DocumentReference") {
            return Err(bad("unsupported document sequence element"));
        }
        let document = part_uri(&sequence, required(reference, "Source")?)?;
        let data = package.text(&document)?;
        let data = xml(&data)?;
        if !data.root_element().has_tag_name("FixedDocument") {
            return Err(bad("expected FixedDocument"));
        }
        for page in data.root_element().children().filter(Node::is_element) {
            if !page.has_tag_name("PageContent") {
                return Err(bad("unsupported fixed document element"));
            }
            count = count.saturating_add(1);
            if count > 1000 {
                return Err(bad("at most 1000 pages may be converted"));
            }
            let name = part_uri(&document, required(page, "Source")?)?;
            let text = package.text(&name)?;
            let tree = xml(&text)?;
            let page = tree.root_element();
            if !page.has_tag_name("FixedPage") {
                return Err(bad("expected FixedPage"));
            }
            let width = number(required(page, "Width")?)?;
            let height = number(required(page, "Height")?)?;
            if !(4.0..=19_200.0).contains(&width) || !(4.0..=19_200.0).contains(&height) {
                return Err(bad("invalid page dimensions"));
            }
            let mut painter = Painter {
                doc: &mut output,
                package: &mut package,
                part: &name,
                fonts: Dict::new(),
                images: Dict::new(),
                profiles: Vec::new(),
                stream: String::new(),
            };
            let _ = writeln!(painter.stream, "q 0.75 0 0 -0.75 0 {} cm", height * 0.75);
            painter.draw(page, 0)?;
            painter.stream.push_str("Q\n");
            content_bytes = content_bytes
                .checked_add(painter.stream.len())
                .filter(|n| *n <= MAX_TOTAL)
                .ok_or_else(|| bad("generated page content exceeds 256 MiB"))?;
            let mut resources = Dict::new();
            resources.set(b"Font".to_vec(), Object::Dict(painter.fonts));
            resources.set(b"XObject".to_vec(), Object::Dict(painter.images));
            crate::add_page(painter.doc, width * 0.75, height * 0.75, resources, Some(painter.stream.into_bytes()))?;
        }
    }
    if count == 0 {
        return Err(bad("no fixed pages"));
    }
    crate::set_title(&mut output, title);
    Ok(output)
}

struct Painter<'a, 'b> {
    doc: &'a mut Document,
    package: &'a mut Package<'b>,
    part: &'a str,
    fonts: Dict,
    images: Dict,
    profiles: Vec<(Vec<u8>, ObjRef)>,
    stream: String,
}

impl Painter<'_, '_> {
    fn draw(&mut self, node: Node<'_, '_>, depth: usize) -> Result<()> {
        if depth > 64 {
            return Err(bad("drawing nesting exceeds 64 levels"));
        }
        let allowed: &[&str] = match node.tag_name().name() {
            "FixedPage" => &["Width", "Height", "ContentBox", "BleedBox"],
            "Glyphs" => &[
                "Fill",
                "FontUri",
                "FontRenderingEmSize",
                "OriginX",
                "OriginY",
                "UnicodeString",
                "Indices",
                "BidiLevel",
                "IsSideways",
                "StyleSimulations",
                "CaretStops",
                "DeviceFontName",
            ],
            "Path" => &[
                "Data",
                "Fill",
                "Stroke",
                "StrokeThickness",
                "StrokeDashArray",
                "StrokeDashOffset",
                "StrokeDashCap",
                "StrokeStartLineCap",
                "StrokeEndLineCap",
                "StrokeLineJoin",
                "StrokeMiterLimit",
            ],
            _ => &[],
        };
        check_attributes(node, allowed)?;
        if node.attribute("OpacityMask").is_some() || attr(node, "Opacity", 1.0)? != 1.0 {
            return Err(bad("opacity groups are not supported yet"));
        }
        self.stream.push_str("q\n");
        let property_name = format!("{}.RenderTransform", node.tag_name().name());
        let properties: Vec<_> = node.children().filter(|n| n.is_element() && n.tag_name().name() == property_name).collect();
        if properties.len() > 1 || (!properties.is_empty() && node.attribute("RenderTransform").is_some()) {
            return Err(bad("duplicate render transform"));
        }
        let property = properties
            .first()
            .copied()
            .map(|p| {
                let mut children = p.children().filter(Node::is_element);
                let matrix = children.next().ok_or_else(|| bad("missing MatrixTransform"))?;
                if !matrix.has_tag_name("MatrixTransform") || children.next().is_some() {
                    return Err(bad("unsupported render transform"));
                }
                required(matrix, "Matrix")
            })
            .transpose()?;
        if let Some(matrix) = node.attribute("RenderTransform").or(property) {
            let values: [f64; 6] = numbers(matrix)?.try_into().map_err(|_| bad("expected six matrix values"))?;
            let [a, b, c, d, e, f] = values;
            let _ = writeln!(self.stream, "{a} {b} {c} {d} {e} {f} cm");
        }
        if let Some(clip) = node.attribute("Clip") {
            let even = path(&mut self.stream, clip)?;
            self.stream.push_str(if even { "W* n\n" } else { "W n\n" });
        }
        match node.tag_name().name() {
            "FixedPage" | "Canvas" => {
                for child in node.children().filter(|n| n.is_element() && n.tag_name().name() != property_name) {
                    self.draw(child, depth + 1)?;
                }
            }
            "Glyphs" => self.glyphs(node)?,
            "Path" => {
                if node.children().any(|n| n.is_element() && n.tag_name().name() != property_name && n.tag_name().name() != "Path.Fill") {
                    return Err(bad("path property elements are not supported yet"));
                }
                let fills: Vec<_> = node.children().filter(|n| n.has_tag_name("Path.Fill")).collect();
                if fills.len() > 1 || (!fills.is_empty() && node.attribute("Fill").is_some()) {
                    return Err(bad("duplicate path fill"));
                }
                let brush = fills
                    .first()
                    .map(|p| {
                        let mut nodes = p.children().filter(Node::is_element);
                        let brush = nodes.next().ok_or_else(|| bad("missing fill brush"))?;
                        if nodes.next().is_some() {
                            return Err(bad("multiple fill brushes"));
                        }
                        Ok(brush)
                    })
                    .transpose()?;
                let fill = node.attribute("Fill");
                let stroke = node.attribute("Stroke");
                if let Some(color) = fill {
                    self.color(color, false)?;
                }
                if let Some(color) = stroke {
                    self.color(color, true)?;
                    let width = attr(node, "StrokeThickness", 1.0)?;
                    if width < 0.0 {
                        return Err(bad("negative stroke width"));
                    }
                    let _ = writeln!(self.stream, "{width} w");
                }
                for unsupported in [
                    "StrokeDashArray",
                    "StrokeDashOffset",
                    "StrokeDashCap",
                    "StrokeStartLineCap",
                    "StrokeEndLineCap",
                    "StrokeLineJoin",
                    "StrokeMiterLimit",
                ] {
                    if node.attribute(unsupported).is_some() {
                        return Err(bad(format!("{unsupported} is not supported yet")));
                    }
                }
                if let Some(brush) = brush {
                    if !brush.has_tag_name("ImageBrush") {
                        return Err(bad("only image brush property fills are supported yet"));
                    }
                    self.stream.push_str("q\n");
                    let even = path(&mut self.stream, required(node, "Data")?)?;
                    self.stream.push_str(if even { "W* n\n" } else { "W n\n" });
                    self.image_brush(brush)?;
                    self.stream.push_str("Q\n");
                }
                let even = path(&mut self.stream, required(node, "Data")?)?;
                self.stream.push_str(match (fill.is_some(), stroke.is_some(), even) {
                    (true, true, true) => "B*\n",
                    (true, true, false) => "B\n",
                    (true, false, true) => "f*\n",
                    (true, false, false) => "f\n",
                    (false, true, _) => "S\n",
                    _ => "n\n",
                });
            }
            name => return Err(bad(format!("unsupported drawing element {name}"))),
        }
        self.stream.push_str("Q\n");
        if self.stream.len() > MAX_PART {
            return Err(bad("page drawing exceeds its budget"));
        }
        Ok(())
    }

    fn image_brush(&mut self, brush: Node<'_, '_>) -> Result<()> {
        check_attributes(
            brush,
            &[
                "ImageSource",
                "Viewport",
                "Viewbox",
                "ViewportUnits",
                "ViewboxUnits",
                "TileMode",
                "Stretch",
                "AlignmentX",
                "AlignmentY",
                "Transform",
                "RelativeTransform",
            ],
        )?;
        if brush.children().any(|n| n.is_element())
            || attr(brush, "Opacity", 1.0)? != 1.0
            || brush.attribute("TileMode").is_some_and(|s| s != "None")
            || brush.attribute("Stretch").is_some_and(|s| s != "Fill")
            || brush.attribute("Transform").is_some()
            || brush.attribute("RelativeTransform").is_some()
            || brush.attribute("ViewportUnits") != Some("Absolute")
            || brush.attribute("ViewboxUnits") != Some("Absolute")
        {
            return Err(bad("image brush tiling, relative units or transforms are not supported yet"));
        }
        let rect = |key: &str| -> Result<[f64; 4]> {
            let value: [f64; 4] = numbers(required(brush, key)?)?.try_into().map_err(|_| bad(format!("{key} needs four values")))?;
            if value.get(2).is_none_or(|v| *v <= 0.0) || value.get(3).is_none_or(|v| *v <= 0.0) {
                return Err(bad("image brush has no area"));
            }
            Ok(value)
        };
        let [x, y, w, h] = rect("Viewport")?;
        let [bx, by, bw, bh] = rect("Viewbox")?;
        let name = part_uri(self.part, required(brush, "ImageSource")?)?;
        let bytes = self.package.read(&name)?;
        let image = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            let reader = png::Decoder::new(Cursor::new(&bytes)).read_info().map_err(|e| bad(e.to_string()))?;
            self.package.image_budget(reader.info().width, reader.info().height)?;
            crate::png_image(&name, &bytes, 96.0)?
        } else if bytes.starts_with(&[0xff, 0xd8]) {
            let image = crate::jpeg(&name, &bytes, 96.0)?;
            self.package.image_budget(image.px.0, image.px.1)?;
            image
        } else {
            return Err(bad("only PNG and JPEG print images are supported yet"));
        };
        let natural_w = f64::from(image.px.0) * 96.0 / image.dpi.0;
        let natural_h = f64::from(image.px.1) * 96.0 / image.dpi.1;
        let width = natural_w * w / bw;
        let height = natural_h * h / bh;
        let left = x - bx * w / bw;
        let top = y - by * h / bh;
        if ![width, height, left, top].iter().all(|v| v.is_finite() && v.abs() <= 1e8) {
            return Err(bad("image brush geometry exceeds its budget"));
        }
        let resource = format!("Im{}", self.images.iter().count().saturating_add(1));
        let image = crate::add_image(self.doc, image, &mut self.profiles);
        self.images.set(resource.as_bytes().to_vec(), Object::Ref(image));
        let bottom = top + height;
        let _ = writeln!(self.stream, "q {x} {y} {w} {h} re W n {width} 0 0 {} {left} {bottom} cm /{resource} Do Q", -height);
        Ok(())
    }

    fn color(&mut self, text: &str, stroke: bool) -> Result<()> {
        let hex = text.strip_prefix('#').ok_or_else(|| bad("only opaque hexadecimal solid colors are supported yet"))?;
        let hex = match hex.len() {
            6 => hex,
            8 if hex.starts_with("FF") || hex.starts_with("ff") => hex.get(2..).ok_or_else(|| bad("invalid color"))?,
            _ => return Err(bad("unsupported color or transparency")),
        };
        let bytes = hex.as_bytes();
        let mut channels = Vec::new();
        for pair in bytes.as_chunks::<2>().0 {
            let pair = std::str::from_utf8(pair).map_err(|_| bad("invalid color"))?;
            channels.push(f64::from(u8::from_str_radix(pair, 16).map_err(|_| bad("invalid color"))?) / 255.0);
        }
        let [r, g, b]: [f64; 3] = channels.try_into().map_err(|_| bad("invalid color"))?;
        let _ = writeln!(self.stream, "{r} {g} {b} {}", if stroke { "RG" } else { "rg" });
        Ok(())
    }

    fn glyphs(&mut self, node: Node<'_, '_>) -> Result<()> {
        if node.children().any(|n| n.is_element() && n.tag_name().name() != "Glyphs.RenderTransform")
            || attr(node, "BidiLevel", 0.0)? != 0.0
            || node.attribute("IsSideways").is_some_and(|s| s != "false")
            || node.attribute("StyleSimulations").is_some_and(|s| s != "None")
        {
            return Err(bad("unsupported glyph run orientation or properties"));
        }
        let size = number(required(node, "FontRenderingEmSize")?)?;
        if !(0.01..=4096.0).contains(&size) {
            return Err(bad("invalid glyph size"));
        }
        let mut x = attr(node, "OriginX", 0.0)?;
        let y = attr(node, "OriginY", 0.0)?;
        let text = required(node, "UnicodeString")?.strip_prefix("{}").unwrap_or(required(node, "UnicodeString")?);
        if text.len() > 65_536 {
            return Err(bad("glyph run is too long"));
        }
        let font_name = part_uri(self.part, required(node, "FontUri")?)?;
        let mut bytes = self.package.read(&font_name)?;
        if font_name.to_ascii_lowercase().ends_with(".odttf") {
            deobfuscate(&font_name, &mut bytes)?;
        }
        let (family, _) = pdfcraft_fonts::embedding::names(&bytes).map_err(bad)?;
        let font = FontRef::new(&bytes).map_err(|e| bad(e.to_string()))?;
        let metrics = font.metrics(Size::unscaled(), LocationRef::default());
        if metrics.units_per_em == 0 {
            return Err(bad("font has no units per em"));
        }
        let glyphs = glyph_indices(node.attribute("Indices").unwrap_or(""), text, &font)?;
        self.package.glyphs_left = self.package.glyphs_left.checked_sub(glyphs.len()).ok_or_else(|| bad("print job exceeds 500000 glyphs"))?;
        let mappings: Vec<_> = glyphs.iter().map(|g| (g.text.clone(), g.id)).collect();
        let resource = pdfcraft_fonts::embedding::embed_mapping(self.doc, &mut self.fonts, &bytes, &family, &mappings).map_err(bad)?;
        self.color(required(node, "Fill")?, false)?;
        let advances = font.glyph_metrics(Size::unscaled(), LocationRef::default());
        for (i, glyph) in glyphs.iter().enumerate() {
            let cid = u16::try_from(i.saturating_add(1)).map_err(|_| bad("too many glyphs"))?;
            let px = x + glyph.u * size / 100.0;
            let py = y - glyph.v * size / 100.0;
            let _ = writeln!(self.stream, "BT /{resource} {size} Tf 1 0 0 -1 {px} {py} Tm <{cid:04X}> Tj ET");
            let advance =
                glyph.advance.unwrap_or_else(|| f64::from(advances.advance_width(glyph.id).unwrap_or(0.0)) / f64::from(metrics.units_per_em) * 100.0);
            x += advance * size / 100.0;
            if !x.is_finite() || x.abs() > 1e8 {
                return Err(bad("glyph positions exceed their budget"));
            }
        }
        Ok(())
    }
}

fn check_attributes(node: Node<'_, '_>, allowed: &[&str]) -> Result<()> {
    for attribute in node.attributes() {
        let common = matches!(attribute.name(), "Name" | "Uid" | "Opacity")
            || node.tag_name().name() != "ImageBrush" && matches!(attribute.name(), "RenderTransform" | "Clip" | "OpacityMask");
        let xml = attribute.namespace() == Some("http://www.w3.org/XML/1998/namespace") && matches!(attribute.name(), "lang" | "space");
        if !xml && !(attribute.namespace().is_none() && (common || allowed.contains(&attribute.name()))) {
            return Err(bad(format!("unsupported {} attribute {}", node.tag_name().name(), attribute.name())));
        }
    }
    Ok(())
}

struct Glyph {
    id: GlyphId,
    text: String,
    advance: Option<f64>,
    u: f64,
    v: f64,
}

fn glyph_indices(indices: &str, text: &str, font: &FontRef<'_>) -> Result<Vec<Glyph>> {
    if indices.len() > 1024 * 1024 {
        return Err(bad("glyph indices exceed their budget"));
    }
    let chars: Vec<_> = text.chars().collect();
    let mut entries: Vec<_> = if indices.is_empty() { vec![""; chars.len()] } else { indices.split(';').collect() };
    if entries.len() == chars.len().saturating_add(1) && entries.last() == Some(&"") {
        entries.pop();
    }
    if entries.len() != chars.len() || entries.len() > 10_000 {
        return Err(bad("complex glyph clusters are not supported yet"));
    }
    let mut out = Vec::new();
    for (entry, ch) in entries.into_iter().zip(chars) {
        if entry.contains(['(', ')']) {
            return Err(bad("complex glyph clusters are not supported yet"));
        }
        let fields: Vec<_> = entry.split(',').collect();
        if fields.len() > 4 {
            return Err(bad("unsupported glyph flags"));
        }
        let id = match fields.first().copied().unwrap_or("") {
            "" => font.charmap().map(ch).ok_or_else(|| bad("character has no font glyph"))?,
            id => GlyphId::new(id.parse::<u16>().map_err(|_| bad("invalid glyph index"))?.into()),
        };
        let field = |i: usize| fields.get(i).filter(|s| !s.is_empty()).map(|s| number(s)).transpose();
        out.push(Glyph { id, text: ch.to_string(), advance: field(1)?, u: field(2)?.unwrap_or(0.0), v: field(3)?.unwrap_or(0.0) });
    }
    Ok(out)
}

fn deobfuscate(name: &str, bytes: &mut [u8]) -> Result<()> {
    let name = name.rsplit('/').next().and_then(|s| s.rsplit_once('.').map(|(stem, _)| stem)).ok_or_else(|| bad("invalid obfuscated font name"))?;
    let hex: String = name.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 || bytes.len() < 32 {
        return Err(bad("invalid obfuscated font"));
    }
    let mut key = Vec::new();
    for pair in hex.as_bytes().as_chunks::<2>().0 {
        key.push(u8::from_str_radix(std::str::from_utf8(pair).map_err(|_| bad("invalid font GUID"))?, 16).map_err(|_| bad("invalid font GUID"))?);
    }
    for (i, byte) in bytes.iter_mut().take(32).enumerate() {
        *byte ^= *key.get(15 - i % 16).ok_or_else(|| bad("invalid font key"))?;
    }
    Ok(())
}

fn path(stream: &mut String, data: &str) -> Result<bool> {
    use svgtypes::PathSegment::*;
    if data.len() > MAX_PART {
        return Err(bad("path data exceeds its budget"));
    }
    let (even, data) = if let Some(s) = data.strip_prefix("F0") {
        (true, s)
    } else if let Some(s) = data.strip_prefix("F1") {
        (false, s)
    } else {
        (true, data)
    };
    let (mut x, mut y) = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    for (count, segment) in svgtypes::PathParser::from(data).enumerate() {
        if count > 100_000 {
            return Err(bad("too many path segments"));
        }
        let point = |abs: bool, px: f64, py: f64| -> Result<(f64, f64)> {
            let p = if abs { (px, py) } else { (px + x, py + y) };
            if ![p.0, p.1].iter().all(|v| v.is_finite() && v.abs() <= 1e8) {
                return Err(bad("invalid path coordinate"));
            }
            Ok(p)
        };
        match segment.map_err(|e| bad(e.to_string()))? {
            MoveTo { abs, x: px, y: py } => {
                (x, y) = point(abs, px, py)?;
                start = (x, y);
                let _ = writeln!(stream, "{x} {y} m");
            }
            LineTo { abs, x: px, y: py } => {
                (x, y) = point(abs, px, py)?;
                let _ = writeln!(stream, "{x} {y} l");
            }
            HorizontalLineTo { abs, x: px } => {
                x = point(abs, px, if abs { y } else { 0.0 })?.0;
                let _ = writeln!(stream, "{x} {y} l");
            }
            VerticalLineTo { abs, y: py } => {
                y = point(abs, if abs { x } else { 0.0 }, py)?.1;
                let _ = writeln!(stream, "{x} {y} l");
            }
            CurveTo { abs, x1, y1, x2, y2, x: px, y: py } => {
                let (a, b) = point(abs, x1, y1)?;
                let (c, d) = point(abs, x2, y2)?;
                let end = point(abs, px, py)?;
                (x, y) = end;
                let _ = writeln!(stream, "{a} {b} {c} {d} {x} {y} c");
            }
            ClosePath { .. } => {
                stream.push_str("h\n");
                (x, y) = start;
            }
            _ => return Err(bad("unsupported vector path command")),
        }
    }
    Ok(even)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    const FONT: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

    fn package(page: &str) -> Vec<u8> {
        package_bytes(page.as_bytes())
    }

    fn package_bytes(page: &[u8]) -> Vec<u8> {
        package_parts(page, &[])
    }

    fn package_parts(page: &[u8], extra: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let parts=[
            ("_rels/.rels",br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Type="http://schemas.microsoft.com/xps/2005/06/fixedrepresentation" Target="Fixed.fdseq" Id="r1"/></Relationships>"#.as_slice()),
            ("Fixed.fdseq",br#"<FixedDocumentSequence xmlns="http://schemas.microsoft.com/xps/2005/06"><DocumentReference Source="Documents/1.fdoc"/></FixedDocumentSequence>"#.as_slice()),
            ("Documents/1.fdoc",br#"<FixedDocument xmlns="http://schemas.microsoft.com/xps/2005/06"><PageContent Source="Pages/1.fpage"/></FixedDocument>"#.as_slice()),
            ("Documents/Pages/1.fpage",page),
            ("Resources/font.ttf",FONT),
        ];
        for (name, data) in parts {
            zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        for (name, data) in extra {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn image_brush_keeps_source_pixels_and_uses_xps_96_dpi_default() {
        let mut image = Vec::new();
        let mut encoder = png::Encoder::new(&mut image, 2, 2);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]).unwrap();
        let page=br##"<FixedPage Width="816" Height="1056"><Path Data="M96,96L192,96 192,192 96,192Z"><Path.Fill><ImageBrush ImageSource="/Resources/picture.png" Viewport="96,96,96,96" Viewbox="0,0,2,2" ViewportUnits="Absolute" ViewboxUnits="Absolute"/></Path.Fill></Path></FixedPage>"##;
        let doc = from_xps("Image print", &package_parts(page, &[("Resources/picture.png", &image)])).unwrap();
        let images: Vec<_> =
            doc.scan_objects().filter(|(_, o)| o.as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Image".as_slice()))).collect();
        assert_eq!(images.len(), 1);
        let image = images[0].1.as_dict().unwrap();
        assert_eq!(image.get(b"Width").and_then(Object::as_int), Some(2));
        assert_eq!(image.get(b"Height").and_then(Object::as_int), Some(2));
        let page = pdfcraft_model::pages(&doc).remove(0);
        let stream = doc.resolve(page.dict.get(b"Contents").unwrap());
        let Object::Stream(stream) = stream.as_ref() else { panic!("page stream") };
        let text = String::from_utf8(stream.decoded().unwrap()).unwrap();
        assert!(text.contains("96 0 0 -96 96 192 cm"), "{text}");
        let fixture = package("<FixedPage Width=\"816\" Height=\"1056\"/>");
        let mut package = Package::new(&fixture).unwrap();
        assert!(package.image_budget(u32::MAX, u32::MAX).is_err());
    }

    #[test]
    fn word_utf16_fixed_pages_are_supported() {
        let page = "<FixedPage Width=\"816\" Height=\"1056\"/>";
        let bytes: Vec<u8> = [0xff, 0xfe].into_iter().chain(page.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert!(from_xps("Word UTF16", &package_bytes(&bytes)).is_ok());
    }

    #[test]
    fn interleaved_print_parts_are_reassembled_and_malformed_sequences_refused() {
        fn pieces(parts: &[(&str, &[u8])]) -> Vec<u8> {
            let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for (name, data) in parts {
                zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap().into_inner()
        }
        let bytes = pieces(&[("part/[2].last.piece", b"c"), ("part/[0].piece", b"a"), ("part/[1].piece", b"b")]);
        assert_eq!(Package::new(&bytes).unwrap().read("part").unwrap(), b"abc");
        for parts in [
            vec![("part/[0].piece", b"a".as_slice())],
            vec![("part/[1].last.piece", b"a".as_slice())],
            vec![("part/[0].last.piece", b"a".as_slice()), ("part/[1].last.piece", b"b")],
            vec![("part", b"direct".as_slice()), ("part/[0].last.piece", b"a")],
            vec![("part/[00].last.piece", b"a".as_slice())],
        ] {
            let bytes = pieces(&parts);
            assert!(Package::new(&bytes).and_then(|mut p| p.read("part")).is_err());
        }
        let original = package("<FixedPage Width=\"816\" Height=\"1056\"/>");
        let mut source = zip::ZipArchive::new(Cursor::new(original)).unwrap();
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for i in 0..source.len() {
            let mut part = source.by_index(i).unwrap();
            let name = part.name().unwrap().to_string();
            let mut data = Vec::new();
            part.read_to_end(&mut data).unwrap();
            for (n, chunk) in data.chunks(127).enumerate() {
                out.start_file(format!("{name}/[{n}].piece"), zip::write::SimpleFileOptions::default()).unwrap();
                out.write_all(chunk).unwrap();
            }
            out.start_file(format!("{name}/[{}].last.piece", data.len().div_ceil(127)), zip::write::SimpleFileOptions::default()).unwrap();
        }
        assert!(from_xps("Native interleaved print", &out.finish().unwrap().into_inner()).is_ok());
    }

    #[test]
    fn xps_creates_real_text_unicode_font_and_vector_paths() {
        let page = r##"<FixedPage xmlns="http://schemas.microsoft.com/xps/2005/06" Width="816" Height="1056"><Canvas RenderTransform="1,0,0,1,12,20"><Glyphs Fill="#000000" FontUri="/Resources/font.ttf" FontRenderingEmSize="20" OriginX="72" OriginY="96" UnicodeString="Highlight these words"/><Path Data="M 50,120 L 250,120 250,160 50,160 Z" Fill="#008080"/></Canvas></FixedPage>"##;
        let doc = from_xps("Synthetic print", &package(page)).unwrap();
        let bytes = pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
        let doc = Document::open(bytes.into()).unwrap();
        let pages = pdfcraft_model::pages(&doc);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].crop(&doc), [0.0, 0.0, 612.0, 792.0]);
        let resources = doc.resolve(pages[0].dict.get(b"Resources").unwrap());
        assert!(resources.as_dict().unwrap().get(b"Font").is_some(), "embedded Unicode text font");
    }

    #[test]
    fn unsupported_content_and_external_parts_are_refused_without_raster_fallback() {
        for content in [
            "<Image/>",
            "<Canvas UnrecognizedVisual=\"changed\"/>",
            "<Canvas Opacity=\"0.5\"/>",
            "<Path Fill=\"{StaticResource missing}\" Data=\"M0,0L1,1\"/>",
        ] {
            let page = format!("<FixedPage Width=\"816\" Height=\"1056\">{content}</FixedPage>");
            assert!(from_xps("Unsupported", &package(&page)).is_err());
        }
        for target in ["https://example.com/font.ttf", "../../secret.ttf", "//host/font.ttf", "font%2Fsecret.ttf", "C:\\font.ttf"] {
            assert!(part_uri("page.fpage", target).is_err(), "{target}");
        }
        assert!(xml("<!DOCTYPE a [<!ENTITY b SYSTEM 'file:///secret'>]><a>&b;</a>").is_err());
        assert!(from_xps("Bad", b"not a ZIP").is_err());
        let mut oversized = package("<FixedPage Width=\"816\" Height=\"1056\"/>");
        let end = oversized.len() - 22;
        oversized[end + 10..end + 12].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(from_xps("Bad count", &oversized).err().unwrap().to_string().contains("20000"));
    }

    #[test]
    fn font_obfuscation_round_trip_and_path_coordinates() {
        let mut bytes = FONT.to_vec();
        let original = bytes.clone();
        let name = "Resources/00112233-4455-6677-8899-AABBCCDDEEFF.odttf";
        deobfuscate(name, &mut bytes).unwrap();
        assert_ne!(bytes, original);
        deobfuscate(name, &mut bytes).unwrap();
        assert_eq!(bytes, original);
        let mut out = String::new();
        assert!(!path(&mut out, "F1 M 10,20 l 30,40 h 5 v -10 Z").unwrap());
        assert!(out.contains("40 60 l\n45 60 l\n45 50 l\nh\n"), "{out}");
        assert!(path(&mut out, "M0,0A10,10,0,0,0,30,40").is_err());
    }
}
