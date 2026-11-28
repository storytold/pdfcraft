//! Synthetic documents of Type 0 fonts shown through `Identity-H`, for the tests of the document
//! survey and of the rules that read it: every font program is built here (no font file, no
//! corpus file), so the tests run with `CRAFT_FONTS_DIR` unset.

use std::fmt::Write as _;

/// A font's embedded program.
#[derive(Clone)]
pub(super) enum Program {
    /// A TrueType program (`/FontFile2`) of this many glyphs (the glyphs shown are drawn).
    TrueType(u16),
    /// A CID-keyed CFF program (`/FontFile3 /CIDFontType0C`) with these CIDs.
    Cff(Vec<u16>),
    /// A TrueType program of this many glyphs whose `post` table has this version.
    TrueTypePost(u16, u32),
    /// These bytes as `/FontFile3 /CIDFontType0C` (under a `CIDFontType0` descendant).
    Raw(Vec<u8>),
    /// No embedded program.
    None,
}

/// One Type 0 font of a [`Doc`].
#[derive(Clone)]
pub(super) struct Font {
    /// `/BaseFont`, as written (subset tag included).
    pub(super) base: String,
    pub(super) program: Program,
    /// A `/ToUnicode` stream mapping every code to `X`.
    pub(super) to_unicode: bool,
    /// A `/ToUnicode` name (`Identity-H`) in place of a stream.
    pub(super) to_unicode_name: Option<&'static str>,
    /// A `/CIDToGIDMap` stream: CID `i` draws glyph `map[i]`.
    pub(super) cid_to_gid: Option<Vec<u16>>,
    /// The descendant's `/CIDSystemInfo` ordering (`Identity` unless set).
    pub(super) ordering: &'static str,
    /// The encoding CMap's name.
    pub(super) encoding: &'static str,
    /// Write the Type 0 dictionary of font `i` (an earlier one) instead, byte for byte: a second
    /// object of the same font.
    pub(super) alias_of: Option<usize>,
}

impl Font {
    /// A TrueType subset of `glyphs` glyphs named `base`, without `/ToUnicode`.
    pub(super) fn truetype(base: &str, glyphs: u16) -> Self {
        Self {
            base: base.into(),
            program: Program::TrueType(glyphs),
            to_unicode: false,
            to_unicode_name: None,
            cid_to_gid: None,
            ordering: "Identity",
            encoding: "Identity-H",
            alias_of: None,
        }
    }

    /// A CID-keyed CFF subset with `cids`, named `base`, without `/ToUnicode`.
    pub(super) fn cff(base: &str, cids: Vec<u16>) -> Self {
        Self { program: Program::Cff(cids), ..Self::truetype(base, 1) }
    }
}

/// What a page (or a form it draws) shows.
#[derive(Clone)]
pub(super) enum Show {
    /// Codes in font `font` (its index in [`Doc::fonts`]).
    Text(usize, Vec<u16>),
    /// A form XObject showing these, drawn `times` times.
    Form(Vec<Show>, usize),
    /// Content operators as written (`/F0 10 Tf q /F1 10 Tf Q`).
    Raw(&'static str),
}

/// A document of [`Font`]s and pages of [`Show`]s.
#[derive(Clone, Default)]
pub(super) struct Doc {
    pub(super) fonts: Vec<Font>,
    pub(super) pages: Vec<Vec<Show>>,
}

/// How [`Doc::build_as`] writes the content streams.
#[derive(Clone, Copy, Default)]
pub(super) struct Streams {
    /// Pages' and forms' content Flate-compressed (`/Filter /FlateDecode`).
    pub(super) flate: bool,
    /// Each [`Show`] of a page in a stream of its own (`/Contents` an array).
    pub(super) split: bool,
    /// Pages' and forms' content after this many spaces, written in ASCIIHex and that
    /// Flate-compressed (`/Filter [/FlateDecode /ASCIIHexDecode]`): the hex is twice the
    /// content's size, and nothing this large is held in memory.
    pub(super) hex_after: Option<usize>,
    /// With `flate`, the content in PNG rows of this many bytes (`/Predictor 12`, each row
    /// tagged None, the last one padded with spaces): the inflated data is longer than the
    /// content by a byte a row.
    pub(super) png_columns: Option<usize>,
}

/// `content` as a stream's dictionary entries (after `dict`) and data, as `streams` says.
fn content_stream(dict: String, content: String, streams: Streams) -> (String, Option<Vec<u8>>) {
    use std::io::Write as _;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    if let Some(spaces) = streams.hex_after {
        let chunk = "20".repeat(1 << 16);
        for _ in 0..spaces >> 16 {
            z.write_all(chunk.as_bytes()).expect("compress in memory");
        }
        z.write_all("20".repeat(spaces & 0xFFFF).as_bytes()).expect("compress in memory");
        let hex: String = content.bytes().map(|b| format!("{b:02X}")).collect();
        z.write_all(hex.as_bytes()).expect("compress in memory");
        z.write_all(b">").expect("compress in memory");
        return (format!("{dict} /Filter [/FlateDecode /ASCIIHexDecode]"), Some(z.finish().expect("compress in memory")));
    }
    if !streams.flate {
        return (dict, Some(content.into_bytes()));
    }
    if let Some(columns) = streams.png_columns.filter(|c| *c > 0) {
        for row in content.as_bytes().chunks(columns) {
            z.write_all(&[0]).expect("compress in memory");
            z.write_all(row).expect("compress in memory");
            z.write_all(&vec![b' '; columns - row.len()]).expect("compress in memory");
        }
        let parms = format!(" /DecodeParms << /Predictor 12 /Columns {columns} >>");
        return (format!("{dict} /Filter /FlateDecode{parms}"), Some(z.finish().expect("compress in memory")));
    }
    z.write_all(content.as_bytes()).expect("compress in memory");
    (format!("{dict} /Filter /FlateDecode"), Some(z.finish().expect("compress in memory")))
}

impl Doc {
    pub(super) fn build(&self) -> Vec<u8> {
        self.build_as(Streams::default())
    }

