//! Open an EPUB as a PDF (#493, #2620): the book's text laid out on pages of the region's paper
//! size, in its own fonts where it brings them, with its pictures, tables, links, contents (as
//! bookmarks), title, authors and language. Fixed-layout books become a page per content document.
//!
//! No browser engine: blocks and the CSS a book's layout needs are read here (`dom`, `css`) and
//! set by our own layout (`layout`, `text`). What isn't converted is said in the notes (the
//! document's repair log): scripts, MathML, audio and video, CSS floats, columns and grids,
//! vertical writing, SVG text. A book protected by DRM is refused.

mod css;
mod dom;
mod entities;
mod layout;
mod package;
mod text;
mod zip;

use std::collections::{BTreeSet, HashMap};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};

use crate::{LETTER, MAX_SIDE, add_page};
use layout::{Item, Layout, Page};
use text::{FaceId, Fonts};

/// ISO A4 in points.
pub const A4: (f64, f64) = (595.276, 841.89);

/// Why a book can't be converted.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum EpubError {
    /// DRM: encryption other than font obfuscation, or a DRM scheme's rights file.
    #[error("this book is protected by digital rights management (DRM), so it can't be converted to PDF")]
    Protected,
    #[error("the book can't be read: {0}")]
    Damaged(String),
    #[error("the book is too large to convert: {0}")]
    TooLarge(String),
}

/// A converted book: the PDF and what the conversion left out or chose (for the repair log).
#[derive(Debug)]
pub struct Converted {
    pub document: Document,
    pub notes: Vec<String>,
}

/// The paper a reflowed book is set on for a region (ISO 3166 code): US Letter in the United
/// States, Puerto Rico, Canada and Mexico, A4 everywhere else.
pub fn paper_for_region(region: &str) -> (f64, f64) {
    match region.to_ascii_uppercase().as_str() {
        "US" | "PR" | "CA" | "MX" => LETTER,
        _ => A4,
    }
}

/// The region of a locale tag (`en_US.UTF-8`, `es-PR`, `zh-Hant-TW`); `None` without one.
pub fn region_of(tag: &str) -> Option<String> {
    let tag = tag.split(['.', '@']).next().unwrap_or_default();
    tag.split(['-', '_']).skip(1).filter(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_alphabetic())).last().map(str::to_ascii_uppercase)
}

/// Whether `bytes` are an EPUB: a ZIP whose first entry is the stored `mimetype`
/// `application/epub+zip` (OCF), or any ZIP with `META-INF/container.xml` named `.epub`.
pub fn is_epub(name: &str, bytes: &[u8]) -> bool {
    if bytes.starts_with(b"PK\x03\x04") && bytes.get(30..58) == Some(b"mimetypeapplication/epub+zip".as_slice()) {
        return true;
    }
    name.to_ascii_lowercase().ends_with(".epub") && zip::Zip::open(bytes).is_ok_and(|z| z.has("META-INF/container.xml"))
}

/// A picture to embed: a file name (for its format), its bytes (SVG drawn as PNG) and its natural
/// size in points.
type Picture = (String, Vec<u8>, (f64, f64));

/// A content document ready to lay out: its path, whether it is fixed-layout, its viewport (CSS
/// pixels) and its blocks.
type Chapter = (String, bool, Option<(f64, f64)>, Vec<dom::Block>);

/// Images by container path.
struct Pictures<'a> {
    zip: &'a zip::Zip<'a>,
    cache: HashMap<String, Option<Picture>>,
    notes: BTreeSet<String>,
}

