#[test]
fn png_predictor_rows_longer_than_the_data_are_refused_before_allocating() {
    // Vendored lopdf patch: a fuzzed `/Columns 4294967295` allocated two 4 GiB rows before
    // reading any data. A row that cannot fit in the data is refused up front.
    let e = lopdf::filters::png::decode_frame(&[2, 0, 0, 0], 1, 64 << 20).unwrap_err();
    assert!(e.to_string().contains("longer than the data"), "{e}");
    // A real frame still decodes: two 3-byte rows, the second Up-filtered.
    assert_eq!(lopdf::filters::png::decode_frame(&[0, 1, 2, 3, 2, 1, 1, 1], 1, 3).unwrap(), [1, 2, 3, 2, 3, 4]);
}

#[test]
fn empty_png_predictor_frames_do_not_allocate_rows() {
    // usize::MAX cannot be reserved, so the old code safely errors before allocating.
    // An empty frame needs no rows regardless of the declared width.
    assert!(lopdf::filters::png::decode_frame(&[], 1, usize::MAX).unwrap().is_empty());
}

fn tiff_predictor_stream(data: Vec<u8>, columns: i64, colors: i64, bits: i64) -> lopdf::Stream {
    let mut params = lopdf::Dictionary::new();
    params.set("Predictor", 2i64);
    params.set("Columns", columns);
    params.set("Colors", colors);
    params.set("BitsPerComponent", bits);
    let mut dict = lopdf::Dictionary::new();
    dict.set("DecodeParms", params);
    let mut stream = lopdf::Stream::new(dict, data);
    stream.compress().unwrap();
    assert_eq!(stream.dict.get(b"Filter").unwrap().as_name().unwrap(), b"FlateDecode");
    stream
}

#[test]
fn tiff_subbyte_predictor_row_width_overflow_is_refused() {
    // Each multiplication overflows before any scratch allocation in the old debug build.
    for bits in [1, 2, 4] {
        let stream = tiff_predictor_stream(vec![0; 64], i64::MAX, 3, bits);
        assert!(
            matches!(stream.decompressed_content_with_limit(64), Err(lopdf::Error::Decompress(lopdf::DecompressError::Predictor(_)))),
            "{bits}-bit row"
        );
    }
}

#[test]
fn tiff_subbyte_predictor_scratch_is_bounded_by_available_samples() {
    // 1/2-bit row widths fit usize on 32- and 64-bit hosts; Colors previously made Vec<u16>
    // reject the capacity before allocating. A partial row has no preceding pixel.
    for bits in [1, 2] {
        let data = vec![0b1010_0110; 64];
        let stream = tiff_predictor_stream(data.clone(), 1, i64::try_from(isize::MAX).unwrap(), bits);
        assert_eq!(stream.decompressed_content_with_limit(64).unwrap(), data, "{bits}-bit row");
    }
}

#[test]
fn tiff_subbyte_predictors_keep_components_rows_and_padding() {
    // Repeated tiny rows compress through the public Stream API. Rows remain independent,
    // differences wrap at each component depth, and trailing padding bits survive.
    for (columns, colors, bits, encoded, decoded) in [
        (16, 1, 1, vec![255, 170, 8, 255], vec![170, 204, 15, 85]),
        (6, 1, 2, vec![85, 179], vec![108, 147]),
        (3, 1, 4, vec![25, 16], vec![26, 176]),
        (2, 2, 4, vec![18, 34], vec![18, 52]),
    ] {
        let stream = tiff_predictor_stream(encoded.repeat(32), columns, colors, bits);
        assert_eq!(stream.decompressed_content_with_limit(128).unwrap(), decoded.repeat(32), "{bits}-bit, {colors} colours");
    }
}

// Xref-stream fixtures exercise indirect /Length lookup during document loading: stream 2's /Length is `3 0 R`,
// `containers` lists compressed (id, container, index) entries and `objects` the other objects in the file.
fn compressed_length_pdf(containers: &[(u32, u32, u16)], objects: &[(u32, &[u8])]) -> Vec<u8> {
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let ids = containers.iter().flat_map(|&(id, container, _)| [id, container]).chain(objects.iter().map(|&(id, _)| id));
    let xref_id = ids.max().unwrap_or(3) + 1;
    let mut records = vec![[0u8; 7]; (xref_id + 1) as usize];
    records[0] = [0, 0, 0, 0, 0, 255, 255];
    let mut append = |id: u32, body: &[u8]| {
        records[id as usize][0] = 1;
        records[id as usize][1..5].copy_from_slice(&(pdf.len() as u32).to_be_bytes());
        pdf.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    };
    append(1, b"<< /Type /Catalog >>");
    append(2, b"<< /Length 3 0 R >>\nstream\nhello\nendstream");
    for &(id, body) in objects {
        append(id, body);
    }
    for &(id, container, index) in containers {
        records[id as usize][0] = 2;
        records[id as usize][1..5].copy_from_slice(&container.to_be_bytes());
        records[id as usize][5..7].copy_from_slice(&index.to_be_bytes());
    }
    let xref_offset = pdf.len();
    records[xref_id as usize][0] = 1;
    records[xref_id as usize][1..5].copy_from_slice(&(xref_offset as u32).to_be_bytes());
    pdf.extend_from_slice(
        format!("{xref_id} 0 obj\n<< /Type /XRef /Size {} /Root 1 0 R /W [1 4 2] /Length {} >>\nstream\n", records.len(), records.len() * 7)
            .as_bytes(),
    );
    for record in records {
        pdf.extend_from_slice(&record);
    }
    pdf.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes());
    pdf
}

// Stream 2's /Length starts a chain of `hops` indirect /Length lookups, each naming a new object; only the last
// link's /Length is direct. Compressed: link k is the integer 3 + 2k (the length it measures) alone in the object
// stream 4 + 2k, whose own /Length is link k + 1. Plain: link k is the stream 3 + k, whose /Length is link k + 1.
fn length_chain_pdf(hops: u32, compressed: bool) -> Vec<u8> {
    let length = |hop: u32, next: u32, data: &str| if hop + 1 < hops { format!("{next} 0 R") } else { data.len().to_string() };
    let (mut containers, mut objects) = (Vec::new(), Vec::new());
    let mut value = 5; // the length of stream 2's data, "hello"
    for hop in 0..hops {
        if compressed {
            let (id, container) = (3 + 2 * hop, 4 + 2 * hop);
            let index = format!("{id} 0 ");
            let data = format!("{index}{value}");
            let dict = format!("/Type /ObjStm /N 1 /First {} /Length {}", index.len(), length(hop, id + 2, &data));
            objects.push((container, format!("<< {dict} >>\nstream\n{data}\nendstream")));
            containers.push((id, container, 0));
            value = data.len();
        } else {
            objects.push((3 + hop, format!("<< /Length {} >>\nstream\n\nendstream", length(hop, 4 + hop, ""))));
        }
    }
    let objects: Vec<_> = objects.iter().map(|(id, body)| (*id, body.as_bytes())).collect();
    compressed_length_pdf(&containers, &objects)
}

// Lenient loading keeps a document whose stream 2 has an unresolvable /Length: the catalog, and stream 2's
// dictionary without its data.
fn load_without_stream_2_data(pdf: &[u8]) -> Document {
    let doc = Document::load_mem(pdf).unwrap();
    assert!(doc.catalog().unwrap().has_type(b"Catalog"));
    assert!(doc.get_object((2, 0)).unwrap().as_stream().unwrap().content.is_empty());
    doc
}

#[test]
fn compressed_object_self_container_does_not_recurse() {
    let doc = load_without_stream_2_data(&compressed_length_pdf(&[(3, 3, 0)], &[]));
    assert!(doc.get_object((3, 0)).is_err());
}

#[test]
fn compressed_object_mutual_containers_do_not_recurse() {
    let doc = load_without_stream_2_data(&compressed_length_pdf(&[(3, 4, 0), (4, 3, 0)], &[]));
    assert!(doc.get_object((3, 0)).is_err());
    assert!(doc.get_object((4, 0)).is_err());
}

#[test]
fn object_stream_length_cycle_does_not_recurse() {
    let pdf = compressed_length_pdf(&[(3, 4, 0)], &[(4, b"<< /Type /ObjStm /N 1 /First 4 /Length 3 0 R >>\nstream\n3 0 5\nendstream")]);
    let doc = load_without_stream_2_data(&pdf);
    // The container's /Length is the object it holds: the container is kept without data, so 3 never loads.
    assert!(doc.get_object((4, 0)).unwrap().as_stream().unwrap().content.is_empty());
    assert!(doc.get_object((3, 0)).is_err());
}

#[test]
fn compressed_object_container_chain_does_not_recurse() {
    let containers: Vec<_> = (3..100_003).map(|id| (id, id + 1, 0)).collect();
    let pdf = compressed_length_pdf(&containers, &[(100_003, b"<< /Type /ObjStm /N 1 /First 4 /Length 5 >>\nstream\n3 0 5\nendstream")]);
    let doc = load_without_stream_2_data(&pdf);
    // The object stream in the file loads, but the xref puts 3 in the compressed container 4, which is refused.
    assert_eq!(doc.get_object((100_003, 0)).unwrap().as_stream().unwrap().content, b"3 0 5");
    assert!(doc.get_object((3, 0)).is_err());
}

#[test]
fn compressed_objects_and_indirect_stream_length_still_load() {
    let pdf =
        compressed_length_pdf(&[(3, 4, 0), (5, 4, 1)], &[(4, b"<< /Type /ObjStm /N 2 /First 8 /Length 17 >>\nstream\n3 0 5 2 5 (value)\nendstream")]);
    let doc = Document::load_mem_with_options(&pdf, LoadOptions { strict: true, ..Default::default() }).unwrap();
    assert_eq!(doc.get_object((3, 0)).unwrap().as_i64().unwrap(), 5);
    assert_eq!(doc.get_object((5, 0)).unwrap().as_str().unwrap(), b"value");
    assert_eq!(doc.get_object((2, 0)).unwrap().as_stream().unwrap().content, b"hello");
}

#[test]
fn compressed_length_in_a_container_with_an_indirect_length_still_loads() {
    // Two hops, as in a valid file: stream 2's /Length (3) is compressed in container 4, whose own /Length (5) is
    // a plain indirect integer.
    let pdf = compressed_length_pdf(&[(3, 4, 0)], &[(4, b"<< /Type /ObjStm /N 1 /First 4 /Length 5 0 R >>\nstream\n3 0 5\nendstream"), (5, b"5")]);
    let doc = Document::load_mem_with_options(&pdf, LoadOptions { strict: true, ..Default::default() }).unwrap();
    assert_eq!(doc.get_object((3, 0)).unwrap().as_i64().unwrap(), 5);
    assert_eq!(doc.get_object((5, 0)).unwrap().as_i64().unwrap(), 5);
    assert_eq!(doc.get_object((2, 0)).unwrap().as_stream().unwrap().content, b"hello");
}

#[test]
fn compressed_length_chain_does_not_recurse() {
    // Every container is an ordinary object in the file and every hop names a new object, so neither the
    // container check nor the cycle history stops the chain; each hop recursed once more (a 512 KiB stack
    // overflowed within 150 hops). Lookups past the cap fail: the first link never loads, while the last one,
    // next to the direct /Length, does.
    let hops = 5_000;
    let doc = load_without_stream_2_data(&length_chain_pdf(hops, true));
    assert!(doc.get_object((3, 0)).is_err());
    assert!(doc.get_object((3 + 2 * (hops - 1), 0)).unwrap().as_i64().is_ok());
    // The cap itself: loading link j reads its container's /Length, a chain of 2 * (hops - 1 - j) lookups, and
    // 16 are allowed, so the 9th link from the end loads and the 10th does not.
    assert!(doc.get_object((3 + 2 * (hops - 9), 0)).is_ok());
    assert!(doc.get_object((3 + 2 * (hops - 10), 0)).is_err());
}

#[test]
fn plain_stream_length_chain_does_not_recurse() {
    // The same recursion without object streams: each stream's /Length names the next stream (a 512 KiB stack
    // overflowed within 200 hops). A stream is no length, so every link is kept as a stream without data.
    let hops = 5_000;
    let doc = load_without_stream_2_data(&length_chain_pdf(hops, false));
    assert!((3..3 + hops).all(|id| doc.get_object((id, 0)).is_ok_and(|link| link.as_stream().is_ok_and(|s| s.content.is_empty()))));
}

fn indexed_object_stream(pairs: &[(u32, usize)], members: &[u8]) -> lopdf::Stream {
    let mut content = Vec::new();
    for &(id, offset) in pairs {
        content.extend_from_slice(format!("{id} {offset} ").as_bytes());
    }
    let mut dict = Dictionary::new();
    dict.set("Type", "ObjStm");
    dict.set("N", pairs.len() as i64);
    dict.set("First", content.len() as i64);
    content.extend_from_slice(members);
    lopdf::Stream::new(dict, content)
}

