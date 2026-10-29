#[cfg(test)]
use super::core::inspect;
use super::core::{Inspector, load_options};
use super::model::{Attachment, AttachmentSource, FontInfo, Layer, PageTransition, TransitionStyle};
use super::navigation::NameTreeSeen;
use super::{AssertUnwindSafe, Dictionary, DocInfo, Document, HashSet, Object, ObjectId, catch_unwind};

/// Decoded bytes read from an XFA form's configuration packet (or a whole-XDP `/XFA` stream) to
/// find `dynamicRender`. Designer's packets are a few kilobytes; a whole XDP can hold the template.
const MAX_XFA_CONFIG_BYTES: usize = 16 << 20;

/// At most this many layers are read from all of `/RBGroups` together.
const MAX_LAYER_GROUP_ENTRIES: usize = 4096;

impl<'a> Inspector<'a> {
    // ── object helpers ──────────────────────────────────────────────────────────────────────

    /// `/Trans` and `/Dur` of every page (ISO 32000-2 §12.4.4), in page order. Lenient: a value
    /// outside the spec's choices takes that entry's default, and a page with neither entry is
    /// not listed. Bounded by the page count.
    pub(super) fn transitions(&self, info: &mut DocInfo) {
        for (page, &pid) in self.page_ids.iter().enumerate() {
            let Ok(d) = self.doc.get_dictionary(pid) else { continue };
            let display = d.get(b"Dur").ok().and_then(|o| self.resolve(o).as_float().ok()).filter(|s| s.is_finite() && *s >= 0.0);
            let trans = d.get(b"Trans").ok().and_then(|o| self.dict(o));
            if trans.is_none() && display.is_none() {
                continue;
            }
            let mut t = PageTransition {
                page,
                style: None,
                duration: 1.0,
                horizontal: true,
                inward: true,
                direction: Some(0),
                scale: 1.0,
                opaque: false,
                display,
            };
            if let Some(tr) = trans {
                t.style = Some(self.name(tr, b"S").map_or(TransitionStyle::Replace, |n| TransitionStyle::from_name(&n)));
                let num = |key: &[u8]| tr.get(key).ok().and_then(|o| self.resolve(o).as_float().ok()).filter(|v| v.is_finite());
                t.duration = num(b"D").filter(|d| *d >= 0.0).unwrap_or(1.0);
                t.horizontal = self.name(tr, b"Dm").as_deref() != Some("V");
                t.inward = self.name(tr, b"M").as_deref() != Some("O");
                t.direction = match tr.get(b"Di").map(|o| self.resolve(o)) {
                    Ok(Object::Name(n)) if n.as_slice() == b"None" => None,
                    Ok(o) => match o.as_i64().ok() {
                        Some(0) => Some(0),
                        Some(90) => Some(90),
                        Some(180) => Some(180),
                        Some(270) => Some(270),
                        Some(315) => Some(315),
                        _ => Some(0),
                    },
                    Err(_) => Some(0),
                };
                t.scale = num(b"SS").filter(|s| *s > 0.0).unwrap_or(1.0);
                t.opaque = matches!(tr.get(b"B").map(|o| self.resolve(o)), Ok(Object::Boolean(true)));
            }
            info.transitions.push(t);
        }
    }

    /// The value of `dynamicRender` in an XFA form's configuration packet (`config`, under
    /// `acrobat/acrobat7`): `required` for a dynamic form, `forbidden` for a static one. `/XFA` is
    /// either one stream holding the whole XDP or an array of packet names and streams (ISO
    /// 32000-1 §12.7.8). Lenient: `None` when the packet is missing, unreadable or over
    /// `MAX_XFA_CONFIG_BYTES` decoded, or has no such element.
    pub(super) fn xfa_dynamic_render(&self, xfa: &'a Object) -> Option<String> {
        let stream = match self.resolve(xfa) {
            Object::Array(items) => {
                let at = items.iter().position(|o| matches!(self.resolve(o), Object::String(name, _) if name.as_slice() == b"config"))?;
                self.resolve(items.get(at.checked_add(1)?)?)
            }
            whole => whole,
        };
        let stream = stream.as_stream().ok()?;
        let bytes = if stream.dict.get(b"Filter").is_ok() {
            stream.decompressed_content_with_limit(MAX_XFA_CONFIG_BYTES).ok()?
        } else {
            stream.content.get(..stream.content.len().min(MAX_XFA_CONFIG_BYTES))?.to_vec()
        };
        xfa_config_dynamic_render(&xfa_packet_text(&bytes))
    }