    /// The document with its content streams written as `streams` says.
    pub(super) fn build_as(&self, streams: Streams) -> Vec<u8> {
        let mut objects: Vec<(String, Option<Vec<u8>>)> = Vec::new();
        // 1 catalog, 2 pages; fonts from 3 (five objects each), then forms and pages.
        objects.push(("<< /Type /Catalog /Pages 2 0 R >>".into(), None));
        objects.push((String::new(), None));
        let mut font_refs = String::new();
        for (i, font) in self.fonts.iter().enumerate() {
            let at = objects.len() + 1;
            let (type0, cidfont, descriptor, program, extra) = (at, at + 1, at + 2, at + 3, at + 4);
            let tu = match (font.to_unicode, font.to_unicode_name) {
                (true, _) => format!(" /ToUnicode {extra} 0 R"),
                (false, Some(name)) => format!(" /ToUnicode /{name}"),
                (false, None) => String::new(),
            };
            let type0_dict = format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /{} /Encoding /{} /DescendantFonts [{cidfont} 0 R]{tu} >>",
                font.base, font.encoding
            );
            let type0_dict = match font.alias_of.and_then(|j| objects.get(2 + 5 * j)) {
                Some((original, _)) => original.clone(),
                None => type0_dict,
            };
            objects.push((type0_dict, None));
            let (subtype, file_key) = match font.program {
                Program::Cff(_) | Program::Raw(_) => ("CIDFontType0", "/FontFile3"),
                _ => ("CIDFontType2", "/FontFile2"),
            };
            let c2g = match &font.cid_to_gid {
                Some(_) => format!(" /CIDToGIDMap {extra} 0 R"),
                None if subtype == "CIDFontType2" => " /CIDToGIDMap /Identity".into(),
                None => String::new(),
            };
            objects.push((
                format!(
                    "<< /Type /Font /Subtype /{subtype} /BaseFont /{} /CIDSystemInfo << /Registry (Adobe) /Ordering ({}) /Supplement 0 >>{c2g} /DW 1000 /FontDescriptor {descriptor} 0 R >>",
                    font.base, font.ordering
                ),
                None,
            ));
            let file = match font.program {
                Program::None => String::new(),
                _ => format!(" {file_key} {program} 0 R"),
            };
            objects.push((
                format!(
                    "<< /Type /FontDescriptor /FontName /{} /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80{file} >>",
                    font.base
                ),
                None,
            ));
            let program_stream = match &font.program {
                Program::TrueType(glyphs) => {
                    let drawn = self.codes_of(i).into_iter().map(|c| font.glyph_of(c)).filter(|g| g < glyphs).collect::<Vec<_>>();
                    ("".into(), pdfcraft_testkit::truetype::subset(*glyphs, &drawn, pdfcraft_testkit::truetype::Cmap::None, None))
                }
                Program::Cff(cids) => ("/Subtype /CIDFontType0C".into(), pdfcraft_testkit::cff::cid_keyed(cids)),
                Program::TrueTypePost(glyphs, post) => {
                    let drawn = self.codes_of(i).into_iter().map(|c| font.glyph_of(c)).collect::<Vec<_>>();
                    ("".into(), pdfcraft_testkit::truetype::subset(*glyphs, &drawn, pdfcraft_testkit::truetype::Cmap::None, Some(*post)))
                }
                Program::Raw(bytes) => ("/Subtype /CIDFontType0C".into(), bytes.clone()),
                Program::None => ("".into(), Vec::new()),
            };
            objects.push((program_stream.0, Some(program_stream.1)));
            // The fifth object: the ToUnicode map or the CIDToGIDMap (a font has at most one).
            let fifth = if font.to_unicode {
                Some(b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /T def 1 begincodespacerange <0000> <FFFF> endcodespacerange 1 beginbfrange <0000> <FFFF> <0058> endbfrange endcmap CMapName currentdict /CMap defineresource pop end end".to_vec())
            } else {
                font.cid_to_gid.as_ref().map(|map| map.iter().flat_map(|g| g.to_be_bytes()).collect())
            };
            objects.push((String::new(), Some(fifth.unwrap_or_default())));
            let _ = write!(font_refs, " /F{i} {type0} 0 R");
        }
        let mut kids = Vec::new();
        for page in &self.pages {
            let mut xobjects = String::new();
            let contents: Vec<String> = if streams.split {
                page.iter().map(|show| self.content(std::slice::from_ref(show), &mut objects, &font_refs, &mut xobjects, streams)).collect()
            } else {
                vec![self.content(page, &mut objects, &font_refs, &mut xobjects, streams)]
            };
            let page_at = objects.len() + 1;
            let refs: Vec<String> = (0..contents.len()).map(|k| format!("{} 0 R", page_at + 1 + k)).collect();
            let refs = if streams.split { format!("[{}]", refs.join(" ")) } else { refs.join(" ") };
            objects.push((
                format!("<< /Type /Page /Parent 2 0 R /Contents {refs} /Resources << /Font <<{font_refs} >> /XObject <<{xobjects} >> >> >>"),
                None,
            ));
            for content in contents {
                objects.push(content_stream(String::new(), content, streams));
            }
            kids.push(format!("{page_at} 0 R"));
        }
        if let Some(pages) = objects.get_mut(1) {
            pages.0 = format!("<< /Type /Pages /Kids [{}] /Count {} /MediaBox [0 0 600 800] >>", kids.join(" "), kids.len());
        }
        let mut pdf = pdfcraft_testkit::Pdf::new();
        for (dict, data) in objects {
            pdf = match data {
                Some(data) => pdf.stream(&dict, data),
                None => pdf.obj(dict),
            };
        }
        pdf.build()
    }