impl Pictures<'_> {
    fn get(&mut self, path: &str) -> Option<&Picture> {
        if !self.cache.contains_key(path) {
            let loaded = self.load(path);
            self.cache.insert(path.to_string(), loaded);
        }
        self.cache.get(path).and_then(Option::as_ref)
    }

    fn load(&mut self, path: &str) -> Option<Picture> {
        let bytes = self.zip.read(path).ok().flatten()?;
        if path.to_ascii_lowercase().ends_with(".svg") || bytes.windows(4).take(512).any(|w| w == b"<svg") {
            let opts = svg_options();
            let tree = resvg::usvg::Tree::from_data(&bytes, &opts).ok()?;
            let size = tree.size();
            let (w, h) = (f64::from(size.width()), f64::from(size.height()));
            // Drawn at twice its size (at most 4096 px a side), as a picture.
            let scale = (2.0f64).min(4096.0 / w.max(h).max(1.0));
            let (pw, ph) = ((w * scale).ceil().max(1.0) as u32, (h * scale).ceil().max(1.0) as u32);
            let mut pixmap = resvg::tiny_skia::Pixmap::new(pw, ph)?;
            resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale as f32, scale as f32), &mut pixmap.as_mut());
            if bytes.windows(5).any(|w| w == b"<text") {
                self.notes.insert(format!("{path}: text inside an SVG picture was left out"));
            }
            let png = pixmap.encode_png().ok()?;
            return Some(("picture.png".into(), png, (w * dom::PT_PER_PX, h * dom::PT_PER_PX)));
        }
        // PNG's size is in its header; the other formats through their decoders.
        let (w, h) = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            let be = |at: usize| bytes.get(at..at + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_be_bytes);
            (be(16)?, be(20)?)
        } else {
            image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().ok()?.into_dimensions().ok()?
        };
        if w == 0 || h == 0 {
            return None;
        }
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        Some((name, bytes, (f64::from(w) * dom::PT_PER_PX, f64::from(h) * dom::PT_PER_PX)))
    }
}

/// Options for a picture's SVG: an `<image>` that names a file is left out, never read from disk
/// (the default resolver reads any path it is given, so a book could embed a local file or hang on
/// a device file). Pictures inside the SVG as `data:` URLs still draw.
fn svg_options() -> resvg::usvg::Options<'static> {
    let mut opts = resvg::usvg::Options::default();
    opts.image_href_resolver.resolve_string = Box::new(|_, _| None);
    opts
}

/// Text of a content document that isn't well-formed XHTML: its tags dropped, a paragraph per
/// block-level tag, so its words still reach the PDF.
fn loose_blocks(text: &str, path: &str) -> Vec<dom::Block> {
    let mut paras = Vec::new();
    let mut current = String::new();
    let mut rest = text;
    let body = rest.find("<body").unwrap_or(0);
    rest = &rest[body..];
    while let Some(lt) = rest.find('<') {
        current.push_str(&rest[..lt]);
        // The tag runs to its `>`, or to the end of the text when it is never closed ('<' is one
        // byte, so `lt + 1` is a char boundary, and so are `lt + g` and the end).
        let (tag_end, gt) = rest[lt..].find('>').map_or((rest.len(), rest.len()), |g| (lt + g, lt + g + 1));
        let tag = rest.get(lt + 1..tag_end).unwrap_or_default().trim_start_matches('/').to_ascii_lowercase();
        let name = tag.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or_default();
        if matches!(name, "p" | "div" | "br" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "tr" | "blockquote") {
            paras.push(std::mem::take(&mut current));
        }
        rest = &rest[gt.min(rest.len())..];
    }
    current.push_str(rest);
    paras.push(current);
    let _ = path;
    paras
        .into_iter()
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .map(|p| {
            let style = dom::Style { margin: [6.0, 0.0, 6.0, 0.0], ..dom::Style::default() };
            dom::Block::Text {
                runs: vec![dom::Run { text: decode_entities(&p), style: style.run.clone(), link: None }],
                style,
                anchors: Vec::new(),
                heading: None,
            }
        })
        .flat_map(|b| [dom::Block::Gap(6.0), b])
        .collect()
}