#[test]
fn object_stream_member_extents_bound_owned_strings() {
    let mut members = vec![b'('; 32];
    members.extend(vec![b'x'; 1 << 20]);
    members.extend([b')'; 32]);
    // Put the innermost (complete) string first in index order, then overlapping starts
    // and duplicates. Only this first member at offset 31 may own the large string.
    let mut pairs = vec![(100, 31)];
    pairs.extend((0..31).map(|offset| (offset as u32 + 1, offset)));
    pairs.extend([(200, 31), (201, 31)]);
    let stream = indexed_object_stream(&pairs, &members);
    let parsed = lopdf::ObjectStream::new_with_limit(&stream, Some(2 << 20)).unwrap();
    assert_eq!(parsed.objects.get(&(100, 0)).unwrap().as_str().unwrap().len(), 1 << 20);
    let total: usize = parsed.objects.values().filter_map(|object| object.as_str().ok()).map(<[u8]>::len).sum();
    assert!(total <= stream.content.len(), "owned {total} string bytes from {} decoded bytes", stream.content.len());
    assert!(!parsed.objects.contains_key(&(200, 0)));
    assert!(!parsed.objects.contains_key(&(201, 0)));
}

#[test]
fn object_stream_member_extents_preserve_index_order_and_whitespace() {
    let members = b" \n42\n /Name \n[true 7]\n << /Key (value) >>\n (last)";
    // Unsorted offsets, and object 9 appears twice: its last index entry still wins.
    let pairs = [(9, 0), (12, 21), (10, 5), (11, 12), (9, 41)];
    let parsed = lopdf::ObjectStream::new(&indexed_object_stream(&pairs, members)).unwrap();
    assert_eq!(parsed.objects.len(), 4);
    assert_eq!(parsed.objects.get(&(9, 0)).unwrap().as_str().unwrap(), b"last");
    assert_eq!(parsed.objects.get(&(10, 0)).unwrap().as_name().unwrap(), b"Name");
    assert_eq!(parsed.objects.get(&(11, 0)).unwrap().as_array().unwrap(), &[Object::Boolean(true), Object::Integer(7)]);
    assert_eq!(parsed.objects.get(&(12, 0)).unwrap().as_dict().unwrap().get(b"Key").unwrap().as_str().unwrap(), b"value");
}

#[test]
fn view_and_user_space_round_trip_for_every_rotation() {
    for rotation in [0u16, 90, 180, 270] {
        let (w, h) = if rotation % 180 == 0 { (200.0, 300.0) } else { (300.0, 200.0) };
        let p = super::PageInfo { width: w, height: h, label: String::new(), crop: [10.0, 20.0, 210.0, 320.0], rotation };
        for (x, y) in [(0.0, 0.0), (15.0, 40.0), (w, h)] {
            let [ux, uy] = p.view_to_user(x, y);
            let [vx, vy] = p.user_to_view(ux, uy);
            assert!((vx - x).abs() < 1e-3 && (vy - y).abs() < 1e-3, "{rotation}: {x},{y} → {ux},{uy} → {vx},{vy}");
        }
    }
    // Unrotated: the view's top-left is the crop box's top-left.
    let p = super::PageInfo { width: 200.0, height: 300.0, label: String::new(), crop: [10.0, 20.0, 210.0, 320.0], rotation: 0 };
    assert_eq!(p.view_to_user(0.0, 0.0), [10.0, 320.0]);
    assert_eq!(p.view_rect_to_quad([0.0, 0.0, 10.0, 5.0]), [10.0, 320.0, 20.0, 320.0, 10.0, 315.0, 20.0, 315.0]);
}

/// #2583: a NoRotate comment is drawn upright with its top-left as displayed at the upper-left
/// corner of `/Rect`, its width across and its height down; a box resized as drawn maps back to
/// the `/Rect` that shows it. Other comments, and pages that aren't quarter-turned, keep `/Rect`.
#[test]
fn no_rotate_comments_are_shown_upright_from_their_corner_and_back() {
    let mut note = super::Annotation {
        page: 0,
        subtype: "Text".into(),
        author: None,
        contents: None,
        modified: None,
        name: None,
        in_reply_to: None,
        parent: None,
        grouped: false,
        rect: [40.0, 50.0, 70.0, 70.0],
        color: None,
        index: 0,
        state: None,
        quads: Vec::new(),
        locked: false,
        intent: None,
        fill_sign: false,
        no_rotate: true,
        points: Vec::new(),
        text_box: None,
    };
    let rect = note.rect.map(f64::from);
    for rotation in [0u16, 90, 180, 270, 45] {
        let (w, h) = if rotation % 180 == 90 { (140.0, 200.0) } else { (200.0, 140.0) };
        let p = super::PageInfo { width: w, height: h, label: String::new(), crop: [0.0, 0.0, 200.0, 140.0], rotation };
        let shown = note.shown_rect(rotation);
        let (a, b) = (p.user_to_view(shown[0], shown[1]), p.user_to_view(shown[2], shown[3]));
        let [cx, cy] = p.user_to_view(40.0, 70.0);
        let want = [cx, cy, cx + 30.0, cy + 20.0];
        let got = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
        assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-3), "{rotation}: shown {got:?}, want {want:?}");
        assert_eq!(note.rect_for_shown(note.shown(rect, rotation), rotation), rect, "{rotation}");
        // Moved by (5, -7) and resized to 50 × 10 as drawn, back to a /Rect.
        let resized = [shown[0] as f64 + 5.0, shown[1] as f64 - 7.0, 0.0, 0.0];
        let across = if rotation % 180 == 90 { [10.0, 50.0] } else { [50.0, 10.0] };
        let resized = [resized[0], resized[1], resized[0] + across[0], resized[1] + across[1]];
        let back = note.rect_for_shown(resized, rotation);
        assert_eq!((back[2] - back[0], back[3] - back[1]), (50.0, 10.0), "{rotation}: {back:?}");
        assert_eq!(note.shown(back, rotation), resized, "{rotation}");
    }
    note.no_rotate = false;
    assert_eq!(note.shown_rect(90), note.rect);
    assert_eq!(note.rect_for_shown([1.0, 2.0, 3.0, 4.0], 90), [1.0, 2.0, 3.0, 4.0]);
}

use super::*;

#[test]
fn roman_and_alpha_labels() {
    assert_eq!(roman(4, MAX_LABEL_BYTES).as_deref(), Some("iv"));
    assert_eq!(roman(1994, MAX_LABEL_BYTES).as_deref(), Some("mcmxciv"));
    assert_eq!(alpha(1, MAX_LABEL_BYTES).as_deref(), Some("a"));
    assert_eq!(alpha(27, MAX_LABEL_BYTES).as_deref(), Some("aa"));
    assert_eq!(alpha(53, MAX_LABEL_BYTES).as_deref(), Some("aaa"));
}

// Synthetic label-only inspection fixtures: never construct an explosive branching tree.
fn label_info(nums: Vec<Object>, kids: Vec<Object>) -> DocInfo {
    let mut doc = Document::with_version("1.7");
    let mut tree = Dictionary::new();
    tree.set("Nums", nums);
    tree.set("Kids", kids);
    doc.objects.insert((6, 0), Object::Dictionary(tree));
    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("PageLabels", Object::Reference((6, 0)));
    doc.objects.insert((1, 0), Object::Dictionary(catalog));
    doc.trailer.set("Root", Object::Reference((1, 0)));
    let mut info = DocInfo {
        pages: (1..=3).map(|n| PageInfo { width: 200.0, height: 300.0, crop: [0.0, 0.0, 200.0, 300.0], rotation: 0, label: n.to_string() }).collect(),
        ..Default::default()
    };
    Inspector::new(&doc).fill(&mut info);
    info
}

fn label_spec(style: &str, prefix: Vec<u8>, first: i64) -> Object {
    let mut d = Dictionary::new();
    d.set("S", Object::Name(style.as_bytes().to_vec()));
    d.set("P", Object::String(prefix, lopdf::StringFormat::Hexadecimal));
    d.set("St", first);
    Object::Dictionary(d)
}

fn assert_label_fallback(info: &DocInfo) {
    assert_eq!(info.pages.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["1", "2", "3"]);
    assert_eq!(info.warnings.len(), 1, "{:?}", info.warnings);
    assert!(info.warnings[0].contains("Page labels"), "{:?}", info.warnings);
    assert!(info.warnings[0].contains("physical page numbers"), "{:?}", info.warnings);
}

// Contributor-original, blank three-page PDFs. The public importer reads both labels and metadata.
fn page_prefix_space_pdf(prefix: Vec<u8>, decimal: bool, indirect: bool) -> Vec<u8> {
    use lopdf::dictionary;
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let kids: Vec<Object> = (0..3)
        .map(|_| {
            doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 200.into(), 300.into()],
                "Resources" => Dictionary::new(),
            })
            .into()
        })
        .collect();
    doc.objects.insert(pages_id, dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 3 }.into());
    let prefix = Object::String(prefix, lopdf::StringFormat::Hexadecimal);
    let prefix = if indirect { Object::Reference(doc.add_object(prefix)) } else { prefix };
    let mut label = Dictionary::new();
    label.set("P", prefix);
    if decimal {
        label.set("S", Object::Name(b"D".to_vec()));
    }
    let labels = dictionary! { "Nums" => vec![Object::Integer(0), Object::Dictionary(label)] };
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id, "PageLabels" => labels });
    let info_id = doc.add_object(dictionary! { "Title" => Object::string_literal("  Metadata title  ") });
    doc.trailer.set("Root", catalog_id);
    doc.trailer.set("Info", info_id);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).expect("write original page-prefix fixture");
    bytes
}

#[test]
fn page_label_prefix_spaces_survive_public_inspection() {
    let mut utf16 = vec![0xfe, 0xff];
    utf16.extend(" Ω ".encode_utf16().flat_map(u16::to_be_bytes));
    for (raw, prefix, decimal, indirect) in [
        (b" leading".to_vec(), " leading", true, false),
        (b"trailing ".to_vec(), "trailing ", true, false),
        (b" both ".to_vec(), " both ", false, false),
        (b" ".to_vec(), " ", false, false),
        (b" Caf\xe9 ".to_vec(), " Café ", true, false),
        (utf16.clone(), " Ω ", true, false),
        (utf16, " Ω ", false, true),
        (Vec::new(), "", true, false),
        (Vec::new(), "", false, false),
    ] {
        let bytes = page_prefix_space_pdf(raw, decimal, indirect);
        let info = inspect(Arc::new(bytes), None).expect("public import of original page-prefix fixture");
        let expected: Vec<String> = (1..=3)
            .map(|n| {
                if decimal {
                    format!("{prefix}{n}")
                } else if prefix.is_empty() {
                    n.to_string()
                } else {
                    prefix.to_owned()
                }
            })
            .collect();
        assert_eq!(info.pages.iter().map(|p| p.label.clone()).collect::<Vec<_>>(), expected, "{prefix:?}, decimal={decimal}, indirect={indirect}");
        assert!(info.warnings.is_empty(), "{:?}", info.warnings);
        assert_eq!(info.title.as_deref(), Some("Metadata title"), "page-prefix decoding must not change metadata normalization");
    }
}

#[test]
fn page_label_limits_keep_normal_pdf_numbering() {
    for (style, first, expected) in
        [("r", 4, ["iv", "v", "vi"]), ("R", 1994, ["MCMXCIV", "MCMXCV", "MCMXCVI"]), ("a", 26, ["z", "aa", "bb"]), ("A", 52, ["ZZ", "AAA", "BBB"])]
    {
        let info = label_info(vec![Object::Integer(0), label_spec(style, Vec::new(), first)], Vec::new());
        assert_eq!(info.pages.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), expected);
        assert!(info.warnings.is_empty(), "{:?}", info.warnings);
    }
}

#[test]
fn page_label_limits_reject_long_prefixes_without_partial_labels() {
    for prefix in [vec![b'x'; 1025], vec![b'x'; 2051]] {
        let nums = vec![Object::Integer(0), label_spec("D", b"ok-".to_vec(), 1), Object::Integer(1), label_spec("D", prefix, 1)];
        assert_label_fallback(&label_info(nums, Vec::new()));
    }
}

#[test]
fn page_label_limits_count_decoded_utf8_bytes() {
    let mut prefix = vec![0xfe, 0xff];
    for _ in 0..342 {
        prefix.extend_from_slice(&0x2022u16.to_be_bytes());
    }
    assert_label_fallback(&label_info(vec![Object::Integer(0), label_spec("", prefix, 1)], Vec::new()));
}

#[test]
fn page_label_limits_bound_roman_and_alpha_output() {
    // Just 1025 output bytes on the old path, so RED does not consume excessive memory.
    for (style, first) in [("r", 1_025_000), ("a", 26 * 1024 + 1)] {
        assert_label_fallback(&label_info(vec![Object::Integer(0), label_spec(style, Vec::new(), first)], Vec::new()));
    }
}