    /// Every code shown in font `font`, anywhere in the document.
    fn codes_of(&self, font: usize) -> Vec<u16> {
        fn walk(shows: &[Show], font: usize, out: &mut Vec<u16>) {
            for show in shows {
                match show {
                    Show::Text(f, codes) if *f == font => out.extend(codes),
                    Show::Text(..) | Show::Raw(_) => {}
                    Show::Form(inner, _) => walk(inner, font, out),
                }
            }
        }
        let mut out = Vec::new();
        for page in &self.pages {
            walk(page, font, &mut out);
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// A content stream showing `shows`, forms added to `objects` and named in `xobjects`.
    fn content(
        &self,
        shows: &[Show],
        objects: &mut Vec<(String, Option<Vec<u8>>)>,
        font_refs: &str,
        xobjects: &mut String,
        streams: Streams,
    ) -> String {
        let mut content = String::new();
        let mut y = 760;
        for show in shows {
            match show {
                Show::Text(font, codes) => {
                    // Forty glyphs a line, so every glyph is on the page.
                    for line in codes.chunks(40) {
                        let hex: String = line.iter().map(|c| format!("{c:04X}")).collect();
                        let _ = write!(content, "BT /F{font} 10 Tf 20 {y} Td <{hex}> Tj ET ");
                        y -= 12;
                    }
                }
                Show::Form(inner, times) => {
                    let mut inner_xobjects = String::new();
                    let inner_content = self.content(inner, objects, font_refs, &mut inner_xobjects, streams);
                    let at = objects.len() + 1;
                    objects.push(content_stream(
                        format!(
                            "/Type /XObject /Subtype /Form /BBox [0 0 600 800] /Resources << /Font <<{font_refs} >> /XObject <<{inner_xobjects} >> >>"
                        ),
                        inner_content,
                        streams,
                    ));
                    let _ = write!(xobjects, " /X{at} {at} 0 R");
                    for _ in 0..*times {
                        let _ = write!(content, "q /X{at} Do Q ");
                    }
                }
                Show::Raw(ops) => {
                    content.push_str(ops);
                    content.push(' ');
                }
            }
        }
        content
    }
}

impl Font {
    /// The glyph id code `code` draws (through the `/CIDToGIDMap` stream, when there is one).
    pub(super) fn glyph_of(&self, code: u16) -> u16 {
        match &self.cid_to_gid {
            Some(map) => map.get(usize::from(code)).copied().unwrap_or(0),
            None => code,
        }
    }
}