    // ── optional content ────────────────────────────────────────────────────────────────────

    pub(super) fn layers(&self, catalog: &Dictionary, out: &mut Vec<Layer>) {
        let Some(props) = catalog.get(b"OCProperties").ok().and_then(|o| self.dict(o)) else { return };
        let config = props.get(b"D").ok().and_then(|o| self.dict(o));
        let off: HashSet<ObjectId> = config
            .and_then(|c| c.get(b"OFF").ok())
            .and_then(|o| self.resolve(o).as_array().ok())
            .map(|a| a.iter().filter_map(|x| x.as_reference().ok()).collect())
            .unwrap_or_default();
        let base_off = config.and_then(|c| self.name(c, b"BaseState")).as_deref() == Some("OFF");
        let on: HashSet<ObjectId> = config
            .and_then(|c| c.get(b"ON").ok())
            .and_then(|o| self.resolve(o).as_array().ok())
            .map(|a| a.iter().filter_map(|x| x.as_reference().ok()).collect())
            .unwrap_or_default();
        let Ok(Object::Array(ocgs)) = props.get(b"OCGs").map(|o| self.resolve(o)) else { return };
        for g in ocgs {
            let Some(d) = self.dict(g) else { continue };
            let id = g.as_reference().ok();
            let visible = match id {
                Some(id) if off.contains(&id) => false,
                Some(id) if on.contains(&id) => true,
                _ => !base_off,
            };
            let Some(id) = id else { continue };
            out.push(Layer { id, name: self.text(d, b"Name").unwrap_or_else(|| "Layer".into()), visible, initial: visible });
        }
    }

    /// The default configuration's radio-button groups (`/RBGroups`) of two or more layers.
    pub(super) fn layer_groups(&self, catalog: &Dictionary) -> Vec<Vec<ObjectId>> {
        let groups = catalog
            .get(b"OCProperties")
            .ok()
            .and_then(|o| self.dict(o))
            .and_then(|p| p.get(b"D").ok())
            .and_then(|o| self.dict(o))
            .and_then(|c| c.get(b"RBGroups").ok())
            .and_then(|o| self.resolve(o).as_array().ok());
        let mut left = MAX_LAYER_GROUP_ENTRIES;
        let mut out = Vec::new();
        for g in groups.into_iter().flatten() {
            let Ok(members) = self.resolve(g).as_array() else { continue };
            let group: Vec<ObjectId> = members.iter().filter_map(|x| x.as_reference().ok()).take(left).collect();
            left = left.saturating_sub(group.len());
            if group.len() > 1 {
                out.push(group);
            }
            if left == 0 {
                break;
            }
        }
        out
    }