#[test]
fn page_label_limits_detect_one_node_cycle() {
    let nums = vec![Object::Integer(0), label_spec("D", b"custom-".to_vec(), 1)];
    assert_label_fallback(&label_info(nums, vec![Object::Reference((6, 0))]));
}

#[test]
fn page_label_limits_accept_utf16_ascii_at_the_output_boundary() {
    let mut prefix = vec![0xfe, 0xff];
    for _ in 0..1023 {
        prefix.extend_from_slice(&u16::from(b'x').to_be_bytes());
    }
    let info = label_info(vec![Object::Integer(0), label_spec("D", prefix, 1)], Vec::new());
    assert!(info.warnings.is_empty(), "{:?}", info.warnings);
    assert_eq!(info.pages[0].label, format!("{}1", "x".repeat(1023)));
    assert!(info.pages.iter().all(|p| p.label.len() == MAX_LABEL_BYTES));
}

#[test]
fn page_label_limits_refuse_hostile_numbers_before_allocating() {
    for style in ["r", "R", "a", "A"] {
        assert_label_fallback(&label_info(vec![Object::Integer(0), label_spec(style, Vec::new(), i64::MAX)], Vec::new()));
    }
    // Decimal labels remain small even when their first number exceeds 32-bit usize.
    let info = label_info(vec![Object::Integer(0), label_spec("D", Vec::new(), i64::MAX)], Vec::new());
    assert_eq!(info.pages[0].label, i64::MAX.to_string());
    assert_eq!(info.pages[1].label, (i64::MAX as u64 + 1).to_string());
    assert!(info.warnings.is_empty());
}

#[test]
fn page_label_limits_bound_wide_number_trees() {
    assert_label_fallback(&label_info(Vec::new(), vec![Object::Null; MAX_LABEL_TREE_WORK + 1]));
    let pair = [Object::Integer(0), label_spec("D", Vec::new(), 1)];
    let nums = (0..MAX_LABEL_TREE_WORK).flat_map(|_| pair.clone()).collect();
    assert_label_fallback(&label_info(nums, Vec::new()));
}

#[test]
fn page_label_limits_bound_total_custom_output_atomically() {
    let doc = Document::with_version("1.7");
    let mut tree = Dictionary::new();
    tree.set("Nums", vec![Object::Integer(0), label_spec("", vec![b'x'; MAX_LABEL_BYTES], 1)]);
    let mut catalog = Dictionary::new();
    catalog.set("PageLabels", Object::Dictionary(tree));
    let count = MAX_LABEL_TOTAL_BYTES / MAX_LABEL_BYTES + 1;
    let mut pages: Vec<_> =
        (1..=count).map(|n| PageInfo { width: 200.0, height: 300.0, crop: [0.0, 0.0, 200.0, 300.0], rotation: 0, label: n.to_string() }).collect();
    let err = Inspector::new(&doc).page_labels(&catalog, &mut pages).unwrap_err();
    assert!(err.contains("4 MiB"), "{err}");
    assert!(pages.iter().enumerate().all(|(i, p)| p.label == (i + 1).to_string()));
    // Exact aggregate boundary remains supported.
    pages.pop();
    Inspector::new(&doc).page_labels(&catalog, &mut pages).unwrap();
    assert!(pages.iter().all(|p| p.label.len() == MAX_LABEL_BYTES));
}

#[test]
fn page_label_limits_bound_the_total_of_decoded_prefixes() {
    // `ranges` ranges naming one shared prefix object of `prefix` bytes, without a style.
    let shared = |ranges: i64, prefix: usize| {
        let mut doc = Document::with_version("1.7");
        doc.objects.insert((7, 0), Object::String(vec![b'x'; prefix], lopdf::StringFormat::Literal));
        let mut spec = Dictionary::new();
        spec.set("P", Object::Reference((7, 0)));
        doc.objects.insert((8, 0), Object::Dictionary(spec));
        let mut tree = Dictionary::new();
        tree.set("Nums", (0..ranges).flat_map(|i| [Object::Integer(i), Object::Reference((8, 0))]).collect::<Vec<_>>());
        let mut catalog = Dictionary::new();
        catalog.set("PageLabels", Object::Dictionary(tree));
        (doc, catalog)
    };
    let mut pages: Vec<_> =
        (1..=3).map(|n| PageInfo { width: 200.0, height: 300.0, crop: [0.0, 0.0, 200.0, 300.0], rotation: 0, label: n.to_string() }).collect();
    // Each range decodes its prefix, so a small file sharing one prefix object asks for a
    // copy per range: 5,000 ranges and a 1,000-byte prefix make 5 MB, past the 4 MiB total.
    // Three pages make three labels, far below the label total: the prefixes alone refuse it.
    let (doc, catalog) = shared(5000, 1000);
    let err = Inspector::new(&doc).page_labels(&catalog, &mut pages).unwrap_err();
    assert!(err.contains("prefixes exceed 4 MiB"), "{err}");
    assert!(pages.iter().enumerate().all(|(i, p)| p.label == (i + 1).to_string()));
    // Exactly 4 MiB of prefixes is still read.
    let (doc, catalog) = shared((MAX_PREFIX_TOTAL_BYTES / MAX_LABEL_BYTES) as i64, MAX_LABEL_BYTES);
    Inspector::new(&doc).page_labels(&catalog, &mut pages).unwrap();
    assert!(pages.iter().all(|p| p.label.len() == MAX_LABEL_BYTES));
}

#[test]
fn page_label_limits_handle_prefix_only_and_reference_cycles() {
    let info = label_info(vec![Object::Integer(0), label_spec("", b"prefix".to_vec(), i64::MAX)], Vec::new());
    assert!(info.warnings.is_empty());
    assert!(info.pages.iter().all(|p| p.label == "prefix"));
    let mut doc = Document::with_version("1.7");
    doc.objects.insert((6, 0), Object::Reference((6, 0)));
    let mut catalog = Dictionary::new();
    catalog.set("PageLabels", Object::Reference((6, 0)));
    assert!(Inspector::new(&doc).page_labels(&catalog, &mut []).unwrap_err().contains("cyclic"));
}

#[test]
fn page_label_limits_bound_depth_and_count_combined_work() {
    for depth in [MAX_LABEL_TREE_DEPTH, MAX_LABEL_TREE_DEPTH + 1] {
        let mut child = Dictionary::new();
        child.set("Nums", vec![Object::Integer(0), label_spec("D", b"custom-".to_vec(), 1)]);
        let mut child = Object::Dictionary(child);
        for _ in 1..depth {
            let mut parent = Dictionary::new();
            parent.set("Kids", vec![child]);
            child = Object::Dictionary(parent);
        }
        let info = label_info(Vec::new(), vec![child]);
        if depth == MAX_LABEL_TREE_DEPTH {
            assert!(info.warnings.is_empty());
            assert_eq!(info.pages[0].label, "custom-1");
        } else {
            assert_label_fallback(&info);
        }
    }
    // Each array alone fits: their combined work still must be bounded.
    let half = MAX_LABEL_TREE_WORK / 2;
    let spec = label_spec("D", Vec::new(), 1);
    let nums = (0..half).flat_map(|i| [Object::Integer(i as i64), spec.clone()]).collect();
    assert_label_fallback(&label_info(nums, vec![Object::Null; half]));
}

#[test]
fn page_label_limits_keep_the_later_of_duplicate_ranges() {
    let nums = vec![Object::Integer(0), label_spec("D", b"first-".to_vec(), 1), Object::Integer(0), label_spec("D", b"last-".to_vec(), 1)];
    let info = label_info(nums, Vec::new());
    assert_eq!(info.pages.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["last-1", "last-2", "last-3"]);
    assert_eq!(info.warnings.len(), 1, "{:?}", info.warnings);
    assert!(info.warnings[0].contains("twice"), "{:?}", info.warnings);
}

#[test]
fn page_label_limits_keep_other_ranges_when_one_is_too_long() {
    // Page 1 is labelled normally; pages 2–3 would need over 1024 bytes of roman numerals.
    let nums = vec![Object::Integer(0), label_spec("D", b"ok-".to_vec(), 1), Object::Integer(1), label_spec("r", Vec::new(), 1_025_000)];
    let info = label_info(nums, Vec::new());
    assert_eq!(info.pages.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["ok-1", "2", "3"]);
    assert_eq!(info.warnings.len(), 1, "{:?}", info.warnings);
    assert!(info.warnings[0].contains("physical page numbers"), "{:?}", info.warnings);
}

const ATTACHMENTS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(notes.txt) 6 0 R] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [4 0 R] /Resources << /Font << /F1 9 0 R >> >> >> endobj
4 0 obj << /Type /Annot /Subtype /FileAttachment /Rect [10 10 30 30] /Contents (data.csv) /FS 8 0 R >> endobj
5 0 obj << /Type /EmbeddedFile /Length 11 >> stream
hello notes
endstream endobj
6 0 obj << /Type /Filespec /F (notes.txt) /UF (notes.txt) /EF << /F 5 0 R >> /Desc (Doc-level) >> endobj
7 0 obj << /Type /EmbeddedFile /Length 5 >> stream
a,b,c
endstream endobj
8 0 obj << /Type /Filespec /F (data.csv) /EF << /F 7 0 R >> >> endobj
9 0 obj << /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn attachments_of_both_kinds_are_listed_and_extracted() {
    let bytes = Arc::new(ATTACHMENTS.to_vec());
    let info = inspect(bytes.clone(), None).expect("opens");
    let names: Vec<_> = info.attachments.iter().map(|a| a.name.as_str()).collect();
    assert!(names.contains(&"notes.txt") && names.contains(&"data.csv"), "{names:?}");
    for a in &info.attachments {
        let data = attachment_data(&bytes, None, a).expect("extracts");
        let expected: &[u8] = if a.name == "notes.txt" { b"hello notes" } else { b"a,b,c" };
        assert_eq!(data, expected, "{}", a.name);
    }
    // A non-embedded /Helvetica is reported as standing in for itself: the standard-14
    // metrics are exact, so the substitution is named rather than left implicit.
    assert_eq!(
        info.fonts,
        vec![FontInfo {
            name: "Helvetica".into(),
            kind: "Type1".into(),
            embedded: false,
            subset: true,
            encoding: None,
            substitute: Some("Helvetica".into()),
            substitution: Some(pdfcraft_fonts::fallback::Why::NamedStandard14.note().into()),
        }]
    );
}

const LAYERS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R 7 0 R] /D << /OFF [6 0 R] /RBGroups [[5 0 R 6 0 R] [7 0 R] 8 0 R] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [10 0 R 11 0 R] >> endobj
5 0 obj << /Type /OCG /Name (Red) >> endobj
6 0 obj << /Type /OCG /Name (Green) >> endobj
7 0 obj << /Type /OCG /Name (Blue) >> endobj
8 0 obj [6 0 R 7 0 R] endobj
9 0 obj [/OFF 5 0 R] endobj
10 0 obj << /Type /Annot /Subtype /Link /Rect [10 10 50 30] /A << /S /SetOCGState /State [7 0 R /ON 6 0 R /Bogus 5 0 R /Toggle 5 0 R 1 7 0 R] /PreserveRB false >> >> endobj
11 0 obj << /Type /Annot /Subtype /Link /Rect [60 10 100 30] /A << /S /SetOCGState /State 9 0 R >> >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// `/Trans` and `/Dur` (ISO 32000-2 §12.4.4): read with the spec's defaults for what a page
/// leaves out, lenient about what is out of range, and only for pages that have either.
const TRANSITIONS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R 7 0 R] /Count 5 /MediaBox [0 0 200 200] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Trans << /Type /Trans /S /Wipe /D 2.5 /Di 270 >> /Dur 5 >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /Dur 3 >> endobj
5 0 obj << /Type /Page /Parent 2 0 R /Trans 8 0 R >> endobj
6 0 obj << /Type /Page /Parent 2 0 R /Trans << /S /Bogus /D (slow) /Di 45 /Dm /Q >> /Dur -1 >> endobj
7 0 obj << /Type /Page /Parent 2 0 R >> endobj
8 0 obj << /S /Fly /M /O /SS 0.5 /B true /Di /None /Dm /V >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn page_transitions_and_display_times_are_read() {
    use super::TransitionStyle as S;
    let info = inspect(Arc::new(TRANSITIONS.to_vec()), None).expect("opens");
    let t = &info.transitions;
    assert_eq!(t.iter().map(|t| t.page).collect::<Vec<_>>(), [0, 1, 2, 3], "pages with /Trans or /Dur, in order; page 5 has neither");
    assert_eq!((t[0].style, t[0].duration, t[0].direction, t[0].display), (Some(S::Wipe), 2.5, Some(270), Some(5.0)));
    assert!((t[0].horizontal, t[0].inward, t[0].scale, t[0].opaque) == (true, true, 1.0, false), "defaults: {:?}", t[0]);
    assert_eq!((t[1].style, t[1].duration, t[1].display), (None, 1.0, Some(3.0)), "/Dur without /Trans");
    assert_eq!(
        (t[2].style, t[2].inward, t[2].scale, t[2].opaque, t[2].direction, t[2].horizontal),
        (Some(S::Fly), false, 0.5, true, None, false),
        "an indirect /Trans: {:?}",
        t[2]
    );
    // Out of range: an unknown style is the spec's default R, a non-numeric /D the default
    // second, a direction that is not one of the five falls back to 0, a bad /Dm to
    // horizontal, and a negative /Dur is no display time.
    assert_eq!(
        (t[3].style, t[3].duration, t[3].direction, t[3].horizontal, t[3].display),
        (Some(S::Replace), 1.0, Some(0), true, None),
        "{:?}",
        t[3]
    );
}