fn decode_entities(s: &str) -> String {
    let xml = package::xml_text(s.as_bytes());
    // One pass, left to right: a reference is decoded once, so `&amp;lt;` stays `&lt;`.
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml.as_str();
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        // A reference is short; look no further than its first dozen bytes for the `;`.
        let decoded = tail.bytes().take(12).position(|b| b == b';').and_then(|e| entity(&tail[..e]).map(|c| (c, e)));
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &tail[e + 1..];
            }
            None => {
                out.push('&');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The character a reference stands for (the text between `&` and `;`): decimal or hex, or one of
/// XML's five names.
fn entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse::<u32>().ok()?,
        };
        return char::from_u32(code);
    }
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

/// Convert an EPUB to PDF; reflowed text is set on `paper` (points). `fallback_title` names the
/// document when the book has no title.
pub fn from_epub(bytes: &[u8], paper: (f64, f64), fallback_title: &str) -> Result<Converted, EpubError> {
    let zip = zip::Zip::open(bytes)?;
    let pkg = package::read(&zip)?;
    let paper = (paper.0.clamp(144.0, MAX_SIDE), paper.1.clamp(144.0, MAX_SIDE));
    let mut notes: Vec<String> = Vec::new();
    let mut fonts = Fonts::new();
    let mut loaded_fonts: HashMap<String, bool> = HashMap::new();
    let mut pictures = Pictures { zip: &zip, cache: HashMap::new(), notes: BTreeSet::new() };
    let mut sheet_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut flags = (false, false, false);
    // Each content document's blocks (or fixed page), in spine order.
    let mut docs: Vec<Chapter> = Vec::new();
    for item in &pkg.spine {
        let Some(raw) = zip.read(&item.path)? else {
            notes.push(format!("{}: listed in the spine but missing from the book", item.path));
            continue;
        };
        if pkg.media.get(&item.path).is_some_and(|m| m == "image/svg+xml") {
            let block =
                dom::Block::Image { path: item.path.clone(), alt: String::new(), style: dom::Style::default(), anchors: vec![item.path.clone()] };
            docs.push((item.path.clone(), true, None, vec![block]));
            continue;
        }
        let text = package::xml_text(&raw);
        let lower = text.to_ascii_lowercase();
        flags.0 |= lower.contains("<script");
        flags.1 |= lower.contains("<math") || lower.contains(":math");
        flags.2 |= lower.contains("<audio") || lower.contains("<video");
        let xml = match package::parse(&text) {
            Ok(x) => x,
            Err(e) => {
                notes.push(format!("{}: not well-formed XHTML ({e}); its text was kept without its formatting", item.path));
                let mut blocks = vec![dom::Block::Anchor(vec![item.path.clone()])];
                blocks.extend(loose_blocks(&text, &item.path));
                docs.push((item.path.clone(), false, None, blocks));
                continue;
            }
        };
        let mut read_sheet = |p: &str| -> Option<String> {
            sheet_cache.entry(p.to_string()).or_insert_with(|| zip.read(p).ok().flatten().map(|b| crate::decode_text(&b))).clone()
        };
        let sheets = dom::sheets(&xml, &item.path, &mut read_sheet);
        for face in sheets.iter().flat_map(|s| &s.faces) {
            // The first source that embeds wins; the rest are fallbacks the book never needs. A
            // note is made only when none of them embeds.
            let mut embedded = false;
            let mut failed: Vec<&str> = Vec::new();
            for src in &face.sources {
                let key = format!("{src}|{}|{}|{}", face.family, face.bold, face.italic);
                if let Some(&done) = loaded_fonts.get(&key) {
                    if done {
                        embedded = true;
                        break;
                    }
                    continue;
                }
                let mut ok = false;
                if let Ok(Some(mut font)) = zip.read(src) {
                    if let Some(kind) = pkg.obfuscated.get(src) {
                        package::deobfuscate(&mut font, *kind, &pkg.identifier);
                    }
                    ok = fonts.add_book(src, font, &face.family, face.bold, face.italic);
                }
                loaded_fonts.insert(key, ok);
                if ok {
                    embedded = true;
                    break;
                }
                failed.push(src);
            }
            if !embedded {
                for src in failed {
                    notes.push(format!("{src}: the book's font {} can't be embedded; a standard face stands in for it", face.family));
                }
            }
        }
        let cascade = dom::Cascade::new(sheets);
        let mut builder = dom::Builder::new(&cascade, &item.path);
        if let Some(body) = xml.descendants().find(|n| n.tag_name().name() == "body") {
            builder.node(body);
        } else {
            builder.node(xml.root_element());
        }
        let (mut blocks, mut more) = builder.finish();
        notes.append(&mut more);
        blocks.insert(0, dom::Block::Anchor(vec![item.path.clone()]));
        docs.push((item.path.clone(), item.fixed, dom::viewport(&xml), blocks));
    }
    if flags.0 {
        notes.push("the book's scripts were not run".into());
    }
    if flags.1 {
        notes.push("mathematics (MathML) was left out".into());
    }
    if flags.2 {
        notes.push("audio and video were left out".into());
    }

    // A fixed page without a viewport (an SVG page, a page that forgot it): its picture's size.
    for (_, fixed, viewport, blocks) in &mut docs {
        if *fixed && viewport.is_none() {
            let picture = blocks.iter().find_map(|b| match b {
                dom::Block::Image { path, .. } => Some(path.clone()),
                _ => None,
            });
            *viewport = picture.and_then(|p| pictures.get(&p).map(|x| (x.2.0 / dom::PT_PER_PX, x.2.1 / dom::PT_PER_PX)));
        }
    }
    let mut size = |p: &str| pictures.get(p).map(|x| x.2);
    let mut layout = Layout::new(&mut fonts, paper, &mut size);
    let mut reflowed = false;
    for (path, fixed, viewport, blocks) in &docs {
        if *fixed {
            let vp = viewport.unwrap_or((paper.0 / dom::PT_PER_PX, paper.1 / dom::PT_PER_PX));
            layout.fixed(blocks, vp, path);
        } else {
            reflowed = true;
            // Each content document starts a page, as reading systems start a chapter.
            layout.new_page();
            layout.blocks(blocks);
        }
    }
    if layout.pages.is_empty() {
        layout.new_page();
    }
    let Layout { pages, anchors, notes: mut layout_notes, .. } = layout;
    notes.append(&mut layout_notes);
    notes.extend(pictures.notes.iter().cloned());
    if reflowed {
        let name = if paper == LETTER {
            "US Letter"
        } else if paper == A4 {
            "A4"
        } else {
            "custom"
        };
        notes.insert(0, format!("the book's text was laid out on {name} paper ({:.0} × {:.0} pt), the default for this region", paper.0, paper.1));
    }
    if !fonts.missing.is_empty() {
        let n: usize = fonts.missing.values().sum();
        let mut chars: Vec<&char> = fonts.missing.keys().collect();
        chars.sort();
        let sample: Vec<String> = chars.iter().take(5).map(|c| format!("U+{:04X}", u32::from(**c))).collect();
        notes.push(format!("{n} characters no available font can draw were left out ({})", sample.join(", ")));
    }
    let title = pkg.title.clone().unwrap_or_else(|| fallback_title.to_string());
    let document = write(&pages, &anchors, &fonts, &mut pictures, &pkg, &title, &mut notes)?;
    Ok(Converted { document, notes })
}