    pub(super) fn fonts(&self, out: &mut Vec<FontInfo>) {
        let mut seen = HashSet::new();
        for &pid in &self.page_ids {
            let Ok(fonts) = self.doc.get_page_fonts(pid) else { continue };
            for (_, f) in fonts {
                let base = f
                    .get(b"BaseFont")
                    .ok()
                    .and_then(|o| o.as_name().ok())
                    .map(|n| String::from_utf8_lossy(n).into_owned())
                    .unwrap_or_else(|| "(unnamed)".into());
                let kind = self.name(f, b"Subtype").unwrap_or_default();
                let encoding = match f.get(b"Encoding").map(|o| self.resolve(o)) {
                    Ok(Object::Name(n)) => Some(String::from_utf8_lossy(n).into_owned()),
                    Ok(Object::Dictionary(_)) => Some("Custom".into()),
                    Ok(Object::Stream(_)) => Some("Embedded CMap".into()),
                    _ => None,
                };
                // Embedded: FontFile/2/3 in the descriptor (for Type0, in the descendant font).
                let descriptor_of = |d: &'a Dictionary| d.get(b"FontDescriptor").ok().and_then(|o| self.dict(o));
                let descendant =
                    f.get(b"DescendantFonts").ok().and_then(|o| self.resolve(o).as_array().ok()).and_then(|a| a.first()).and_then(|o| self.dict(o));
                let desc = descriptor_of(f).or_else(|| descendant.and_then(descriptor_of));
                let embedded =
                    kind == "Type3" || desc.is_some_and(|d| d.get(b"FontFile").is_ok() || d.get(b"FontFile2").is_ok() || d.get(b"FontFile3").is_ok());
                // What the substitution chain needs beyond the name: the descriptor flags and
                // weight, and for a composite font the script its descendant declares (ISO 32000-2
                // 9.7.3). The renderer asks the chain with the same, so this names the face drawn.
                let number = |key: &[u8]| {
                    desc.and_then(|d| d.get(key).ok()).and_then(|o| self.resolve(o).as_i64().ok()).and_then(|n| u32::try_from(n).ok()).unwrap_or(0)
                };
                let (flags, weight) = (number(b"Flags"), number(b"FontWeight"));
                let ordering =
                    descendant.and_then(|d| d.get(b"CIDSystemInfo").ok()).and_then(|o| self.dict(o)).and_then(|c| self.text(c, b"Ordering"));
                let subset = base.len() > 7 && base.as_bytes()[6] == b'+' && base[..6].bytes().all(|b| b.is_ascii_uppercase());
                let name = if subset { base[7..].to_string() } else { base };
                let chosen = pdfcraft_fonts::fallback::substitute(&pdfcraft_fonts::fallback::Request {
                    base_font: &name,
                    embedded,
                    flags,
                    ordering: ordering.as_deref(),
                    weight,
                });
                let info = FontInfo {
                    name,
                    kind,
                    embedded,
                    subset,
                    encoding,
                    substitute: chosen.face_name(),
                    substitution: (!chosen.why.is_faithful()).then(|| chosen.why.note().to_owned()),
                };
                if seen.insert((info.name.clone(), info.kind.clone(), info.embedded)) {
                    out.push(info);
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// The attachment's name is the file specification's, else `fallback`'s, else a generic one.
    pub(super) fn attachment(
        &self,
        spec: &Object,
        source: AttachmentSource,
        text_budget: &mut usize,
        fallback: impl FnOnce(&mut usize) -> Option<String>,
    ) -> Attachment {
        let d = self.dict(spec);
        let size =
            d.and_then(|d| d.get(b"EF").ok()).and_then(|o| self.dict(o)).and_then(|ef| ef.get(b"F").ok().or_else(|| ef.get(b"UF").ok())).and_then(
                |f| match self.resolve(f) {
                    Object::Stream(s) => s
                        .dict
                        .get(b"Params")
                        .ok()
                        .and_then(|p| self.dict(p))
                        .and_then(|p| p.get(b"Size").ok())
                        .and_then(|s| s.as_i64().ok())
                        .map(|s| s as usize)
                        .or(Some(s.content.len())),
                    _ => None,
                },
            );
        let display = d
            .and_then(|d| self.budgeted_text(d, b"UF", text_budget).or_else(|| self.budgeted_text(d, b"F", text_budget)))
            .or_else(|| fallback(text_budget))
            .unwrap_or_else(|| "attachment".into());
        Attachment { name: display, description: d.and_then(|d| self.budgeted_text(d, b"Desc", text_budget)), size, source }
    }
}

/// Fetch an attachment's bytes (decoded). Capped at 1 GiB to defuse decompression bombs.
pub fn attachment_data(bytes: &[u8], password: Option<&str>, att: &Attachment) -> Result<Vec<u8>, String> {
    let run = || -> Result<Vec<u8>, String> {
        let options = load_options(password);
        let doc = Document::load_mem_with_options(bytes, options).map_err(|e| e.to_string())?;
        let insp = Inspector::new(&doc);
        let spec: &Object = match &att.source {
            AttachmentSource::Document { key } => {
                let names = doc.catalog().ok().and_then(|c| c.get(b"Names").ok()).and_then(|o| insp.dict(o)).ok_or("no /Names")?;
                let ef = names.get(b"EmbeddedFiles").ok().and_then(|o| insp.dict(o)).ok_or("no /EmbeddedFiles")?;
                insp.name_tree_find(ef, key, &mut NameTreeSeen::default(), 0).ok_or("attachment not found")?
            }
            AttachmentSource::Annotation { page, index } => {
                let (&pid, _) = insp.page_index.iter().find(|(_, i)| **i == *page).ok_or("page not found")?;
                let annots = doc.get_dictionary(pid).map_err(|e| e.to_string())?.get(b"Annots").map_err(|e| e.to_string())?;
                let a = insp.resolve(annots).as_array().map_err(|e| e.to_string())?.get(*index).ok_or("annotation not found")?;
                insp.dict(a).and_then(|d| d.get(b"FS").ok()).ok_or("annotation has no file")?
            }
        };
        let fs = insp.dict(spec).ok_or("bad file specification")?;
        let ef = fs.get(b"EF").ok().and_then(|o| insp.dict(o)).ok_or("the file is not embedded (external reference)")?;
        let stream = ef.get(b"UF").ok().or_else(|| ef.get(b"F").ok()).map(|o| insp.resolve(o)).ok_or("no embedded stream")?;
        let stream = stream.as_stream().map_err(|e| e.to_string())?;
        if stream.dict.get(b"Filter").is_ok() {
            stream.decompressed_content_with_limit(1 << 30).map_err(|e| e.to_string())
        } else {
            Ok(stream.content.clone())
        }
    };
    catch_unwind(AssertUnwindSafe(run)).unwrap_or_else(|p| Err(format!("reading the attachment crashed: {}", crate::raster::panic_message(&p))))
}

/// An XFA packet's bytes as text: UTF-16 (little- or big-endian, told by a byte order mark or by
/// how `<` is written) or UTF-8, as `pdfcraft_xfa::packets::decode` reads them. Lossy: only one
/// element is looked for.
pub(super) fn xfa_packet_text(bytes: &[u8]) -> String {
    let utf16 = |b: &[u8], from: fn([u8; 2]) -> u16| {
        let units = b.as_chunks::<2>().0.iter().map(|c| from(*c));
        char::decode_utf16(units).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)).collect::<String>()
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [b'<', 0, ..] => utf16(bytes, u16::from_le_bytes),
        [0, b'<', ..] => utf16(bytes, u16::from_be_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// One markup tag found by [`next_tag`]: where it starts (`<`) and ends (after `>`), whether it
/// closes (`</x>`) or closes itself (`<x/>`), and its name without a namespace prefix.
struct Tag<'a> {
    start: usize,
    end: usize,
    close: bool,
    empty: bool,
    local: &'a str,
}

/// The next element tag in `xml` at or after `from`, skipping comments, CDATA sections,
/// processing instructions and declarations. A byte scan, not an XML parser: enough to find one
/// known element in an XFA packet, whose tags Designer writes with a line break before each `>`.
fn next_tag(xml: &str, mut from: usize) -> Option<Tag<'_>> {
    loop {
        let lt = from + xml.get(from..)?.find('<')?;
        let rest = xml.get(lt..)?;
        let skip_to = |end: &str| rest.find(end).map(|i| lt + i + end.len());
        if rest.starts_with("<!--") {
            from = skip_to("-->")?;
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            from = skip_to("]]>")?;
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            from = skip_to(">")?;
            continue;
        }
        let gt = lt + rest.find('>')?;
        let inner = xml.get(lt + 1..gt)?;
        let (close, inner) = match inner.strip_prefix('/') {
            Some(i) => (true, i),
            None => (false, inner),
        };
        let empty = !close && inner.ends_with('/');
        let name = inner.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or_default();
        let local = name.rsplit(':').next().unwrap_or(name);
        return Some(Tag { start: lt, end: gt + 1, close, empty, local });
    }
}

/// What `<name>…</name>` holds: the content of the first top-level element of `xml` called `name`
/// (any prefix). Elements nested in other elements are not looked at. `None` when there is none,
/// it closes itself, or it is never closed.
fn xml_child_body<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let mut at = 0;
    // The elements open around `at`, innermost last: a closing tag must name the innermost one,
    // or the markup is malformed and has no value.
    let mut open_tags: Vec<&str> = Vec::new();
    // Where the wanted element's content starts, once its opening tag is seen at the top level.
    let mut open = None;
    loop {
        let tag = next_tag(xml, at)?;
        at = tag.end;
        if tag.close {
            if open_tags.pop()? != tag.local {
                return None;
            }
            if open_tags.is_empty()
                && let Some(start) = open
            {
                return xml.get(start..tag.start);
            }
        } else if !tag.empty {
            if open_tags.is_empty() && tag.local == name {
                open = Some(tag.end);
            }
            open_tags.push(tag.local);
        } else if open_tags.is_empty() && tag.local == name {
            return None;
        }
    }
}

/// The trimmed text at `path` (element names from the outside in, any prefix, each a child of the
/// one before) in `xml`, if it is plain text.
fn xml_path_text(xml: &str, path: &[&str]) -> Option<String> {
    let body = path.iter().try_fold(xml, |within, name| xml_child_body(within, name))?;
    let text = body.trim();
    (!text.is_empty() && !text.contains('<')).then(|| text.to_owned())
}

/// `dynamicRender` at `config/acrobat/acrobat7` in the configuration packet: the `config` child
/// of the `xdp` root in a whole-XDP stream, or the `config` packet itself. The same names inside
/// the form data or the template are not the configuration.
pub(super) fn xfa_config_dynamic_render(packet: &str) -> Option<String> {
    const IN_CONFIG: [&str; 4] = ["config", "acrobat", "acrobat7", "dynamicRender"];
    xml_path_text(packet, &["xdp", IN_CONFIG[0], IN_CONFIG[1], IN_CONFIG[2], IN_CONFIG[3]]).or_else(|| xml_path_text(packet, &IN_CONFIG))
}

#[cfg(test)]
mod substitution_tests {
    use std::sync::Arc;

    use super::{FontInfo, inspect};

    /// One page naming four fonts the document does not embed: a Chinese CID font (Adobe-GB1), a
    /// Japanese one (Adobe-Japan1), an unknown serif-flagged simple font, and an aliased
    /// standard-14 name.
    const NOT_EMBEDDED: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 4 0 R /F2 6 0 R /F3 8 0 R /F4 9 0 R /F5 11 0 R >> >> >> endobj
4 0 obj << /Type /Font /Subtype /Type0 /BaseFont /STSong-Light /Encoding /UniGB-UCS2-H /DescendantFonts [5 0 R] >> endobj
5 0 obj << /Type /Font /Subtype /CIDFontType0 /BaseFont /STSong-Light /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 2 >> >> endobj
6 0 obj << /Type /Font /Subtype /Type0 /BaseFont /KozMinPro-Regular /Encoding /UniJIS-UCS2-H /DescendantFonts [7 0 R] >> endobj
7 0 obj << /Type /Font /Subtype /CIDFontType0 /BaseFont /KozMinPro-Regular /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 4 >> >> endobj
8 0 obj << /Type /Font /Subtype /TrueType /BaseFont /SomeHouseSerif /FontDescriptor 10 0 R >> endobj
9 0 obj << /Type /Font /Subtype /TrueType /BaseFont /ArialMT >> endobj
10 0 obj << /Type /FontDescriptor /FontName /SomeHouseSerif /Flags 34 >> endobj
11 0 obj << /Type /Font /Subtype /TrueType /BaseFont /HouseGrotesk /FontDescriptor 12 0 R >> endobj
12 0 obj << /Type /FontDescriptor /FontName /HouseGrotesk /Flags 33 >> endobj
trailer << /Size 13 /Root 1 0 R >>
%%EOF";

    fn font<'a>(fonts: &'a [FontInfo], name: &str) -> &'a FontInfo {
        fonts.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("{name} listed; got {:?}", fonts.iter().map(|f| &f.name).collect::<Vec<_>>()))
    }

    #[test]
    fn doc_info_says_which_face_stands_in_and_why() {
        let info = inspect(Arc::new(NOT_EMBEDDED.to_vec()), None).expect("opens");

        // The one that matters most to a user: a Chinese PDF. Before this, doc_info said only
        // `embedded: false` and left them to guess. With craft-fonts its Simplified Chinese face
        // stands in, the face the renderer draws with (#5329); without, nothing is drawn, and
        // the report says why.
        let cjk = font(&info.fonts, "STSong-Light");
        assert!(!cjk.embedded);
        let why = cjk.substitution.as_deref().unwrap_or_default();
        match pdfcraft_fonts::ui_chinese_fonts().first() {
            Some(face) => {
                assert_eq!(cjk.substitute, Some(face.name()), "the Simplified Chinese face stands in");
                assert!(why.contains("the script the font declares"), "{why}");
            }
            None => {
                assert_eq!(cjk.substitute, None, "no licensed Chinese face, so nothing stands in");
                assert!(why.contains("no licensed face") && why.contains("rather than the wrong glyphs"), "{why}");
            }
        }

        // An aliased standard-14 name resolves to the real face, whose metrics are exact.
        let arial = font(&info.fonts, "ArialMT");
        assert_eq!(arial.substitute.as_deref(), Some("Helvetica"));
        assert!(arial.substitution.as_deref().unwrap_or_default().contains("standard 14"));

        // A house font whose name merely contains "Serif" gets Times -- reported as the guess it
        // is, not as a standard-14 name, which would be untrue of a font nobody has heard of.
        let serif = font(&info.fonts, "SomeHouseSerif");
        assert_eq!(serif.substitute.as_deref(), Some("Times-Roman"));
        assert!(serif.substitution.as_deref().unwrap_or_default().contains("hints at one"), "{serif:?}");

        // No hint in the name at all, so the descriptor flags decide: 33 = fixed pitch + nonsymbolic.
        let grotesk = font(&info.fonts, "HouseGrotesk");
        assert_eq!(grotesk.substitute.as_deref(), Some("Courier"));
        assert!(grotesk.substitution.as_deref().unwrap_or_default().contains("descriptor flags"), "{grotesk:?}");

        // Japanese depends on the optional craft-fonts build input; either way it is reported,
        // and never as a Latin face.
        let jpan = font(&info.fonts, "KozMinPro-Regular");
        assert!(jpan.substitution.is_some(), "a substitution is always explained");
        match &jpan.substitute {
            Some(face) => {
                assert!(!face.starts_with("Helvetica") && !face.starts_with("Times"), "a Latin face is never a Japanese substitute: {face}")
            }
            None => assert!(jpan.substitution.as_deref().unwrap_or_default().contains("no licensed face")),
        }

        // Nothing claims to be embedded, so every font here carries a reason.
        assert!(info.fonts.iter().all(|f| !f.embedded && f.substitution.is_some()), "{:?}", info.fonts);
    }

    /// An embedded font is reported as faithful: nothing stands in, and there is nothing to explain.
    const EMBEDDED: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 4 0 R >> >> >> endobj
4 0 obj << /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+SomeHouseSans /FontDescriptor 5 0 R >> endobj
5 0 obj << /Type /FontDescriptor /FontName /ABCDEF+SomeHouseSans /Flags 32 /FontFile2 6 0 R >> endobj
6 0 obj << /Length 4 >> stream
test
endstream endobj
trailer << /Size 7 /Root 1 0 R >>
%%EOF";

    #[test]
    fn an_embedded_font_reports_no_substitution_at_all() {
        let info = inspect(Arc::new(EMBEDDED.to_vec()), None).expect("opens");
        let f = font(&info.fonts, "SomeHouseSans");
        assert!(f.embedded && f.subset, "{f:?}");
        assert_eq!((f.substitute.as_deref(), f.substitution.as_deref()), (None, None), "the document's own glyphs are shown");
    }
}