const CUSTOM_INFO: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Title (Report) /Producer (Test) /Project (Apollo) /Revision 7 /Dept (R&D) /Trapped /False /Note <FEFF90E8> >> endobj
trailer << /Root 1 0 R /Info 4 0 R >>
%%EOF";

#[test]
fn custom_document_properties_are_listed_apart_from_the_standard_ones() {
    let info = inspect(Arc::new(CUSTOM_INFO.to_vec()), None).expect("opens");
    assert_eq!((info.title.as_deref(), info.producer.as_deref()), (Some("Report"), Some("Test")));
    let mut custom = info.custom.clone();
    custom.sort();
    // Revision is a number, not text, so it is not listed; Trapped is a standard entry.
    assert_eq!(
        custom,
        [("Dept".to_string(), "R&D".to_string()), ("Note".to_string(), "部".to_string()), ("Project".to_string(), "Apollo".to_string())]
    );
}

#[test]
fn radio_button_layer_groups_are_read() {
    let info = inspect(Arc::new(LAYERS.to_vec()), None).expect("opens");
    let layers: Vec<_> = info.layers.iter().map(|l| (l.id, l.name.as_str(), l.visible)).collect();
    assert_eq!(layers, [((5, 0), "Red", true), ((6, 0), "Green", false), ((7, 0), "Blue", true)]);
    // A group of one constrains nothing; an indirect group is read.
    assert_eq!(info.layer_groups, [vec![(5, 0), (6, 0)], vec![(6, 0), (7, 0)]]);
}

#[test]
fn initial_layer_visibility_is_the_default_configuration() {
    // BaseState OFF: Red is listed ON; Green is listed both ON and OFF, and OFF wins; Blue
    // isn't listed, so it takes the base state.
    let pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R 7 0 R] /D << /BaseState /OFF /ON [5 0 R 6 0 R] /OFF [6 0 R] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
5 0 obj << /Type /OCG /Name (Red) >> endobj
6 0 obj << /Type /OCG /Name (Green) >> endobj
7 0 obj << /Type /OCG /Name (Blue) >> endobj
trailer << /Root 1 0 R >>
%%EOF";
    let info = inspect(Arc::new(pdf.to_vec()), None).expect("opens");
    let layers: Vec<_> = info.layers.iter().map(|l| (l.name.as_str(), l.initial)).collect();
    assert_eq!(layers, [("Red", true), ("Green", false), ("Blue", false)]);
}

const ORPHAN_FIELDS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [5 0 R 6 0 R] >> endobj
4 0 obj << /Fields [] >> endobj
5 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (alpha) /V (one) /Rect [10 100 90 120] >> endobj
6 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (beta) /Rect [10 60 90 80] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn fields_listed_only_on_the_pages_are_listed() {
    let info = inspect(Arc::new(ORPHAN_FIELDS.to_vec()), None).expect("opens");
    let listed: Vec<_> = info.fields.iter().map(|f| (f.name.as_str(), f.value.as_deref(), f.page)).collect();
    assert_eq!(listed, [("alpha", Some("one"), Some(0)), ("beta", None, Some(0))], "{:?}", info.fields);
}

/// A `Structure` that counts the objects read through it.
struct CountingReads<'a> {
    doc: &'a Document,
    reads: std::cell::Cell<usize>,
}

impl Structure for CountingReads<'_> {
    fn trailer(&self) -> &Dictionary {
        &self.doc.trailer
    }
    fn get_object(&self, id: ObjectId) -> lopdf::Result<&Object> {
        self.reads.set(self.reads.get() + 1);
        self.doc.get_object(id)
    }
    fn encrypted(&self) -> bool {
        false
    }
}

/// One comment referenced many times, or a page-shared `/Annots` array, is listed once, and many
/// small comments sharing one large `/Contents` string stop at the retained-bytes budget.
#[test]
fn repeated_annotations_are_listed_once_within_a_byte_budget() {
    let small = "<< /Subtype /Text /Rect [0 0 1 1] /Contents 7 0 R >> ".repeat(1200);
    let pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots 5 0 R >> endobj\n\
             4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots 5 0 R >> endobj\n\
             5 0 obj [{} {small}] endobj\n\
             6 0 obj << /Type /Annot /Subtype /Text /Rect [0 0 1 1] /Contents (note) >> endobj\n\
             7 0 obj ({}) endobj\ntrailer << /Root 1 0 R >>\n%%EOF",
        "6 0 R ".repeat(1000),
        "x".repeat(64 << 10),
    );
    let doc = Document::load_mem(pdf.as_bytes()).expect("loads");
    let page = PageInfo { width: 200.0, height: 200.0, crop: [0.0, 0.0, 200.0, 200.0], rotation: 0, label: String::new() };
    let mut info = DocInfo { pages: vec![page.clone(), page], ..Default::default() };
    Inspector::new(&doc).fill(&mut info);
    let notes = &info.annotations;
    assert_eq!(notes.iter().filter(|a| a.contents.as_deref() == Some("note")).count(), 1);
    assert!(notes.iter().all(|a| a.page == 0), "the shared array is the first page's");
    assert!(notes.len() > 1);
    assert!(notes.iter().map(retained).sum::<usize>() <= MAX_ANNOTATION_BYTES);
}

/// A link reached through an indirect object that holds only a reference to it is the same
/// annotation as the link itself, and so is a page array reached the same way: each is listed once.
#[test]
fn an_annotation_reached_through_a_reference_chain_is_listed_once() {
    let pdf = "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots 7 0 R >> endobj\n\
             4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots 8 0 R >> endobj\n\
             5 0 obj 6 0 R endobj\n\
             6 0 obj << /Type /Annot /Subtype /Link /Rect [0 0 1 1] /A << /S /URI /URI (https://example.com) >> >> endobj\n\
             7 0 obj [5 0 R 6 0 R] endobj\n\
             8 0 obj 7 0 R endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF";
    let mut doc = Document::load_mem(pdf.as_bytes()).expect("loads");
    let mut saved = Vec::new();
    doc.save_to(&mut saved).expect("saves");
    let lazy = LazyStructure::new(Arc::new(saved), None).expect("opens");
    let page = PageInfo { width: 200.0, height: 200.0, crop: [0.0, 0.0, 200.0, 200.0], rotation: 0, label: String::new() };
    for reader in [&doc as &dyn Structure, &lazy] {
        let mut info = DocInfo { pages: vec![page.clone(), page.clone()], ..Default::default() };
        Inspector::new(reader).fill(&mut info);
        assert_eq!(info.links.len(), 1, "{:?}", info.links);
    }
}

/// A reply finds its comment where the listing keeps it: on the first page of a page-shared
/// `/Annots` array, whether its `/IRT` names the comment or an object holding a reference to it.
#[test]
fn a_reply_finds_its_comment_through_a_reference_chain_and_a_shared_array() {
    let pdf = "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots 7 0 R >> endobj\n\
             4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots 7 0 R >> endobj\n\
             5 0 obj 6 0 R endobj\n\
             6 0 obj << /Type /Annot /Subtype /Text /Rect [0 0 1 1] /Contents (comment) >> endobj\n\
             7 0 obj [5 0 R 8 0 R 9 0 R] endobj\n\
             8 0 obj << /Type /Annot /Subtype /Text /Rect [0 0 1 1] /Contents (to the comment) /IRT 6 0 R >> endobj\n\
             9 0 obj << /Type /Annot /Subtype /Text /Rect [0 0 1 1] /Contents (through the chain) /IRT 5 0 R >> endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF";
    let mut doc = Document::load_mem(pdf.as_bytes()).expect("loads");
    let mut saved = Vec::new();
    doc.save_to(&mut saved).expect("saves");
    let lazy = LazyStructure::new(Arc::new(saved), None).expect("opens");
    let page = PageInfo { width: 200.0, height: 200.0, crop: [0.0, 0.0, 200.0, 200.0], rotation: 0, label: String::new() };
    for reader in [&doc as &dyn Structure, &lazy] {
        let mut info = DocInfo { pages: vec![page.clone(), page.clone()], ..Default::default() };
        Inspector::new(reader).fill(&mut info);
        let notes = &info.annotations;
        assert_eq!(notes.len(), 3, "{notes:?}");
        let comment = notes.iter().find(|a| a.contents.as_deref() == Some("comment")).expect("listed");
        assert_eq!((comment.page, comment.index, comment.is_reply()), (0, 0, false));
        for reply in ["to the comment", "through the chain"] {
            let a = notes.iter().find(|a| a.contents.as_deref() == Some(reply)).expect("listed");
            assert_eq!((a.page, a.parent), (0, Some((0, 0))), "{reply}");
        }
    }
}

/// A conversion rejected as too long, or trimmed short or to nothing, still spends the raw
/// length, so a string many entries share is converted only a few times.
#[test]
fn rejected_text_conversions_spend_the_budget() {
    let mut budget = 10;
    let mut conversions = 0;
    for _ in 0..10 {
        // Each 0xFF becomes the three-byte replacement character.
        let kept = spend(&[0xFF; 5], &mut budget, |b| {
            conversions += 1;
            String::from_utf8_lossy(b).into_owned()
        });
        assert_eq!(kept, None);
    }
    assert_eq!((conversions, budget), (2, 0));

    let pdf = "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [] /Count 0 >> endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF";
    let doc = Document::load_mem(pdf.as_bytes()).expect("loads");
    let d = Dictionary::from_iter([("Contents", Object::string_literal("   "))]);
    let mut budget = 10;
    assert_eq!(Inspector::new(&doc).budgeted_text(&d, b"Contents", &mut budget), None);
    assert_eq!(budget, 7);
    // Text that trims short spends its raw length.
    let d = Dictionary::from_iter([("Contents", Object::string_literal("      x"))]);
    let mut budget = 10;
    assert_eq!(Inspector::new(&doc).budgeted_text(&d, b"Contents", &mut budget).as_deref(), Some("x"));
    assert_eq!(budget, 3);
}

/// An annotation attachment's `/Contents` spends the text budget only when it names the
/// attachment, so long descriptions of named files leave a later web address listed.
#[test]
fn a_named_attachment_leaves_its_contents_unspent() {
    let files = "<< /Subtype /FileAttachment /Rect [0 0 1 1] /Contents 6 0 R /FS << /F (a.txt) >> >> ".repeat(20);
    // Longer than what 20 MiB of charged `/Contents` would leave of the 16 MiB budget.
    let uri = format!("https://example.com/{}", "a".repeat(2 << 20));
    let pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [{files} \
             << /Subtype /Link /Rect [0 0 1 1] /A << /S /URI /URI ({uri}) >> >>] >> endobj\n\
             6 0 obj ({}) endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF",
        "x".repeat(1 << 20),
    );
    let doc = Document::load_mem(pdf.as_bytes()).expect("loads");
    let page = PageInfo { width: 200.0, height: 200.0, crop: [0.0, 0.0, 200.0, 200.0], rotation: 0, label: String::new() };
    let mut info = DocInfo { pages: vec![page], ..Default::default() };
    Inspector::new(&doc).fill(&mut info);
    assert!(info.attachments.iter().all(|a| a.name == "a.txt"));
    assert_eq!(info.links.len(), 1);
    assert!(matches!(&info.links[0].target, LinkTarget::Uri(u) if *u == uri));
}

/// Embedded files whose name-tree keys share one long string keep it, as key and as name, only
/// within `MAX_LINK_ATTACHMENT_TEXT_BYTES`.
#[test]
fn embedded_file_keys_sharing_a_long_string_keep_it_within_a_budget() {
    let entries = "4 0 R << /Type /Filespec >> ".repeat(20);
    let pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [{entries}] >> >> >> endobj\n\
             2 0 obj << /Type /Pages /Kids [] /Count 0 >> endobj\n\
             4 0 obj ({}) endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF",
        "x".repeat(1 << 20),
    );
    let doc = Document::load_mem(pdf.as_bytes()).expect("loads");
    let mut info = DocInfo::default();
    Inspector::new(&doc).fill(&mut info);
    let kept: usize =
        info.attachments.iter().map(|a| a.name.len() + if let AttachmentSource::Document { key } = &a.source { key.len() } else { 0 }).sum();
    assert!(!info.attachments.is_empty());
    // Past the budget an attachment still has the generic name.
    let generic = info.attachments.len() * "attachment".len();
    assert!(kept <= MAX_LINK_ATTACHMENT_TEXT_BYTES + generic, "{kept} bytes kept");
}