fn num(x: f64) -> String {
    let s = format!("{x:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".into() } else { s.to_string() }
}

/// Which CID each (glyph, text) of a program has, in the order `embed_mapping` assigns them.
#[derive(Default)]
struct Mapping {
    order: Vec<(String, skrifa::GlyphId)>,
    index: HashMap<(u32, String), u16>,
}

impl Mapping {
    fn cid(&mut self, gid: u32, text: &str) -> Option<u16> {
        if let Some(c) = self.index.get(&(gid, text.to_string())) {
            return Some(*c);
        }
        if self.order.len() >= 65_534 {
            return None;
        }
        self.order.push((text.to_string(), skrifa::GlyphId::new(gid)));
        let cid = self.order.len() as u16;
        self.index.insert((gid, text.to_string()), cid);
        Some(cid)
    }
}

/// The picture at `path` as a resource name and object, embedded the first time it is placed.
fn image_entry(
    doc: &mut Document,
    image_names: &mut HashMap<String, (String, ObjRef)>,
    pictures: &mut Pictures,
    path: &str,
    notes: &mut Vec<String>,
) -> Option<(String, ObjRef)> {
    match image_names.get(path) {
        Some(e) => Some(e.clone()),
        None => match pictures.get(path).cloned() {
            Some((name, bytes, _)) => match crate::image_xobject(doc, &name, &bytes) {
                Ok((r, _)) => {
                    let e = (format!("Im{}", image_names.len() + 1), r);
                    image_names.insert(path.to_string(), e.clone());
                    Some(e)
                }
                Err(e) => {
                    notes.push(format!("{path}: the picture could not be embedded ({e})"));
                    None
                }
            },
            None => None,
        },
    }
}