/// An embedded file the listing keeps is extracted, though earlier keys the listing could not
/// keep would fill a fresh key budget before it.
#[test]
fn a_listed_embedded_file_is_extracted_past_keys_the_listing_left_out() {
    let skipped = "6 0 R 7 0 R ".repeat(9);
    let pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [{skipped}(b.txt) 9 0 R] >> >> >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [\
             << /Subtype /Link /Rect [0 0 1 1] /A << /S /URI /URI ({}) >> >>] >> endobj\n\
             6 0 obj ({}) endobj\n\
             7 0 obj << /Type /Filespec /F (a.txt) >> endobj\n\
             8 0 obj << /Type /EmbeddedFile /Length 5 >> stream\nhello\nendstream endobj\n\
             9 0 obj << /Type /Filespec /F (b.txt) /EF << /F 8 0 R >> >> endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF",
        "u".repeat(15 << 20),
        "k".repeat(2 << 20),
    );
    let bytes = Arc::new(pdf.into_bytes());
    let info = inspect(bytes.clone(), None).expect("opens");
    let listed = info.attachments.iter().find(|a| a.name == "b.txt").expect("b.txt is listed");
    assert_eq!(attachment_data(&bytes, None, listed).as_deref(), Ok(&b"hello"[..]));
}

/// Links and attachments sharing one long string each keep a copy only within
/// `MAX_LINK_ATTACHMENT_TEXT_BYTES`, and every one is still listed.
#[test]
fn links_and_attachments_sharing_a_long_string_keep_it_within_a_budget() {
    const EACH: usize = 20;
    let links = "<< /Subtype /Link /Rect [0 0 1 1] /A 5 0 R >> ".repeat(EACH);
    let files = "<< /Subtype /FileAttachment /Rect [0 0 1 1] /FS 7 0 R >> ".repeat(EACH);
    let pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [{links} {files}] >> endobj\n\
             5 0 obj << /S /URI /URI 6 0 R >> endobj\n\
             6 0 obj ({}) endobj\n\
             7 0 obj << /Type /Filespec /F (a.txt) /Desc 6 0 R >> endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF",
        "x".repeat(1 << 20),
    );
    let doc = Document::load_mem(pdf.as_bytes()).expect("loads");
    let page = PageInfo { width: 200.0, height: 200.0, crop: [0.0, 0.0, 200.0, 200.0], rotation: 0, label: String::new() };
    let mut info = DocInfo { pages: vec![page], ..Default::default() };
    Inspector::new(&doc).fill(&mut info);
    assert_eq!(info.links.len(), EACH);
    assert_eq!(info.attachments.len(), EACH);
    let uris = info.links.iter().map(|l| if let LinkTarget::Uri(u) = &l.target { u.len() } else { 0 });
    let descriptions = info.attachments.iter().map(|a| a.description.as_ref().map_or(0, String::len));
    let kept: usize = uris.chain(descriptions).sum();
    assert!(kept > 0 && kept <= MAX_LINK_ATTACHMENT_TEXT_BYTES, "{kept} bytes kept");
    assert!(info.links.iter().any(|l| matches!(&l.target, LinkTarget::Other(s) if s == "URI")));
}

/// Links and attachments after the comment budget fills are still listed, and a JavaScript link
/// among them still marks the document.
#[test]
fn links_and_attachments_past_the_comment_budget_are_listed() {
    let small = "<< /Subtype /Text /Rect [0 0 1 1] /Contents 6 0 R >> ".repeat(1100);
    let pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [{small} \
             << /Subtype /Link /Rect [0 0 1 1] /A << /S /JavaScript /JS (1) >> >>] >> endobj\n\
             4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [\
             << /Subtype /FileAttachment /Rect [0 0 1 1] /FS << /Type /Filespec /F (a.txt) >> >>] >> endobj\n\
             6 0 obj ({}) endobj\ntrailer << /Root 1 0 R >>\n%%EOF",
        "x".repeat(64 << 10),
    );
    let doc = Document::load_mem(pdf.as_bytes()).expect("loads");
    let page = PageInfo { width: 200.0, height: 200.0, crop: [0.0, 0.0, 200.0, 200.0], rotation: 0, label: String::new() };
    let mut info = DocInfo { pages: vec![page.clone(), page], ..Default::default() };
    Inspector::new(&doc).fill(&mut info);
    assert!(info.annotations.len() < 1100, "the comment budget filled");
    assert_eq!(info.links.len(), 1);
    assert!(info.has_javascript);
    assert_eq!(info.attachments.len(), 1);
}

/// Widgets without `/P` are placed on the page that lists them with one pass over the pages'
/// `/Annots`, not a search of every page per field: that was O(fields × annotations), a
/// second to open a form of 8,000 fields. A widget two pages list is on the first of them.
#[test]
fn widgets_without_a_page_entry_are_placed_in_one_pass() {
    const PAGES: usize = 100;
    const PER_PAGE: usize = 10;
    let field = |p: usize, k: usize| 4 + PAGES + p * PER_PAGE + k;
    let mut pdf = String::from("%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm 3 0 R >> endobj\n");
    let kids: Vec<String> = (0..PAGES).map(|p| format!("{} 0 R", 4 + p)).collect();
    pdf += &format!("2 0 obj << /Type /Pages /Kids [{}] /Count {PAGES} >> endobj\n", kids.join(" "));
    let fields: Vec<String> = (0..PAGES).flat_map(|p| (0..PER_PAGE).map(move |k| format!("{} 0 R", field(p, k)))).collect();
    pdf += &format!("3 0 obj << /Fields [{}] >> endobj\n", fields.join(" "));
    for p in 0..PAGES {
        let mut annots: Vec<String> = (0..PER_PAGE).map(|k| format!("{} 0 R", field(p, k))).collect();
        // The last page lists the first page's first widget too.
        if p == PAGES - 1 {
            annots.push(format!("{} 0 R", field(0, 0)));
        }
        pdf += &format!("{} 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [{}] >> endobj\n", 4 + p, annots.join(" "));
    }
    for p in 0..PAGES {
        for k in 0..PER_PAGE {
            pdf += &format!("{} 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (f{p}_{k}) /Rect [10 10 90 30] >> endobj\n", field(p, k));
        }
    }
    pdf += "trailer << /Root 1 0 R >>\n%%EOF";
    let doc = Document::load_mem(pdf.as_bytes()).expect("loads");
    let counting = CountingReads { doc: &doc, reads: std::cell::Cell::new(0) };
    let mut info = DocInfo {
        pages: (0..PAGES)
            .map(|_| PageInfo { width: 200.0, height: 200.0, crop: [0.0, 0.0, 200.0, 200.0], rotation: 0, label: String::new() })
            .collect(),
        ..Default::default()
    };
    Inspector::new(&counting).fill(&mut info);
    let placed: Vec<_> = info.fields.iter().map(|f| (f.name.clone(), f.page)).collect();
    let want: Vec<_> = (0..PAGES).flat_map(|p| (0..PER_PAGE).map(move |k| (format!("f{p}_{k}"), Some(p)))).collect();
    assert_eq!(placed, want);
    // A handful of reads per page and field; searching the pages for each field read the
    // page dictionaries about 50,000 times.
    let reads = counting.reads.get();
    assert!(reads < 20 * (PAGES + PAGES * PER_PAGE), "{reads} object reads");
}

/// #2365: the issue's fixture is two PDFs concatenated, the second cut off before its
/// cross-reference data, and the first's only trailer is a cross-reference stream. The
/// lazy reader refuses a file that needed repair, and lopdf's rebuild found no `trailer`
/// keyword and failed, so Document Properties showed every field empty. The rebuild now
/// takes the cross-reference stream's dictionary as the trailer.
#[test]
fn rebuilt_file_keeps_the_info_of_its_cross_reference_stream() {
    let header = "%PDF-1.5\n";
    let obj1 = "1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n";
    let obj2 = "2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n";
    let obj3 = "3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 10 10]>>endobj\n";
    let obj4 = "4 0 obj<</Title(Kept)/Creator(Me)>>endobj\n";
    let o1 = header.len();
    let o2 = o1 + obj1.len();
    let o3 = o2 + obj2.len();
    let o4 = o3 + obj3.len();
    let o5 = o4 + obj4.len();
    let rows: Vec<u8> =
        [[0u8, 0, 0, 255], [1, 0, o1 as u8, 0], [1, 0, o2 as u8, 0], [1, 0, o3 as u8, 0], [1, 0, o4 as u8, 0], [1, (o5 >> 8) as u8, o5 as u8, 0]]
            .concat();
    let mut bytes = format!("{header}{obj1}{obj2}{obj3}{obj4}").into_bytes();
    bytes.extend_from_slice(format!("5 0 obj<</Type/XRef/Root 1 0 R/Info 4 0 R/Size 6/W[1 2 1]/Length {}>>stream\n", rows.len()).as_bytes());
    bytes.extend_from_slice(&rows);
    // A wrong startxref, and a second file after %%EOF: every reader rebuilds the table.
    bytes.extend_from_slice(b"\nendstream\nendobj\nstartxref\n999999\n%%EOF\n%PDF-1.3\n");
    bytes.extend_from_slice(&[b'x'; 1024]);

    let info = inspect(Arc::new(bytes), None).unwrap();
    assert_eq!(info.pages.len(), 1);
    assert_eq!(info.title.as_deref(), Some("Kept"));
    assert_eq!(info.creator.as_deref(), Some("Me"));
}

/// #2365 review: the rebuild prefers a trailer whose /Root is the catalog. A later
/// cross-reference stream whose /Root points at the Info dictionary used to replace the
/// earlier `trailer`, so Document Properties lost the catalog it should have read.
#[test]
fn rebuilt_file_prefers_a_trailer_whose_root_is_the_catalog() {
    let mut bytes = b"%PDF-1.5\n".to_vec();
    for body in [
        "<</Type/Catalog/Pages 2 0 R>>",
        "<</Type/Pages/Kids[3 0 R]/Count 1>>",
        "<</Type/Page/Parent 2 0 R/MediaBox[0 0 10 10]>>",
        "<</Title(Kept)/Creator(Me)>>",
    ] {
        let n = bytes.iter().filter(|b| **b == b'\n').count();
        bytes.extend_from_slice(format!("{n} 0 obj{body}endobj\n").as_bytes());
    }
    bytes.extend_from_slice(b"trailer\n<</Size 5/Root 1 0 R/Info 4 0 R>>\n");
    bytes.extend_from_slice(b"5 0 obj<</Type/XRef/Root 4 0 R/Size 6/W[1 2 1]/Length 4>>stream\n\x01\x00\x09\x00\nendstream\nendobj\n");
    // A wrong startxref, and a second file after %%EOF: every reader rebuilds the table.
    bytes.extend_from_slice(b"startxref\n999999\n%%EOF\n%PDF-1.3\n");
    bytes.extend_from_slice(&[b'x'; 1024]);

    let info = inspect(Arc::new(bytes.clone()), None).unwrap();
    assert_eq!(info.pages.len(), 1);
    assert_eq!(info.title.as_deref(), Some("Kept"));
    // A later stream whose /Root names the catalog's number with another generation does
    // not count either: loading `1 1 R` finds no such object.
    let at = bytes.windows(10).position(|w| w == b"/Root 4 0 ").unwrap();
    bytes[at..at + 10].copy_from_slice(b"/Root 1 1 ");
    let info = inspect(Arc::new(bytes), None).unwrap();
    assert_eq!((info.pages.len(), info.title.as_deref()), (1, Some("Kept")));
}

#[test]
fn lazy_structure_matches_the_compatibility_inspector() {
    // Each fixture as lopdf writes it, and with its objects in object streams.
    let packed = |bytes: &[u8]| {
        let doc = pdfcraft_cos::Document::open(Arc::new(bytes.to_vec())).unwrap();
        pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions { object_streams: true, ..Default::default() }).unwrap()
    };
    let fixtures = [ATTACHMENTS, LAYERS, DESTS].into_iter().flat_map(|bytes| {
        let mut original = Document::load_mem(bytes).unwrap();
        let mut plain = Vec::new();
        original.save_to(&mut plain).unwrap();
        let stm = packed(&plain);
        [plain, stm]
    });
    for bytes in fixtures {
        let lazy = LazyStructure::new(Arc::new(bytes.clone()), None).unwrap();
        let eager = Document::load_mem(&bytes).unwrap();
        let mut a = inspect(Arc::new(bytes), None).unwrap();
        let mut b = DocInfo { pages: a.pages.clone(), ..Default::default() };
        let mut c = DocInfo { pages: a.pages.clone(), ..Default::default() };
        Inspector::new(&lazy).fill(&mut b);
        Inspector::new(&eager).fill(&mut c);
        assert!(!lazy.failed());
        assert_eq!(format!("{b:?}"), format!("{c:?}"));
        a.file_size = 0;
        a.pdf_version.clear();
        assert_eq!(format!("{a:?}"), format!("{c:?}"));
    }
}

#[test]
fn links_read_set_layer_actions() {
    let info = inspect(Arc::new(LAYERS.to_vec()), None).expect("opens");
    let targets: Vec<_> = info.links.iter().map(|l| l.target.clone()).collect();
    use LayerOp::{Off, On, Toggle};
    assert_eq!(
        targets,
        [
            // Groups before the first name or after an unknown one are skipped, and so is
            // anything that isn't a group.
            LinkTarget::SetLayers { changes: vec![(On, (6, 0)), (Toggle, (5, 0)), (Toggle, (7, 0))], preserve_rb: false },
            // An indirect /State; /PreserveRB defaults to true.
            LinkTarget::SetLayers { changes: vec![(Off, (5, 0))], preserve_rb: true },
        ]
    );
}

/// #4168: a Launch action naming a `.pdf` file is read as the link to that file it amounts
/// to, `\` separators included, and a stray `/D` (which Launch has no use for) doesn't stop
/// it. One naming anything else, with no file, or with `/Win` launch parameters stays an
/// action that isn't followed.
#[test]
fn launch_actions_that_name_a_pdf_open_it_like_a_remote_link() {
    let pdf = "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /Annots [30 0 R 31 0 R 32 0 R 33 0 R 34 0 R 35 0 R 36 0 R 37 0 R 38 0 R] >> endobj\n\
             30 0 obj << /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A << /S /Launch /F (..\\\\se\\\\se_ST.pdf) >> >> endobj\n\
             31 0 obj << /Type /Annot /Subtype /Link /Rect [0 20 10 30] /A << /S /Launch /F << /Type /Filespec /F (chapters/Two.PDF) >> /NewWindow true >> >> endobj\n\
             32 0 obj << /Type /Annot /Subtype /Link /Rect [0 40 10 50] /A << /S /Launch /F (tool.exe) >> >> endobj\n\
             33 0 obj << /Type /Annot /Subtype /Link /Rect [0 60 10 70] /A << /S /Launch /F (a.pdf) /Win << /F (cmd.exe) /P (/c x) >> >> >> endobj\n\
             34 0 obj << /Type /Annot /Subtype /Link /Rect [0 80 10 90] /A << /S /Launch /Win << /F (b.pdf) >> >> >> endobj\n\
             35 0 obj << /Type /Annot /Subtype /Link /Rect [0 100 10 110] /A << /S /Launch /F (.pdf) >> >> endobj\n\
             36 0 obj << /Type /Annot /Subtype /Link /Rect [0 120 10 130] /A << /S /Launch /F (dir/a\\\\b.pdf) >> >> endobj\n\
             37 0 obj << /Type /Annot /Subtype /Link /Rect [0 140 10 150] /A << /S /Launch /FS /URL /F (https://example.com/x.pdf) >> >> endobj\n\
             38 0 obj << /Type /Annot /Subtype /Link /Rect [0 160 10 170] /A << /S /Launch /F (c.pdf) /D 0 >> >> endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF";
    let info = inspect(Arc::new(pdf.as_bytes().to_vec()), None).expect("opens");
    let targets: Vec<_> = info.links.iter().map(|l| l.target.clone()).collect();
    let launch =
        |file: &str, new_window| LinkTarget::RemoteGoTo { launch: true, file: file.into(), destination: RemoteDestination::Default, new_window };
    let other = || LinkTarget::Other("Launch".into());
    assert_eq!(
        targets,
        [
            launch("../se/se_ST.pdf", None),
            launch("chapters/Two.PDF", Some(true)),
            other(),
            other(),
            other(),
            other(),
            launch("dir/a\\b.pdf", None),
            other(),
            launch("c.pdf", None),
        ]
    );
}

#[test]
fn links_read_bounded_local_remote_go_to_actions() {
    use DestView::{Fit, Xyz};
    let too_long = "a".repeat(MAX_REMOTE_FILE_SPEC_BYTES + 1);
    let wide_file = format!("<FEFF{}>", "4E00".repeat(1400));
    let wide_name = format!("<FEFF{}>", "4E00".repeat(342));
    let pdf = format!(
        "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /Annots [99 0 R 30 0 R 31 0 R 32 0 R 33 0 R 34 0 R 35 0 R 36 0 R 37 0 R 38 0 R 39 0 R 40 0 R 41 0 R] >> endobj\n\
             30 0 obj << /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A << /S /GoToR /F (chapters/remote.pdf) /D [2 /XYZ 10 20 1.5] /NewWindow true >> >> endobj\n\
             31 0 obj << /Type /Annot /Subtype /Link /Rect [0 20 10 30] /A << /S /GoToR /F << /Type /Filespec /F (fallback.pdf) /UF <FEFF0066006F006F002E007000640066> >> /D [0 /Fit] /NewWindow false >> >> endobj\n\
             32 0 obj << /Type /Annot /Subtype /Link /Rect [0 40 10 50] /A << /S /GoToR /F (remote.pdf) >> >> endobj\n\
             33 0 obj << /Type /Annot /Subtype /Link /Rect [0 60 10 70] /A << /S /GoToR /F (chapter.pdf) /D (chapter) >> >> endobj\n\
             34 0 obj << /Type /Annot /Subtype /Link /Rect [0 80 10 90] /A << /S /GoToR /FS /URL /F (https://example.com) /D [0 /Fit] >> >> endobj\n\
             35 0 obj << /Type /Annot /Subtype /Link /Rect [0 100 10 110] /A << /S /GoToR /F ({too_long}) /D [0 /Fit] >> >> endobj\n\
             36 0 obj << /Type /Annot /Subtype /Link /Rect [0 120 10 130] /A << /S /GoToR /F (chapter.pdf) /D /chapter >> >> endobj\n\
             37 0 obj << /Type /Annot /Subtype /Link /Rect [0 140 10 150] /A << /S /GoToR /F (chapter.pdf) /D [-1 /Fit] >> >> endobj\n\
             38 0 obj << /Type /Annot /Subtype /Link /Rect [0 160 10 170] /A << /S /GoToR /F (chapter.pdf) /D [1.5 /Fit] >> >> endobj\n\
             39 0 obj << /Type /Annot /Subtype /Link /Rect [0 180 10 190] /A << /S /GoToR /F (chapter.pdf) /D [9223372036854775807 /Fit] >> >> endobj\n\
             40 0 obj << /Type /Annot /Subtype /Link /Rect [0 200 10 210] /A << /S /GoToR /F << /Type /Filespec /UF {wide_file} >> /D [0 /Fit] >> >> endobj\n\
             41 0 obj << /Type /Annot /Subtype /Link /Rect [0 220 10 230] /A << /S /GoToR /F (chapter.pdf) /D {wide_name} >> >> endobj\n\
             trailer << /Root 1 0 R >>\n%%EOF"
    );
    let info = inspect(Arc::new(pdf.into_bytes()), None).expect("opens");
    let targets: Vec<_> = info.links.iter().map(|l| (l.index, l.target.clone())).collect();
    let huge_page_target = usize::try_from(i64::MAX)
        .map(|page| LinkTarget::RemoteGoTo {
            launch: false,
            file: "chapter.pdf".into(),
            destination: RemoteDestination::Page { page, view: Fit },
            new_window: None,
        })
        .unwrap_or_else(|_| LinkTarget::Other("GoToR".into()));
    assert_eq!(
        targets,
        [
            (
                1,
                LinkTarget::RemoteGoTo {
                    launch: false,
                    file: "chapters/remote.pdf".into(),
                    destination: RemoteDestination::Page { page: 2, view: Xyz { left: Some(10.0), top: Some(20.0), zoom: Some(1.5) } },
                    new_window: Some(true),
                }
            ),
            (
                2,
                LinkTarget::RemoteGoTo {
                    launch: false,
                    file: "foo.pdf".into(),
                    destination: RemoteDestination::Page { page: 0, view: Fit },
                    new_window: Some(false),
                }
            ),
            (3, LinkTarget::RemoteGoTo { launch: false, file: "remote.pdf".into(), destination: RemoteDestination::Default, new_window: None }),
            (
                4,
                LinkTarget::RemoteGoTo {
                    launch: false,
                    file: "chapter.pdf".into(),
                    destination: RemoteDestination::Named("chapter".into()),
                    new_window: None
                }
            ),
            (5, LinkTarget::Other("GoToR".into())),
            (6, LinkTarget::Other("GoToR".into())),
            (
                7,
                LinkTarget::RemoteGoTo {
                    launch: false,
                    file: "chapter.pdf".into(),
                    destination: RemoteDestination::Named("chapter".into()),
                    new_window: None
                }
            ),
            (8, LinkTarget::Other("GoToR".into())),
            (9, LinkTarget::Other("GoToR".into())),
            (10, huge_page_target),
            (11, LinkTarget::Other("GoToR".into())),
            (12, LinkTarget::Other("GoToR".into())),
        ]
    );
}

#[test]
fn destination_operands_become_views() {
    use DestView::*;
    let v = |kind: &[u8], ops: &[Option<f32>]| DestView::from_operands(Some(kind), ops);
    // ISO 32000-2 Table 149, each type.
    assert_eq!(v(b"XYZ", &[Some(0.0), Some(420.0), Some(0.0)]), Xyz { left: Some(0.0), top: Some(420.0), zoom: None });
    assert_eq!(v(b"XYZ", &[Some(72.0), Some(500.0), Some(1.5)]), Xyz { left: Some(72.0), top: Some(500.0), zoom: Some(1.5) });
    assert_eq!(v(b"Fit", &[]), Fit);
    assert_eq!(v(b"FitB", &[]), Fit);
    assert_eq!(v(b"FitH", &[Some(250.0)]), FitH { top: Some(250.0) });
    assert_eq!(v(b"FitBH", &[None]), FitH { top: None });
    assert_eq!(v(b"FitV", &[Some(120.0)]), FitV { left: Some(120.0) });
    assert_eq!(v(b"FitBV", &[Some(5.0)]), FitV { left: Some(5.0) });
    // FitR's corners are normalized.
    assert_eq!(v(b"FitR", &[Some(110.0), Some(220.0), Some(10.0), Some(20.0)]), FitR { rect: [10.0, 20.0, 110.0, 220.0] });
    // Missing operands are null; so are non-finite ones, and a zoom that isn't positive.
    assert_eq!(v(b"XYZ", &[]), Xyz { left: None, top: None, zoom: None });
    assert_eq!(v(b"XYZ", &[Some(f32::NAN), Some(f32::INFINITY), Some(f32::NEG_INFINITY)]), Xyz { left: None, top: None, zoom: None });
    assert_eq!(v(b"XYZ", &[None, Some(-5.0), Some(-1.0)]), Xyz { left: None, top: Some(-5.0), zoom: None });
    assert_eq!(v(b"XYZ", &[None, None, Some(f32::NAN)]), Xyz { left: None, top: None, zoom: None });
    assert_eq!(v(b"FitH", &[Some(f32::NAN)]), FitH { top: None });
    // A FitR without four usable numbers, an unknown type or no type: the top of the page.
    assert_eq!(v(b"FitR", &[Some(1.0), Some(2.0)]), Top);
    assert_eq!(v(b"FitR", &[Some(1.0), Some(2.0), Some(f32::INFINITY), Some(4.0)]), Top);
    assert_eq!(v(b"Zoom", &[Some(3.0)]), Top);
    assert_eq!(v(b"xyz", &[Some(3.0)]), Top);
    assert_eq!(DestView::from_operands(None, &[Some(1.0)]), Top);
}