/// One run of glyphs in a face, size and colour, as text operators on the page's content.
fn glyph_run(
    c: &mut Vec<u8>,
    resource: &str,
    style: (FaceId, f64, [f64; 3]),
    glyphs: &[(text::Glyph, [f64; 2])],
    maps: &mut HashMap<usize, Mapping>,
    fonts: &Fonts,
) {
    let (face, size, color) = style;
    c.extend(format!("BT {} {} {} rg /{resource} {} Tf\n", num(color[0]), num(color[1]), num(color[2]), num(size)).bytes());
    // TJ runs: a new Tm where a glyph leaves the line, adjustments where shaping
    // moved it from where the font's own advance would put it.
    let mut pen: Option<(f64, f64)> = None;
    let mut tj = String::new();
    let flush = |tj: &mut String, c: &mut Vec<u8>| {
        if !tj.is_empty() {
            c.extend(format!("[{tj}] TJ\n").bytes());
            tj.clear();
        }
    };
    for (g, [x, y]) in glyphs {
        let code = match face {
            FaceId::Std(_) => format!("{:02X}", g.id),
            FaceId::Program(i) => match maps.entry(i).or_default().cid(g.id, &g.text) {
                Some(cid) => format!("{cid:04X}"),
                None => continue,
            },
        };
        match pen {
            Some((px, py)) if (py - y).abs() < 0.001 => {
                let delta = x - px;
                if delta.abs() > 0.0005 {
                    tj.push_str(&format!(" {} ", num(-delta * 1000.0 / size)));
                }
            }
            _ => {
                flush(&mut tj, c);
                c.extend(format!("1 0 0 1 {} {} Tm\n", num(*x), num(*y)).bytes());
            }
        }
        tj.push('<');
        tj.push_str(&code);
        tj.push('>');
        pen = Some((x + fonts.font_advance(face, g.id) * size, *y));
    }
    flush(&mut tj, c);
    c.extend_from_slice(b"ET\n");
}

/// The pages' shared font dictionary: the standard faces, and the book's own faces embedded for the
/// glyphs used; the resource name each program face got.
fn embed_fonts(
    doc: &mut Document,
    std_names: &HashMap<pdfcraft_fonts::Std14, String>,
    maps: HashMap<usize, Mapping>,
    fonts: &Fonts,
    notes: &mut Vec<String>,
) -> (ObjRef, HashMap<usize, String>) {
    let mut font_dict = Dict::new();
    for (s, name) in std_names {
        let mut f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("Font"));
        f.set(b"Subtype".to_vec(), Object::name("Type1"));
        f.set(b"BaseFont".to_vec(), Object::name(s.base_font()));
        f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        let r = doc.add(f);
        font_dict.set(name.as_bytes().to_vec(), Object::Ref(r));
    }
    let mut program_names: HashMap<usize, String> = HashMap::new();
    let mut order: Vec<_> = maps.into_iter().collect();
    order.sort_by_key(|(i, _)| *i);
    for (i, map) in order {
        let p = &fonts.programs[i];
        let family: String = p.family.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
        let family = if family.is_empty() { format!("BookFont{i}") } else { family };
        match pdfcraft_fonts::embedding::embed_mapping(doc, &mut font_dict, &p.bytes, &family, &map.order) {
            Ok(name) => {
                program_names.insert(i, name);
            }
            Err(e) => notes.push(format!("the font {} could not be embedded ({e}); its text is missing", p.family)),
        }
    }
    (doc.add(font_dict), program_names)
}