/// Two 300×400 pages. Bookmarks and links on page 1 go to page 2 in every destination form,
/// named (`/Names /Dests` and the PDF 1.1 `/Dests`) and malformed.
const DESTS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Outlines 10 0 R /Names << /Dests 40 0 R >> /Dests << /old << /D [4 0 R /FitV 120] >> >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [30 0 R 31 0 R 32 0 R 33 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
10 0 obj << /Type /Outlines /First 11 0 R /Last 20 0 R /Count 10 >> endobj
11 0 obj << /Title (xyz) /Parent 10 0 R /Next 12 0 R /Dest [4 0 R /XYZ 0 420 0] >> endobj
12 0 obj << /Title (fith) /Parent 10 0 R /Next 13 0 R /Dest [4 0 R /FitH 250] >> endobj
13 0 obj << /Title (named) /Parent 10 0 R /Next 14 0 R /Dest (sec) >> endobj
14 0 obj << /Title (old named) /Parent 10 0 R /Next 15 0 R /Dest /old >> endobj
15 0 obj << /Title (action) /Parent 10 0 R /Next 16 0 R /A << /S /GoTo /D [4 0 R /FitR 110 220 10 20] >> >> endobj
16 0 obj << /Title (wrong types) /Parent 10 0 R /Next 17 0 R /Dest [4 0 R /XYZ (left) /Top true] >> endobj
17 0 obj << /Title (short) /Parent 10 0 R /Next 18 0 R /Dest [4 0 R /FitR 1 2] >> endobj
18 0 obj << /Title (no type) /Parent 10 0 R /Next 19 0 R /Dest [4 0 R] >> endobj
19 0 obj << /Title (huge) /Parent 10 0 R /Next 20 0 R /Dest [4 0 R /XYZ 1000000000000 -5 -1] >> endobj
20 0 obj << /Title (no page) /Parent 10 0 R /Dest [/XYZ 0 420 0] >> endobj
30 0 obj << /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Dest [4 0 R /XYZ 72 500 1.5] >> endobj
31 0 obj << /Type /Annot /Subtype /Link /Rect [0 20 10 30] /A << /S /GoTo /D (sec) >> >> endobj
32 0 obj << /Type /Annot /Subtype /Link /Rect [0 40 10 50] /Dest [4 0 R /FitBH 100] >> endobj
33 0 obj << /Type /Annot /Subtype /Link /Rect [0 60 10 70] /Dest [4 0 R /Fit] >> endobj
40 0 obj << /Names [(sec) << /D [4 0 R /XYZ null 300 2] >>] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn bookmarks_and_links_keep_their_destination_view() {
    use DestView::*;
    let info = inspect(Arc::new(DESTS.to_vec()), None).expect("opens");
    let outline: Vec<_> = info.outline.iter().map(|o| (o.title.as_str(), o.page, o.view)).collect();
    assert_eq!(
        outline,
        [
            ("xyz", Some(1), Xyz { left: Some(0.0), top: Some(420.0), zoom: None }),
            ("fith", Some(1), FitH { top: Some(250.0) }),
            ("named", Some(1), Xyz { left: None, top: Some(300.0), zoom: Some(2.0) }),
            ("old named", Some(1), FitV { left: Some(120.0) }),
            ("action", Some(1), FitR { rect: [10.0, 20.0, 110.0, 220.0] }),
            // Hostile operands fall back without failing the bookmark.
            ("wrong types", Some(1), Xyz { left: None, top: None, zoom: None }),
            ("short", Some(1), Top),
            ("no type", Some(1), Top),
            ("huge", Some(1), Xyz { left: Some(1e12), top: Some(-5.0), zoom: None }),
            ("no page", None, Top),
        ]
    );
    let links: Vec<_> = info.links.iter().map(|l| l.target.clone()).collect();
    assert_eq!(
        links,
        [
            LinkTarget::Page(1, Xyz { left: Some(72.0), top: Some(500.0), zoom: Some(1.5) }),
            LinkTarget::Page(1, Xyz { left: None, top: Some(300.0), zoom: Some(2.0) }),
            LinkTarget::Page(1, FitH { top: Some(100.0) }),
            LinkTarget::Page(1, Fit),
        ]
    );
}

#[test]
fn outline_titles_decode_every_text_string_encoding() {
    // Issue #142: CJK bookmark titles showed as mojibake or with a stray BOM.
    let pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Outlines 10 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
10 0 obj << /Type /Outlines /First 11 0 R /Last 16 0 R /Count 6 >> endobj
11 0 obj << /Title <FEFF7B2C4E007AE0> /Parent 10 0 R /Next 12 0 R /Dest [3 0 R /Fit] >> endobj
12 0 obj << /Title <EFBBBFE79BAEE5BD95> /Parent 10 0 R /Next 13 0 R /Dest [3 0 R /Fit] >> endobj
13 0 obj << /Title <EFBBBF41FF42> /Parent 10 0 R /Next 14 0 R /Dest [3 0 R /Fit] >> endobj
14 0 obj << /Title <E6A682E8BFB0> /Parent 10 0 R /Next 15 0 R /Dest [3 0 R /Fit] >> endobj
15 0 obj << /Title <436166E9> /Parent 10 0 R /Next 16 0 R /Dest [3 0 R /Fit] >> endobj
16 0 obj << /Title <FEFFFEFF0041> /Parent 10 0 R /Dest [3 0 R /Fit] >> endobj
trailer << /Root 1 0 R >>
%%EOF";
    let info = inspect(Arc::new(pdf.to_vec()), None).expect("opens");
    let titles: Vec<_> = info.outline.iter().map(|o| o.title.as_str()).collect();
    // UTF-16BE, UTF-8 with BOM, invalid UTF-8 after a BOM (lossy, not dropped), BOM-less
    // raw UTF-8, PDFDocEncoding Latin-1, and a doubled BOM.
    assert_eq!(titles, ["第一章", "目录", "A\u{FFFD}B", "概述", "Café", "A"]);
}

#[test]
fn xfa_dynamic_render_is_read_from_config_acrobat7_only() {
    let text = xfa_config_dynamic_render;
    let config =
        |inner: &str| format!("<config xmlns=\"http://www.xfa.org/schema/xci/3.0/\"><acrobat><acrobat7>{inner}</acrobat7></acrobat></config>");
    // Designer's style: a line break before each `>`.
    assert_eq!(
        text("<config\n><acrobat\n><acrobat7\n><dynamicRender\n>forbidden</dynamicRender\n></acrobat7\n></acrobat\n></config\n>").as_deref(),
        Some("forbidden")
    );
    assert_eq!(text(&config("<xci:dynamicRender> required </xci:dynamicRender>")).as_deref(), Some("required"));
    // Commented out, in CDATA, or a longer name: not the element.
    assert_eq!(text(&config("<!-- a > b <dynamicRender>forbidden</dynamicRender> -->")), None);
    assert_eq!(text(&config("<![CDATA[ a > b <dynamicRender>forbidden</dynamicRender> ]]>")), None);
    assert_eq!(text(&config("<dynamicRenderX>forbidden</dynamicRenderX>")), None);
    // Self-closing, empty, or holding markup: no value.
    assert_eq!(text(&config("<dynamicRender/>")), None);
    assert_eq!(text(&config("<dynamicRender></dynamicRender>")), None);
    assert_eq!(text(&config("<dynamicRender><b>forbidden</b></dynamicRender>")), None);
    // Closing tags that don't match what they close: malformed, no value (review on #3192).
    assert_eq!(text(&config("<dynamicRender>forbidden</other>")), None);
    assert_eq!(text("<config><acrobat><acrobat7><dynamicRender>forbidden</dynamicRender></acrobat></acrobat7></config>"), None);
    // The same name anywhere but config/acrobat/acrobat7 (form data, another config branch).
    let xdp = "<xdp:xdp><xfa:datasets><xfa:data><dynamicRender>forbidden</dynamicRender></xfa:data></xfa:datasets><config><present><dynamicRender>forbidden</dynamicRender></present></config></xdp:xdp>";
    assert_eq!(text(xdp), None);
    // A whole XDP with the config after the template.
    let xdp = format!("<xdp:xdp><template><subform/></template>{}</xdp:xdp>", config("<dynamicRender>forbidden</dynamicRender>"));
    assert_eq!(text(&xdp).as_deref(), Some("forbidden"));
    // A whole config path inside the form data, before the real config packet, is data: the
    // real packet's value counts (review on #3192).
    let xdp = format!(
        "<?xml version=\"1.0\"?><xdp:xdp><xfa:datasets><xfa:data>{}</xfa:data></xfa:datasets>{}</xdp:xdp>",
        config("<dynamicRender>forbidden</dynamicRender>"),
        config("<dynamicRender>required</dynamicRender>")
    );
    assert_eq!(text(&xdp).as_deref(), Some("required"));
    // Only in the data, or nested in the template: no configuration at all.
    let xdp = format!("<xdp:xdp><xfa:datasets><xfa:data>{}</xfa:data></xfa:datasets></xdp:xdp>", config("<dynamicRender>forbidden</dynamicRender>"));
    assert_eq!(text(&xdp), None);
    let xdp = format!("<xdp:xdp><template>{}</template></xdp:xdp>", config("<dynamicRender>forbidden</dynamicRender>"));
    assert_eq!(text(&xdp), None);
    // acrobat7 must be a child of acrobat, not deeper.
    assert_eq!(text("<config><acrobat><x><acrobat7><dynamicRender>forbidden</dynamicRender></acrobat7></x></acrobat></config>"), None);
    // Cut short anywhere: no panic, nothing read.
    let whole = config("<!-- x --><dynamicRender>forbidden</dynamicRender>");
    for end in 0..whole.len() {
        assert_eq!(text(&whole[..end]), None, "cut at {end}");
    }
    assert_eq!(text(&whole).as_deref(), Some("forbidden"));
}

#[test]
fn xfa_config_packets_are_read_in_utf16_too() {
    let xml = "<config><acrobat><acrobat7><dynamicRender>forbidden</dynamicRender></acrobat7></acrobat></config>";
    let le: Vec<u8> = xml.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let be: Vec<u8> = xml.encode_utf16().flat_map(u16::to_be_bytes).collect();
    for bytes in [
        le.clone(),
        be.clone(),
        [&[0xFF, 0xFE][..], &le].concat(),
        [&[0xFE, 0xFF][..], &be].concat(),
        [&[0xEF, 0xBB, 0xBF][..], xml.as_bytes()].concat(),
    ] {
        assert_eq!(xfa_packet_text(&bytes), xml);
    }
    // An odd byte or a broken surrogate doesn't stop the scan.
    assert!(xfa_packet_text(&[0xFF, 0xFE, b'<', 0, 0x00, 0xD8, b'x']).starts_with('<'));
}

/// #3180: an XFA form with no fields was always taken to be dynamic, so its pages were
/// replaced by a layout on open, even when its configuration says it is a static form.
#[test]
fn an_xfa_form_without_fields_is_static_when_its_config_forbids_dynamic_rendering() {
    let xfa_of = |config: &str, catalog_extra: &str| {
        let bodies = [
            format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} /AcroForm << /Fields [] /XFA [(config) 4 0 R (template) 5 0 R] >> >>"),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".to_string(),
            "<< /Type /Page /Parent 2 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{config}\nendstream", config.len()),
            "<< /Length 11 >>\nstream\n<template/>\nendstream".to_string(),
        ];
        inspect(Arc::new(pdf_of(&bodies)), None).expect("opens").xfa
    };
    let config = |value: &str| {
        format!(
            r#"<config xmlns="http://www.xfa.org/schema/xci/3.0/"><acrobat><acrobat7><dynamicRender>{value}</dynamicRender></acrobat7></acrobat></config>"#
        )
    };
    assert_eq!(xfa_of(&config("forbidden"), ""), Some(Xfa::Static));
    assert_eq!(xfa_of(&config("required"), ""), Some(Xfa::Dynamic));
    assert_eq!(xfa_of("<config/>", ""), Some(Xfa::Dynamic), "no field and no word from the config: dynamic, as before");
    assert_eq!(xfa_of(&config("forbidden"), "/NeedsRendering true"), Some(Xfa::Dynamic), "/NeedsRendering still wins");
}

/// A file whose objects are `bodies` in order (object 1 first), with a cross-reference table.
fn pdf_of(bodies: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
    out
}

/// A one-page file whose outline is `items` (objects 5, 6, … in order).
fn outline_file(items: &[String]) -> Vec<u8> {
    let mut bodies = vec![
        "<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R >>".to_string(),
        format!("<< /Type /Outlines /First 5 0 R /Count {} >>", items.len()),
    ];
    bodies.extend(items.iter().cloned());
    pdf_of(&bodies)
}

/// The titles of `items`, with children in brackets (`Parent[Child]`).
fn shape(items: &[OutlineItem]) -> Vec<String> {
    items.iter().map(|o| if o.children.is_empty() { o.title.clone() } else { format!("{}[{}]", o.title, shape(&o.children).join(",")) }).collect()
}

/// #3193: a bookmark's colour and style (ISO 32000-2 §12.3.3, Tables 151 and 152). `/C` is
/// three numbers within 0–1 (one outside is kept at the edge); black, the default, and a `/C`
/// that isn't three numbers read as no colour. `/F` bit 1 is italic, bit 2 bold.
#[test]
fn outline_items_carry_their_colour_and_style() {
    let titles = [
        ("Plain", ""),
        ("Red", "/C [1 0 0]"),
        ("Italic", "/F 1"),
        ("Bold", "/F 2"),
        ("Both", "/C [0 0 1] /F 3"),
        ("Black", "/C [0 0 0]"),
        ("Bright", "/C [2 -1 0.5]"),
        ("Short", "/C [1 0]"),
        ("Long", "/C [1 0 0 0]"),
        ("Named", "/C [1 /x 0]"),
        ("Other bits", "/F 4"),
    ];
    let n = titles.len();
    let items: Vec<String> = titles
        .iter()
        .enumerate()
        .map(|(k, (title, extra))| {
            let prev = if k == 0 { String::new() } else { format!(" /Prev {} 0 R", 4 + k) };
            let next = if k + 1 == n { String::new() } else { format!(" /Next {} 0 R", 6 + k) };
            format!("<< /Title ({title}) /Parent 4 0 R{prev}{next} /Dest [3 0 R /Fit] {extra} >>")
        })
        .collect();
    let info = inspect(Arc::new(outline_file(&items)), None).expect("opens");
    let got: Vec<(&str, Option<[f32; 3]>, bool, bool)> = info.outline.iter().map(|o| (o.title.as_str(), o.color, o.italic, o.bold)).collect();
    assert_eq!(
        got,
        [
            ("Plain", None, false, false),
            ("Red", Some([1.0, 0.0, 0.0]), false, false),
            ("Italic", None, true, false),
            ("Bold", None, false, true),
            ("Both", Some([0.0, 0.0, 1.0]), true, true),
            ("Black", None, false, false),
            ("Bright", Some([1.0, 0.0, 0.5]), false, false),
            ("Short", None, false, false),
            ("Long", None, false, false),
            ("Named", None, false, false),
            ("Other bits", None, false, false),
        ]
    );
}

#[test]
fn outline_window_keeps_the_first_bookmarks_and_says_when_more_follow() {
    const COUNT: usize = OUTLINE_WINDOW + 5;
    let items: Vec<String> = (0..COUNT)
        .map(|k| {
            let prev = if k == 0 { String::new() } else { format!(" /Prev {} 0 R", 4 + k) };
            let next = if k + 1 == COUNT { String::new() } else { format!(" /Next {} 0 R", 6 + k) };
            format!("<< /Title (Item {k}) /Parent 4 0 R{prev}{next} /Dest [3 0 R /Fit] >>")
        })
        .collect();
    let info = inspect(Arc::new(outline_file(&items)), None).expect("opens");
    assert_eq!(info.outline.len(), OUTLINE_WINDOW);
    assert_eq!(info.outline.last().map(|o| o.title.as_str()), Some("Item 9999"));
    assert!(info.outline_more);

    let one = inspect(Arc::new(outline_file(&["<< /Title (Only) /Parent 4 0 R /Dest [3 0 R /Fit] >>".into()])), None).expect("opens");
    assert_eq!(shape(&one.outline), ["Only"]);
    assert!(!one.outline_more);
}

#[test]
fn outline_window_nests_no_deeper_than_its_depth_limit() {
    // A chain of 40 bookmarks, each the first child of the one before.
    let items: Vec<String> = (0..40)
        .map(|k| {
            let parent = if k == 0 { 4 } else { 4 + k };
            let first = if k + 1 == 40 { String::new() } else { format!(" /First {} 0 R", 6 + k) };
            format!("<< /Title (Deep {k}) /Parent {parent} 0 R{first} /Dest [3 0 R /Fit] >>")
        })
        .collect();
    let info = inspect(Arc::new(outline_file(&items)), None).expect("opens");
    let mut levels = 0;
    let mut level = info.outline.first();
    while let Some(item) = level {
        levels += 1;
        level = item.children.first();
    }
    assert_eq!(levels, OUTLINE_DEPTH + 1);
    assert!(info.outline_more);
}

#[test]
fn outline_window_ends_cycles_and_broken_links_without_looping() {
    let listed = |items: &[&str]| {
        let items: Vec<String> = items.iter().map(|s| s.to_string()).collect();
        shape(&inspect(Arc::new(outline_file(&items)), None).expect("opens").outline)
    };
    assert_eq!(listed(&["<< /Title (Self) /Parent 4 0 R /Next 5 0 R >>"]), ["Self"]);
    assert_eq!(listed(&["<< /Title (Self) /Parent 4 0 R /First 5 0 R >>"]), ["Self"]);
    assert_eq!(listed(&["<< /Title (A) /Parent 4 0 R /Next 6 0 R >>", "<< /Title (B) /Parent 4 0 R /Next 5 0 R >>"]), ["A", "B"]);
    assert_eq!(listed(&["<< /Title (A) /Parent 4 0 R /First 6 0 R >>", "<< /Title (B) /Parent 5 0 R /First 5 0 R >>"]), ["A[B]"]);
    assert_eq!(listed(&["<< /Title (A) /Parent 4 0 R /Next 99 0 R >>"]), ["A"]);
    assert_eq!(listed(&["<< /Title (A) /Parent 4 0 R /Next 6 0 R >>", "42"]), ["A"]);
}

#[test]
fn destination_points_map_to_the_displayed_page() {
    // A crop box away from the origin: x 100..400, y 50..450.
    let mut p = PageInfo { width: 300.0, height: 400.0, label: "1".into(), crop: [100.0, 50.0, 400.0, 450.0], rotation: 0 };
    assert_eq!(p.dest_fraction(Some(100.0), Some(450.0), 0), [Some(0.0), Some(0.0)]);
    assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 0), [Some(0.5), Some(0.25)]);
    // Unspecified stays unspecified; off-page and huge values are clamped to the page.
    assert_eq!(p.dest_fraction(None, Some(350.0), 0), [None, Some(0.25)]);
    assert_eq!(p.dest_fraction(Some(-1e30), Some(1e30), 0), [Some(0.0), Some(0.0)]);
    assert_eq!(p.dest_fraction(Some(f32::INFINITY), Some(f32::NAN), 0), [None, None]);
    // /Rotate 90 (clockwise): user y runs left to right across the displayed page, user x
    // top to bottom. A view rotation adds to it.
    p.rotation = 90;
    assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 0), [Some(0.75), Some(0.5)]);
    assert_eq!(p.dest_fraction(None, Some(350.0), 0), [Some(0.75), None]);
    assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 90), [Some(0.5), Some(0.75)]);
    p.rotation = 180;
    assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 0), [Some(0.5), Some(0.75)]);
    p.rotation = 270;
    assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 0), [Some(0.25), Some(0.5)]);
    assert_eq!(p.dest_fraction(Some(250.0), Some(350.0), 90), [Some(0.5), Some(0.25)]);
    // A degenerate crop box never divides by zero.
    let flat = PageInfo { crop: [0.0, 0.0, 0.0, 0.0], rotation: 0, ..p };
    assert!(flat.dest_fraction(Some(5.0), Some(-5.0), 0).iter().all(|f| f.is_some_and(|f| (0.0..=1.0).contains(&f))));
}

#[test]
fn a_displayed_point_becomes_the_xyz_position_that_goes_back_to_it() {
    // Every rotation, on a crop box away from the origin: pinning a displayed point and
    // reading the destination back lands on the same fraction of the displayed page.
    for rotation in [0u16, 90, 180, 270] {
        let (width, height) = if rotation.is_multiple_of(180) { (300.0, 400.0) } else { (400.0, 300.0) };
        let p = PageInfo { width, height, label: "1".into(), crop: [100.0, 50.0, 400.0, 450.0], rotation };
        let (x, y) = (0.25 * width, 0.75 * height);
        let near = |got: [Option<f32>; 2], want: [Option<f32>; 2]| {
            for (g, w) in got.iter().zip(want) {
                match (g, w) {
                    (Some(g), Some(w)) => assert!((g - w).abs() < 1e-4, "{rotation}: {got:?} vs {want:?}"),
                    (None, None) => {}
                    _ => panic!("{rotation}: {got:?} vs {want:?}"),
                }
            }
        };
        let [l, t] = p.xyz_for_view(Some(x), Some(y));
        assert!(l.is_some() && t.is_some(), "{rotation}: a point pins both");
        near(p.dest_fraction(l, t, 0), [Some(0.25), Some(0.75)]);
        // Only a height: the horizontal position is left alone, whichever user axis that is.
        let [l, t] = p.xyz_for_view(None, Some(y));
        assert_eq!((l.is_some(), t.is_some()), if rotation.is_multiple_of(180) { (false, true) } else { (true, false) }, "{rotation}");
        near(p.dest_fraction(l, t, 0), [None, Some(0.75)]);
        let [l, t] = p.xyz_for_view(Some(x), None);
        near(p.dest_fraction(l, t, 0), [Some(0.25), None]);
        assert_eq!(p.xyz_for_view(None, None), [None, None]);
        assert_eq!(p.xyz_for_view(Some(f32::NAN), Some(f32::INFINITY)), [None, None], "not finite is not given");
    }
    // The top of an upright page is the crop box's top edge.
    let p = PageInfo { width: 300.0, height: 400.0, label: "1".into(), crop: [100.0, 50.0, 400.0, 450.0], rotation: 0 };
    assert_eq!(p.xyz_for_view(None, Some(0.0)), [None, Some(450.0)]);
    assert_eq!(p.xyz_for_view(None, Some(100.0)), [None, Some(350.0)]);
}

#[test]
fn structure_failure_preserves_renderer_pages() {
    for crash in [true, false] {
        let mut info = DocInfo {
            file_size: 321,
            pdf_version: "1.7".into(),
            pages: vec![PageInfo { width: 300.0, height: 200.0, label: "1".into(), crop: [10.0, 20.0, 210.0, 320.0], rotation: 90 }],
            ..Default::default()
        };
        inspect_structure(&mut info, |tmp| {
            tmp.pages.clear();
            tmp.title = Some("partially inspected".into());
            if crash {
                panic!("synthetic structure inspection failure");
            }
            Err("synthetic structure inspection failure".into())
        });

        assert_eq!(info.pages.len(), 1);
        let page = &info.pages[0];
        assert_eq!((page.width, page.height, page.label.as_str(), page.crop, page.rotation), (300.0, 200.0, "1", [10.0, 20.0, 210.0, 320.0], 90));
        assert_eq!(info.file_size, 321);
        assert_eq!(info.pdf_version, "1.7");
        assert!(info.title.is_none());
        assert_eq!(info.warnings.len(), 1);
        assert!(info.warnings[0].contains("synthetic structure inspection failure"));
        assert_eq!(info.warnings[0].contains("crashed"), crash);
    }
}

#[test]
fn invalid_dates_with_unicode_are_unchanged() {
    for input in ["", "D:", "D:20260930104", "D:202609x01045", "yesterday"] {
        assert_eq!(pretty_date(input), input);
    }
    // Cover every position before the 12-byte prefix, including characters that
    // straddle its end. None of these strings is an ASCII PDF date.
    for character in ['é', '€', '😀'] {
        for prefix_len in 0..12 {
            let input = format!("D:{}{character}123456789012", "1".repeat(prefix_len));
            assert_eq!(pretty_date(&input), input, "{character} after {prefix_len} digits");
        }
    }
}

#[test]
fn unicode_annotation_date_does_not_prevent_opening() {
    use lopdf::dictionary;

    let date = "D:12345678901éX";
    let mut encoded_date = vec![0xFE, 0xFF];
    encoded_date.extend(date.encode_utf16().flat_map(u16::to_be_bytes));
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let annot_id = doc.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Text",
        "Rect" => vec![10.into(), 10.into(), 30.into(), 30.into()],
        "M" => Object::String(encoded_date, lopdf::StringFormat::Hexadecimal),
    });
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 200.into(), 300.into()],
        "Annots" => vec![annot_id.into()],
    });
    doc.objects.insert(
        pages_id,
        dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        }
        .into(),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).expect("write synthetic fixture");

    let info = inspect(Arc::new(bytes), None).expect("opens despite a non-date /M string");
    assert_eq!(info.pages.len(), 1);
    assert_eq!((info.pages[0].width, info.pages[0].height), (200.0, 300.0));
    assert_eq!(info.annotations.len(), 1);
    assert_eq!(info.annotations[0].modified.as_deref(), Some(date));
    assert!(info.warnings.is_empty(), "{:?}", info.warnings);
}

#[test]
fn carriage_returns_in_comment_text_become_line_breaks() {
    use lopdf::dictionary;

    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let annot_id = doc.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Text",
        "Rect" => vec![10.into(), 10.into(), 30.into(), 30.into()],
        // Acrobat ends lines in /Contents with CR; the interface only breaks lines at LF.
        "Contents" => Object::string_literal("one\r\rtwo\r\nthree"),
    });
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 200.into(), 300.into()],
        "Annots" => vec![annot_id.into()],
    });
    doc.objects.insert(pages_id, dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 }.into());
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).expect("write synthetic fixture");

    let info = inspect(Arc::new(bytes), None).expect("opens");
    assert_eq!(info.annotations[0].contents.as_deref(), Some("one\n\ntwo\nthree"));
}

#[test]
fn dates_are_prettified() {
    assert_eq!(pretty_date("D:20260930104512-04'00'"), "2026-09-30 10:45");
    assert_eq!(pretty_date("yesterday"), "yesterday");
}

#[test]
fn jspdf_high_precision_mediabox_does_not_overflow_dimensions() {
    let pdf = b"%PDF-1.3
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 3874.9606299212600788 5493.5433070866147318] >> endobj
trailer << /Root 1 0 R >>
%%EOF";
    let info = inspect(std::sync::Arc::new(pdf.to_vec()), None).expect("opens");
    assert_eq!(info.pages.len(), 1);
    let p = &info.pages[0];
    assert!((p.width - 3874.96).abs() < 0.1, "width expected ~3874.96, got {}", p.width);
    assert!((p.height - 5493.54).abs() < 0.1, "height expected ~5493.54, got {}", p.height);
}