fn write(
    pages: &[Page],
    anchors: &layout::Anchors,
    fonts: &Fonts,
    pictures: &mut Pictures,
    pkg: &package::Package,
    title: &str,
    notes: &mut Vec<String>,
) -> Result<Document, EpubError> {
    let mut doc = Document::new_empty();
    // Content first (CIDs assigned as glyphs are written), fonts embedded after.
    let mut maps: HashMap<usize, Mapping> = HashMap::new();
    let mut std_names: HashMap<pdfcraft_fonts::Std14, String> = HashMap::new();
    let mut image_names: HashMap<String, (String, ObjRef)> = HashMap::new();
    let mut contents: Vec<(Vec<u8>, Vec<String>)> = Vec::with_capacity(pages.len());
    for page in pages {
        let mut c = Vec::new();
        let mut used_images = Vec::new();
        for item in &page.items {
            match item {
                Item::Fill { rect, color } => {
                    c.extend(
                        format!(
                            "{} {} {} rg {} {} {} {} re f\n",
                            num(color[0]),
                            num(color[1]),
                            num(color[2]),
                            num(rect[0]),
                            num(rect[1]),
                            num(rect[2]),
                            num(rect[3])
                        )
                        .bytes(),
                    );
                }
                Item::Stroke { rect, width, color } => {
                    c.extend(
                        format!(
                            "{} {} {} RG {} w {} {} {} {} re S\n",
                            num(color[0]),
                            num(color[1]),
                            num(color[2]),
                            num(*width),
                            num(rect[0]),
                            num(rect[1]),
                            num(rect[2]),
                            num(rect[3])
                        )
                        .bytes(),
                    );
                }
                Item::Image { path, rect } => {
                    if let Some((name, _)) = image_entry(&mut doc, &mut image_names, pictures, path, notes) {
                        c.extend(format!("q {} 0 0 {} {} {} cm /{name} Do Q\n", num(rect[2]), num(rect[3]), num(rect[0]), num(rect[1])).bytes());
                        if !used_images.contains(&name) {
                            used_images.push(name);
                        }
                    }
                }
                Item::Glyphs { face, size, color, glyphs } => {
                    let resource = match face {
                        FaceId::Std(s) => {
                            let n = std_names.len();
                            std_names.entry(*s).or_insert_with(|| format!("S{n}")).clone()
                        }
                        FaceId::Program(i) => format!("@P{i}"),
                    };
                    glyph_run(&mut c, &resource, (*face, *size, *color), glyphs, &mut maps, fonts);
                }
            }
        }
        contents.push((c, used_images));
    }
    let (fonts_ref, program_names) = embed_fonts(&mut doc, &std_names, maps, fonts, notes);
    // Pages.
    let mut page_refs = Vec::with_capacity(pages.len());
    for (page, (mut c, images)) in pages.iter().zip(contents) {
        // The program resources' names are known now.
        for (i, name) in &program_names {
            let from = format!("/@P{i} ");
            let to = format!("/{name} ");
            if c.windows(from.len()).any(|w| w == from.as_bytes()) {
                c = String::from_utf8_lossy(&c).replace(&from, &to).into_bytes();
            }
        }
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Ref(fonts_ref));
        if !images.is_empty() {
            let mut x = Dict::new();
            for name in &images {
                if let Some((_, r)) = image_names.values().find(|(n, _)| n == name) {
                    x.set(name.as_bytes().to_vec(), Object::Ref(*r));
                }
            }
            res.set(b"XObject".to_vec(), Object::Dict(x));
        }
        let (w, h) = (page.size.0.clamp(3.0, MAX_SIDE), page.size.1.clamp(3.0, MAX_SIDE));
        let r = add_page(&mut doc, w, h, res, Some(c)).map_err(|e| EpubError::Damaged(e.to_string()))?;
        page_refs.push(r);
    }
    // A target (container path with an optional fragment) as an explicit destination.
    let dest = |target: &str| -> Option<Object> {
        let (page, y) = anchors.get(target).or_else(|| anchors.get(target.split('#').next().unwrap_or(target)))?;
        let r = page_refs.get(*page)?;
        Some(Object::Array(vec![Object::Ref(*r), Object::name("XYZ"), Object::Null, Object::Real((*y + 2.0).min(pages[*page].size.1)), Object::Null]))
    };
    // Links.
    for (i, page) in pages.iter().enumerate() {
        let mut annots = Vec::new();
        for (rect, target) in &page.links {
            let mut a = Dict::new();
            a.set(b"Type".to_vec(), Object::name("Annot"));
            a.set(b"Subtype".to_vec(), Object::name("Link"));
            a.set(b"Rect".to_vec(), Object::Array(rect.iter().map(|v| Object::Real(*v)).collect()));
            a.set(b"Border".to_vec(), Object::Array(vec![0.into(), 0.into(), 0.into()]));
            if target.contains("://") || target.starts_with("mailto:") {
                let mut action = Dict::new();
                action.set(b"S".to_vec(), Object::name("URI"));
                action.set(b"URI".to_vec(), Object::String(PdfString::literal(target.as_bytes().to_vec())));
                a.set(b"A".to_vec(), Object::Dict(action));
            } else if let Some(d) = dest(target) {
                a.set(b"Dest".to_vec(), d);
            } else {
                continue;
            }
            annots.push(Object::Ref(doc.add(a)));
        }
        if !annots.is_empty()
            && let Some(r) = page_refs.get(i)
        {
            let _ = doc.update_dict(*r, |d| d.set(b"Annots".to_vec(), Object::Array(annots)));
        }
    }
    // Bookmarks from the table of contents.
    let entries: Vec<(&package::NavEntry, Object)> = pkg.toc.iter().filter_map(|e| dest(&e.target).map(|d| (e, d))).collect();
    let has_outline = !entries.is_empty();
    if has_outline {
        let root = doc.add(Dict::new());
        let refs: Vec<ObjRef> = entries.iter().map(|_| doc.add(Dict::new())).collect();
        // Parent of each entry: the nearest earlier entry of a lower level.
        let mut parents: Vec<Option<usize>> = Vec::with_capacity(entries.len());
        for (i, (e, _)) in entries.iter().enumerate() {
            parents.push((0..i).rev().find(|&j| entries[j].0.level < e.level));
        }
        let children = |p: Option<usize>| -> Vec<usize> { (0..entries.len()).filter(|&i| parents[i] == p).collect() };
        let set_kids = |doc: &mut Document, parent: ObjRef, kids: &[usize]| {
            if let (Some(first), Some(last)) = (kids.first(), kids.last()) {
                let _ = doc.update_dict(parent, |d| {
                    d.set(b"First".to_vec(), Object::Ref(refs[*first]));
                    d.set(b"Last".to_vec(), Object::Ref(refs[*last]));
                    d.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
                });
            }
        };
        let top = children(None);
        let _ = doc.update_dict(root, |d| d.set(b"Type".to_vec(), Object::name("Outlines")));
        set_kids(&mut doc, root, &top);
        for (i, (e, d)) in entries.iter().enumerate() {
            let parent = parents[i].map_or(root, |p| refs[p]);
            let siblings = children(parents[i]);
            let pos = siblings.iter().position(|&s| s == i).unwrap_or(0);
            let mut item = Dict::new();
            item.set(b"Title".to_vec(), PdfString::text(&e.title));
            item.set(b"Parent".to_vec(), Object::Ref(parent));
            item.set(b"Dest".to_vec(), d.clone());
            if pos > 0 {
                item.set(b"Prev".to_vec(), Object::Ref(refs[siblings[pos - 1]]));
            }
            if let Some(next) = siblings.get(pos + 1) {
                item.set(b"Next".to_vec(), Object::Ref(refs[*next]));
            }
            doc.set(refs[i], Object::Dict(item));
            let kids = children(Some(i));
            set_kids(&mut doc, refs[i], &kids);
            if !kids.is_empty() {
                // Closed: a negative count.
                let _ = doc.update_dict(refs[i], |d| d.set(b"Count".to_vec(), Object::Int(-(kids.len() as i64))));
            }
        }
        if let Some(cat) = doc.root() {
            let _ = doc.update_dict(cat, |d| d.set(b"Outlines".to_vec(), Object::Ref(root)));
        }
    }
    // Metadata.
    let mut info = Dict::new();
    info.set(b"Title".to_vec(), PdfString::text(title));
    if !pkg.creators.is_empty() {
        info.set(b"Author".to_vec(), PdfString::text(&pkg.creators.join("; ")));
    }
    info.set(b"Producer".to_vec(), PdfString::text("PdfCraft"));
    info.set(b"Creator".to_vec(), PdfString::text("EPUB"));
    let info_ref = doc.add(info);
    doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(info_ref));
    let xmp = xmp(title, &pkg.creators, pkg.language.as_deref());
    let mut md = Dict::new();
    md.set(b"Type".to_vec(), Object::name("Metadata"));
    md.set(b"Subtype".to_vec(), Object::name("XML"));
    let md_ref = doc.add(Object::Stream(Stream::from_raw(md, xmp.into_bytes())));
    if let Some(cat) = doc.root() {
        let lang = pkg.language.clone();
        let rtl = pkg.rtl;
        let _ = doc.update_dict(cat, |d| {
            d.set(b"Metadata".to_vec(), Object::Ref(md_ref));
            if let Some(l) = &lang {
                d.set(b"Lang".to_vec(), PdfString::text(l));
            }
            if rtl {
                let mut vp = Dict::new();
                vp.set(b"Direction".to_vec(), Object::name("R2L"));
                d.set(b"ViewerPreferences".to_vec(), Object::Dict(vp));
            }
            if has_outline {
                d.set(b"PageMode".to_vec(), Object::name("UseOutlines"));
            }
        });
    }
    Ok(doc)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// XMP with the title, creators and language (dc:), as the Info dictionary has them.
fn xmp(title: &str, creators: &[String], language: Option<&str>) -> String {
    let mut s = String::from(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\">",
    );
    s.push_str(&format!("<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:title>", xml_escape(title)));
    if !creators.is_empty() {
        s.push_str("<dc:creator><rdf:Seq>");
        for c in creators {
            s.push_str(&format!("<rdf:li>{}</rdf:li>", xml_escape(c)));
        }
        s.push_str("</rdf:Seq></dc:creator>");
    }
    if let Some(l) = language {
        s.push_str(&format!("<dc:language><rdf:Bag><rdf:li>{}</rdf:li></rdf:Bag></dc:language>", xml_escape(l)));
    }
    s.push_str("<pdf:Producer>PdfCraft</pdf:Producer></rdf:Description></rdf:RDF></x:xmpmeta>\n<?xpacket end=\"w\"?>");
    s
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_font_sources_first_wins;
#[cfg(test)]
mod tests_image_percent_size;
#[cfg(test)]
mod tests_limits;
#[cfg(test)]
mod tests_line_height_percent;
#[cfg(test)]
mod tests_loose_entities;
