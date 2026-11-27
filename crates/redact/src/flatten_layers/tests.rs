use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object, SaveOptions, write_full};
use pdfcraft_testkit::{Pdf, stream_obj as stream};

use super::*;

/// One page drawing a line in no layer, one in the layer "Notes" (5) and one in the layer
/// "Draft" (6), with extra objects from 7 on. By default Notes is visible and Draft hidden.
struct Layered {
    /// The default configuration, `/D`.
    d: &'static str,
    notes: &'static str,
    draft: &'static str,
    /// The page's `/Properties`.
    props: &'static str,
    /// More of the page's resources.
    extra_res: &'static str,
    /// More of the page dictionary.
    page_extra: &'static str,
    /// More of the catalog.
    catalog_extra: &'static str,
    /// Object 4, the page's content stream.
    contents: Vec<u8>,
    extra: Vec<Vec<u8>>,
}

impl Default for Layered {
    fn default() -> Self {
        Layered {
            d: "<< /OFF [6 0 R] >>",
            notes: "<< /Type /OCG /Name (NotesLayer) >>",
            draft: "<< /Type /OCG /Name (DraftLayer) >>",
            props: "<< /L1 5 0 R /L2 6 0 R >>",
            extra_res: "",
            page_extra: "",
            catalog_extra: "",
            contents: stream("", b"5 5 m 6 6 l S /OC /L1 BDC 0 0 m 10 10 l S EMC /OC /L2 BDC 1 1 m 9 9 l S EMC"),
            extra: Vec::new(),
        }
    }
}

impl Layered {
    fn build(self) -> Document {
        let mut objs = vec![
            format!("<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R] /D {} >> {} >>", self.d, self.catalog_extra).into_bytes(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /Properties {} {} >> {} >>",
                self.props, self.extra_res, self.page_extra
            )
            .into_bytes(),
            self.contents,
            self.notes.as_bytes().to_vec(),
            self.draft.as_bytes().to_vec(),
        ];
        objs.extend(self.extra);
        Document::open(Arc::new(Pdf::new().objs(objs).build())).unwrap()
    }
}

fn layered(extra_res: &'static str, extra: Vec<Vec<u8>>) -> Document {
    Layered { extra_res, extra, ..Default::default() }.build()
}

fn saved(doc: &Document) -> (Vec<u8>, Document) {
    let bytes = write_full(doc, &SaveOptions { object_streams: false, ..Default::default() }).unwrap();
    (bytes.clone(), Document::open(Arc::new(bytes)).unwrap())
}

/// The saved file with every stream it holds decoded and appended, so text inside a
/// compressed stream can be looked for.
fn readable(bytes: &[u8], doc: &Document) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for n in doc.object_numbers() {
        if let Object::Stream(s) = &*doc.get(ObjRef::new(n, doc.generation(n))) {
            out.extend(s.decoded().unwrap_or_default());
        }
    }
    out
}

fn has(bytes: &[u8], what: &[u8]) -> bool {
    bytes.windows(what.len()).any(|w| w == what)
}

fn op_strings(ops: &[Op]) -> Vec<String> {
    ops.iter().map(|op| String::from_utf8_lossy(&serialize_ops(std::slice::from_ref(op))).trim().to_string()).collect()
}

/// The page's operators as text, one per line.
fn page_ops(doc: &Document) -> Vec<String> {
    let page = pdfcraft_model::pages(doc).remove(0);
    let (_, data) = crate::page_streams(doc, &page.dict, 0).unwrap();
    op_strings(&pdfcraft_content::Pieces::join(&data).parse().ops)
}

/// The page's annotations, resolved.
fn annots(doc: &Document) -> Vec<Dict> {
    annots_of(doc, &pdfcraft_model::pages(doc).remove(0).dict).iter().filter_map(|a| doc.resolve(a).as_dict().cloned()).collect()
}

/// Flattening is refused as `want` says, and the document saves as it did before.
fn refused_unchanged(mut doc: Document, want: fn(&RedactError) -> bool) {
    let before = saved(&doc).0;
    let got = flatten_layers(&mut doc);
    assert!(got.as_ref().err().is_some_and(want), "{got:?}");
    assert_eq!(saved(&doc).0, before, "nothing was changed");
}

/// The visible drawing stays, the hidden one goes, and no layer's block is left on the page.
fn merged_as_expected(doc: &Document) {
    let ops = page_ops(doc);
    assert!(ops.iter().any(|o| o == "10 10 l") && !ops.iter().any(|o| o == "9 9 l"), "{ops:?}");
    assert!(!ops.iter().any(|o| o.ends_with("BDC") || o == "EMC"), "{ops:?}");
}

#[test]
fn only_the_visible_layers_count_as_flattened() {
    let mut doc = layered("", Vec::new());
    assert_eq!(flatten_layers(&mut doc).unwrap(), Flattened { visible: 1, hidden: 1 });
}

#[test]
fn the_visible_layer_merges_and_the_hidden_one_goes() {
    let mut doc = layered("", Vec::new());
    flatten_layers(&mut doc).unwrap();
    let (bytes, doc) = saved(&doc);
    let ops = page_ops(&doc);
    assert!(ops.iter().any(|o| o == "6 6 l"), "content in no layer stays: {ops:?}");
    assert!(ops.iter().any(|o| o == "10 10 l"), "the visible layer's drawing stays: {ops:?}");
    assert!(!ops.iter().any(|o| o == "9 9 l"), "the hidden layer's drawing is gone: {ops:?}");
    assert!(!ops.iter().any(|o| o.ends_with("BDC") || o == "EMC"), "no layer's block is left: {ops:?}");
    assert!(!has(&bytes, b"/OCProperties") && !has(&bytes, b"NotesLayer") && !has(&bytes, b"DraftLayer"), "no layer is left in the file");
}

#[test]
fn xobjects_and_annotations_leave_their_layers() {
    let mut doc = layered(
        "/XObject << /FmV 8 0 R >>",
        vec![
            // 7: an image in the hidden layer, drawn only by an appearance.
            stream("/Type /XObject /Subtype /Image /Width 16 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /OC 6 0 R", b"HIDDENLAYERIMAGE"),
            // 8: a form in the visible layer.
            stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10] /OC 5 0 R", b"0 0 m 10 10 l S"),
            b"<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /OC 6 0 R /Contents (HIDDENNOTE) >>".to_vec(),
            b"<< /Type /Annot /Subtype /Widget /Rect [0 0 10 10] /OC 6 0 R /FT /Btn /T (w) >>".to_vec(),
            b"<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /AP << /N 12 0 R >> >>".to_vec(),
            // 12: an appearance in no layer drawing both layers and the hidden image.
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /Properties << /L1 5 0 R /L2 6 0 R >> /XObject << /ImH 7 0 R >> >>",
                b"/OC /L2 BDC (HIDDENAPTEXT) Tj EMC /OC /L1 BDC 2 2 m 3 3 l S EMC /ImH Do",
            ),
        ],
    );
    doc.update_dict(ObjRef::new(3, 0), |p| p.set(b"Annots".to_vec(), Object::Array((9..=11).map(|n| Object::Ref(ObjRef::new(n, 0))).collect())))
        .unwrap();
    flatten_layers(&mut doc).unwrap();
    let (bytes, saved) = saved(&doc);
    let bytes = readable(&bytes, &saved);
    for gone in [&b"HIDDENLAYERIMAGE"[..], b"HIDDENNOTE", b"HIDDENAPTEXT", b"/OC "] {
        assert!(!has(&bytes, gone), "{} survived", String::from_utf8_lossy(gone));
    }
    let annots = annots_of(&saved, &pdfcraft_model::pages(&saved).remove(0).dict);
    assert_eq!(annots.len(), 2, "the hidden comment leaves, the widget and the comment in no layer stay");
    let widget = annots.iter().filter_map(|a| saved.resolve(a).as_dict().cloned()).find(|d| d.name(b"Subtype") == Some(b"Widget")).unwrap();
    assert_ne!(widget.int(b"F").unwrap_or(0) & HIDDEN_FLAG, 0, "the hidden widget stays hidden");
    assert!(has(&bytes, b"3 3 l"), "the visible layer's drawing in the appearance stays");
}

#[test]
fn a_membership_dictionary_refuses_and_changes_nothing() {
    // The page's properties also name a membership dictionary of the visible layer (7).
    let mut doc = layered("", vec![b"<< /Type /OCMD /OCGs [5 0 R] >>".to_vec()]);
    doc.update_dict(ObjRef::new(3, 0), |p| {
        let mut res = p.get(b"Resources").and_then(Object::as_dict).cloned().unwrap();
        let mut props = res.get(b"Properties").and_then(Object::as_dict).cloned().unwrap();
        props.set(b"M".to_vec(), Object::Ref(ObjRef::new(7, 0)));
        res.set(b"Properties".to_vec(), Object::Dict(props));
        p.set(b"Resources".to_vec(), Object::Dict(res));
    })
    .unwrap();
    let before = saved(&doc).0;
    assert!(matches!(flatten_layers(&mut doc), Err(RedactError::LayerMembership)));
    assert_eq!(saved(&doc).0, before, "nothing was changed");
}

#[test]
fn a_document_without_layers_is_left_as_it_is() {
    let mut doc = Document::open(Arc::new(
        Pdf::new()
            .objs(vec![
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_vec(),
                stream("", b"/Span << /MCID 0 >> BDC 0 0 m 1 1 l S EMC"),
            ])
            .build(),
    ))
    .unwrap();
    let before = saved(&doc).0;
    assert_eq!(flatten_layers(&mut doc).unwrap(), Flattened::default());
    assert_eq!(saved(&doc).0, before);
}

#[test]
fn an_inline_membership_refuses_and_changes_nothing() {
    // In the page's content, as a property list of its own.
    let inline = Layered { contents: stream("", b"/OC << /Type /OCMD >> BDC 2 2 m 3 3 l S EMC"), ..Default::default() };
    refused_unchanged(inline.build(), |e| matches!(e, RedactError::LayerMembership));
    // As an annotation's /OC: met only after the page's hidden layer was already removed.
    let annot = Layered {
        page_extra: "/Annots [7 0 R]",
        extra: vec![b"<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /OC << /Type /OCMD /OCGs [6 0 R] >> >>".to_vec()],
        ..Default::default()
    };
    refused_unchanged(annot.build(), |e| matches!(e, RedactError::LayerMembership));
}

#[test]
fn nested_marked_content_keeps_its_own_blocks() {
    // An appearance in no layer: a shown block holding two others, a hidden one holding one,
    // and a stray EMC.
    let mut doc = Layered {
        page_extra: "/Annots [7 0 R]",
        extra: vec![
            b"<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /AP << /N 8 0 R >> >>".to_vec(),
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /Properties << /L1 5 0 R /L2 6 0 R >> >>",
                b"/OC /L1 BDC /Span BMC 1 1 m EMC /P << /MCID 0 >> BDC 2 2 m EMC 3 3 m EMC /OC /L2 BDC /Span BMC 4 4 m EMC 5 5 m EMC 6 6 m EMC 7 7 m",
            ),
        ],
        ..Default::default()
    }
    .build();
    flatten_layers(&mut doc).unwrap();
    let (_, doc) = saved(&doc);
    let ap = annots(&doc).remove(0).get(b"AP").and_then(|a| doc.resolve(a).as_dict().cloned()).unwrap();
    let Object::Stream(s) = &*doc.resolve(ap.get(b"N").unwrap()) else { panic!("no appearance") };
    let ops = op_strings(&pdfcraft_content::parse(&s.decoded().unwrap()).ops);
    let ops: Vec<&str> = ops.iter().map(String::as_str).collect();
    assert_eq!(ops, ["/Span BMC", "1 1 m", "EMC", "/P <</MCID 0>> BDC", "2 2 m", "EMC", "3 3 m", "6 6 m", "EMC", "7 7 m"]);
}

#[test]
fn a_tiling_pattern_leaves_its_layers() {
    let mut doc = layered(
        "/Pattern << /P0 7 0 R >>",
        vec![stream(
            "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << /Properties << /L1 5 0 R /L2 6 0 R >> >>",
            b"/OC /L2 BDC (HIDDENPATTERN) Tj EMC /OC /L1 BDC 7 7 m 8 8 l S EMC",
        )],
    );
    flatten_layers(&mut doc).unwrap();
    let (bytes, saved) = saved(&doc);
    let bytes = readable(&bytes, &saved);
    // " BDC", with its space: the file's random `/ID` is hex, and can hold "BDC".
    assert!(!has(&bytes, b"HIDDENPATTERN") && !has(&bytes, b" BDC"), "the pattern's layers are gone");
    assert!(has(&bytes, b"8 8 l"), "the pattern's visible drawing stays");
}

#[test]
fn a_hidden_comment_leaves_with_its_popup() {
    let mut doc = Layered {
        page_extra: "/Annots [7 0 R 8 0 R]",
        extra: vec![
            b"<< /Type /Annot /Subtype /Text /Rect [0 0 10 10] /OC 6 0 R /Contents (HIDDENNOTE) /Popup 8 0 R >>".to_vec(),
            b"<< /Type /Annot /Subtype /Popup /Rect [0 0 10 10] /Parent 7 0 R >>".to_vec(),
        ],
        ..Default::default()
    }
    .build();
    flatten_layers(&mut doc).unwrap();
    let (bytes, saved) = saved(&doc);
    assert!(annots(&saved).is_empty(), "the popup leaves with its comment");
    assert!(!has(&bytes, b"HIDDENNOTE"));
}

#[test]
fn replies_to_a_hidden_comment_leave_and_dangling_popups_are_dropped() {
    let mut doc = Layered {
        page_extra: "/Annots [7 0 R 8 0 R 9 0 R 10 0 R 11 0 R 12 0 R]",
        extra: vec![
            b"<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /OC 6 0 R /Contents (HIDDENNOTE) >>".to_vec(),
            b"<< /Type /Annot /Subtype /Text /Rect [0 0 10 10] /IRT 7 0 R /Contents (REPLYONE) >>".to_vec(),
            b"<< /Type /Annot /Subtype /Text /Rect [0 0 10 10] /IRT 8 0 R /Contents (REPLYTWO) >>".to_vec(),
            // 10: a comment in no layer whose popup (11) names the hidden comment as its parent.
            b"<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /Popup 11 0 R /Contents (KEPTNOTE) >>".to_vec(),
            b"<< /Type /Annot /Subtype /Popup /Rect [0 0 10 10] /Parent 7 0 R >>".to_vec(),
            b"<< /Type /Annot /Subtype /Text /Rect [0 0 10 10] /IRT 9 0 R /Contents (REPLYTHREE) >>".to_vec(),
        ],
        ..Default::default()
    }
    .build();
    flatten_layers(&mut doc).unwrap();
    let (bytes, saved) = saved(&doc);
    let left = annots(&saved);
    assert_eq!(left.len(), 1, "only the comment in no layer stays");
    assert!(left[0].get(b"Popup").is_none(), "its popup left, so the reference goes");
    for gone in [&b"HIDDENNOTE"[..], b"REPLYONE", b"REPLYTWO", b"REPLYTHREE"] {
        assert!(!has(&bytes, gone), "{} survived", String::from_utf8_lossy(gone));
    }
    assert!(has(&bytes, b"KEPTNOTE"));
}

#[test]
fn base_state_off_hides_the_layers_not_turned_on() {
    let mut doc = Layered { d: "<< /BaseState /OFF /ON [5 0 R] >>", ..Default::default() }.build();
    assert_eq!(flatten_layers(&mut doc).unwrap(), Flattened { visible: 1, hidden: 1 });
    merged_as_expected(&doc);
}

#[test]
fn a_view_usage_rule_hides_a_layer_set_off_on_screen() {
    let mut doc = Layered {
        d: "<< /AS [<< /Event /View /Category [/View] /OCGs [6 0 R] >>] >>",
        draft: "<< /Type /OCG /Name (DraftLayer) /Usage << /View << /ViewState /OFF >> >> >>",
        ..Default::default()
    }
    .build();
    assert_eq!(flatten_layers(&mut doc).unwrap(), Flattened { visible: 1, hidden: 1 });
    merged_as_expected(&doc);
}

#[test]
fn properties_held_by_reference_are_read_and_cleaned() {
    // Both layers visible: removing a hidden layer would rewrite the page's resources first.
    let props = Layered { d: "<< >>", props: "7 0 R", extra: vec![b"<< /L1 5 0 R /L2 6 0 R >>".to_vec()], ..Default::default() };
    let mut doc = props.build();
    flatten_layers(&mut doc).unwrap();
    let (bytes, saved) = saved(&doc);
    let ops = page_ops(&saved);
    assert!(ops.iter().any(|o| o == "10 10 l") && ops.iter().any(|o| o == "9 9 l"), "{ops:?}");
    assert!(!ops.iter().any(|o| o.ends_with("BDC") || o == "EMC"), "{ops:?}");
    assert!(!has(&bytes, b"NotesLayer") && !has(&bytes, b"DraftLayer"), "no layer is left in the file");
}

#[test]
fn a_listed_layer_without_a_type_leaves_the_properties() {
    let mut doc = Layered { notes: "<< /Name (NotesLayer) >>", ..Default::default() }.build();
    flatten_layers(&mut doc).unwrap();
    let (bytes, saved) = saved(&doc);
    merged_as_expected(&saved);
    assert!(!has(&bytes, b"NotesLayer"), "the layer is not left in the file");
}

#[test]
fn an_unreadable_page_refuses_and_changes_nothing() {
    // All layers visible, so only the flatten pass reads the page: one that won't decode, and one
    // with a stray byte the parser skips.
    for contents in [stream("/Filter /FlateDecode", b"not flate"), stream("", b"0 0 m ) /OC /L1 BDC 1 1 l S EMC")] {
        let doc = Layered { d: "<< >>", contents, ..Default::default() }.build();
        refused_unchanged(doc, |e| matches!(e, RedactError::Unreadable(1)));
    }
}

#[test]
fn an_unreadable_form_refuses_only_when_it_can_name_a_layer() {
    let form = |res: &str| stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Filter /FlateDecode /Resources {res}"), b"not flate");
    let layered_form = Layered { d: "<< >>", extra: vec![form("<< /Properties << /L1 5 0 R >> >>")], ..Default::default() };
    refused_unchanged(layered_form.build(), |e| matches!(e, RedactError::LayerContentUnreadable(7)));
    // A stray byte the parser skips.
    let stray = stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /Properties << /L1 5 0 R >> >>", b"0 0 m ) 1 1 l S");
    refused_unchanged(Layered { d: "<< >>", extra: vec![stray], ..Default::default() }.build(), |e| {
        matches!(e, RedactError::LayerContentUnreadable(7))
    });
    let mut plain_form = Layered { d: "<< >>", extra: vec![form("<< >>")], ..Default::default() }.build();
    assert_eq!(flatten_layers(&mut plain_form).unwrap(), Flattened { visible: 2, hidden: 0 });
}

#[test]
fn a_view_usage_rule_shows_a_layer_the_lists_leave_off() {
    // Draft (6) is off by /OFF, then by /BaseState /OFF; its View usage turns it on as the
    // document opens, as the renderer shows it.
    for d in [
        "<< /OFF [6 0 R] /AS [<< /Event /View /Category [/View] /OCGs [6 0 R] >>] >>",
        "<< /BaseState /OFF /ON [5 0 R] /AS [<< /Event /View /Category [/View] /OCGs [6 0 R] >>] >>",
    ] {
        let draft = "<< /Type /OCG /Name (DraftLayer) /Usage << /View << /ViewState /ON >> >> >>";
        let mut doc = Layered { d, draft, ..Default::default() }.build();
        assert_eq!(flatten_layers(&mut doc).unwrap(), Flattened { visible: 2, hidden: 0 }, "{d}");
        let ops = page_ops(&doc);
        assert!(ops.iter().any(|o| o == "10 10 l") && ops.iter().any(|o| o == "9 9 l"), "{d}: {ops:?}");
    }
}

#[test]
fn an_unreadable_form_in_no_layer_stays_drawn_while_a_hidden_layer_goes() {
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Filter /FlateDecode /Resources << >>", b"not flate");
    let contents = stream("", b"5 5 m 6 6 l S /OC /L1 BDC 0 0 m 10 10 l S EMC /OC /L2 BDC 1 1 m 9 9 l S EMC /Fm Do");
    let mut doc = Layered { extra_res: "/XObject << /Fm 7 0 R >>", contents, extra: vec![form], ..Default::default() }.build();
    assert_eq!(flatten_layers(&mut doc).unwrap(), Flattened { visible: 1, hidden: 1 });
    merged_as_expected(&doc);
    let (_, saved) = saved(&doc);
    assert!(page_ops(&saved).iter().any(|o| o == "/Fm Do"), "the form in no layer is still drawn: {:?}", page_ops(&saved));
}

/// A comment whose appearance (8), with the page's layer names, draws form 9, which has no
/// `/Resources`, drawing `inner`.
fn drawn_by_an_appearance(ap_props: &str, inner: &[u8], more: Vec<Vec<u8>>) -> Document {
    let mut extra = vec![
        b"<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /AP << /N 8 0 R >> >>".to_vec(),
        stream(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /Properties {ap_props} /XObject << /In 9 0 R >> >>"),
            b"/In Do",
        ),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", inner),
    ];
    extra.extend(more);
    Layered { page_extra: "/Annots [7 0 R]", extra, ..Default::default() }.build()
}

fn form_9(doc: &Document) -> String {
    let Object::Stream(s) = &*doc.get(ObjRef::new(9, 0)) else { panic!("object 9 is not a stream") };
    String::from_utf8_lossy(&s.decoded().unwrap()).into_owned()
}

#[test]
fn a_form_without_resources_reads_its_layers_where_it_is_drawn() {
    let inner = b"/OC /L2 BDC 2 2 m 8 8 l S EMC /OC /L1 BDC 3 3 m 7 7 l S EMC";
    let mut doc = drawn_by_an_appearance("<< /L1 5 0 R /L2 6 0 R >>", inner, Vec::new());
    flatten_layers(&mut doc).unwrap();
    let form = form_9(&doc);
    assert!(!form.contains("8 8 l"), "the hidden layer's drawing is gone: {form}");
    assert!(form.contains("7 7 l") && !form.contains("BDC"), "the visible one stays, unwrapped: {form}");
}

#[test]
fn a_form_without_resources_whose_layers_cant_be_told_refuses() {
    let inner = b"/OC /L2 BDC 2 2 m 8 8 l S EMC";
    // Drawn as an appearance itself: no resources say what L2 is.
    let direct = Layered {
        page_extra: "/Annots [7 0 R]",
        extra: vec![
            b"<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /AP << /N 8 0 R >> >>".to_vec(),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", inner),
        ],
        ..Default::default()
    };
    refused_unchanged(direct.build(), |e| matches!(e, RedactError::LayerContentUnreadable(8)));
    // L2 is the hidden layer where the appearance draws it, and the shown one where form 10 does.
    let other =
        stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /Properties << /L2 5 0 R >> /XObject << /In 9 0 R >> >>", b"/In Do");
    let doc = drawn_by_an_appearance("<< /L2 6 0 R >>", inner, vec![other]);
    refused_unchanged(doc, |e| matches!(e, RedactError::LayerContentUnreadable(9)));
}

/// The content of stream `n`, decoded.
fn stream_text(doc: &Document, n: u32) -> String {
    let Object::Stream(s) = &*doc.get(ObjRef::new(n, 0)) else { panic!("object {n} is not a stream") };
    String::from_utf8_lossy(&s.decoded().unwrap()).into_owned()
}

#[test]
fn a_hidden_block_keeps_the_state_it_sets() {
    // On the page: the colour and the matrix set inside the hidden block still apply after it.
    let contents = stream("", b"0 g /OC /L2 BDC 0 0 1 rg 2 0 0 2 0 0 cm 1 1 m 9 9 l S EMC 5 5 m 6 6 l S");
    let mut doc = Layered { contents, ..Default::default() }.build();
    flatten_layers(&mut doc).unwrap();
    let ops = page_ops(&doc);
    assert!(ops.iter().any(|o| o == "0 0 1 rg") && ops.iter().any(|o| o == "2 0 0 2 0 0 cm"), "the state stays: {ops:?}");
    assert!(!ops.iter().any(|o| o == "9 9 l"), "the hidden drawing goes: {ops:?}");
    // In a tiling pattern, which the page pass doesn't reach: the cell is filled red, not black.
    let mut doc = layered(
        "/Pattern << /P0 7 0 R >>",
        vec![stream(
            "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << /Properties << /L2 6 0 R >> >>",
            b"0 g /OC /L2 BDC 1 0 0 rg 0 0 1 1 re f EMC 0 0 10 10 re f",
        )],
    );
    flatten_layers(&mut doc).unwrap();
    let cell = stream_text(&doc, 7);
    assert!(cell.contains("1 0 0 rg") && !cell.contains("0 0 1 1 re"), "{cell}");
}

/// A resource a hidden block's kept state names stays listed: the page pass that drops the
/// resources a page no longer names sees the name in what the block left.
#[test]
fn a_hidden_blocks_kept_state_keeps_its_resource() {
    let contents = stream("", b"/OC /L2 BDC /GS1 gs 1 1 m 9 9 l S EMC 5 5 m 6 6 l S");
    let mut doc =
        Layered { contents, extra_res: "/ExtGState << /GS1 7 0 R >>", extra: vec![b"<< /Type /ExtGState /CA 0.5 >>".to_vec()], ..Default::default() }
            .build();
    flatten_layers(&mut doc).unwrap();
    let (_, doc) = saved(&doc);
    assert!(page_ops(&doc).iter().any(|o| o == "/GS1 gs"), "{:?}", page_ops(&doc));
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    let res = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let gs = sub(&doc, &res, b"ExtGState").and_then(|g| g.get(b"GS1").map(|o| doc.resolve(o).as_dict().cloned()));
    assert!(gs.flatten().is_some_and(|d| d.get(b"CA").is_some()), "{res:?}");
}

#[test]
fn hidden_text_leaves_the_text_after_it_where_it_was() {
    let font = "/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> >>";
    let contents = stream("", b"BT /F1 10 Tf 0 0 Td /OC /L2 BDC (HIDDENWORD) Tj EMC (Shown) Tj ET");
    let mut doc = Layered { contents, extra_res: font, ..Default::default() }.build();
    flatten_layers(&mut doc).unwrap();
    let ops = page_ops(&doc);
    assert!(!ops.iter().any(|o| o.contains("HIDDENWORD")), "{ops:?}");
    let shown = ops.iter().position(|o| o.contains("Shown")).unwrap();
    // A `TJ` number moves the text right when negative: "Shown" still starts past the hidden word.
    let moves: f64 = ops[..shown].iter().filter_map(|o| o.strip_prefix('[')?.strip_suffix("] TJ")?.trim().parse::<f64>().ok()).sum();
    assert!(moves < 0.0, "{ops:?}");
    // In a pattern, where fonts aren't read, the move can't be measured: refused.
    let doc = layered(
        "/Pattern << /P0 7 0 R >>",
        vec![stream(
            &format!(
                "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << /Properties << /L2 6 0 R >> {font} >>"
            ),
            b"BT /F1 10 Tf /OC /L2 BDC (HIDDENWORD) Tj EMC (Shown) Tj ET",
        )],
    );
    refused_unchanged(doc, |e| matches!(e, RedactError::LayerContentUnreadable(7)));
}

#[test]
fn a_soft_mask_group_without_resources_reads_its_layers_where_it_is_set() {
    let mut doc = Layered {
        contents: stream("", b"/GS1 gs 0 0 10 10 re f /OC /L2 BDC 1 1 m 9 9 l S EMC"),
        extra_res: "/ExtGState << /GS1 << /SMask << /S /Luminosity /G 7 0 R >> >> >>",
        extra: vec![stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Group << /S /Transparency /CS /DeviceGray >>",
            b"/OC /L2 BDC 2 2 m 8 8 l S EMC /OC /L1 BDC 3 3 m 7 7 l S EMC",
        )],
        ..Default::default()
    }
    .build();
    flatten_layers(&mut doc).unwrap();
    let group = stream_text(&doc, 7);
    assert!(!group.contains("8 8 l"), "the hidden layer's drawing leaves the mask: {group}");
    assert!(group.contains("7 7 l") && !group.contains("BDC"), "the visible one stays, unwrapped: {group}");
}

#[test]
fn a_type3_glyph_leaves_its_layers() {
    let mut doc = Layered {
        contents: stream("", b"BT /T3 1 Tf (a) Tj ET /OC /L2 BDC 1 1 m 9 9 l S EMC"),
        extra_res: "/Font << /T3 7 0 R >>",
        extra: vec![
            b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] /FontMatrix [0.1 0 0 0.1 0 0] /CharProcs << /a 8 0 R >> \
              /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [10] \
              /Resources << /Properties << /L1 5 0 R /L2 6 0 R >> >> >>"
                .to_vec(),
            stream("", b"10 0 d0 /OC /L2 BDC 0 0 5 5 re f EMC /OC /L1 BDC 1 1 2 2 re f EMC"),
        ],
        ..Default::default()
    }
    .build();
    flatten_layers(&mut doc).unwrap();
    let glyph = stream_text(&doc, 8);
    assert!(!glyph.contains("0 0 5 5 re"), "the hidden layer's drawing leaves the glyph: {glyph}");
    assert!(glyph.contains("1 1 2 2 re") && !glyph.contains("BDC"), "the visible one stays, unwrapped: {glyph}");
}

/// In a text object `Q` restores the text matrices `q` saved: a `Td` between them is undone, so
/// the string after `Q` still lands where the hidden one left the position.
#[test]
fn a_move_undone_by_q_still_owes_hidden_texts_move() {
    let font = "/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> >>";
    let pattern = |content: &[u8]| {
        layered(
            "/Pattern << /P0 7 0 R >>",
            vec![stream(
                &format!(
                    "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << /Properties << /L2 6 0 R >> {font} >>"
                ),
                content,
            )],
        )
    };
    refused_unchanged(pattern(b"BT /F1 10 Tf /OC /L2 BDC (A) Tj EMC q 0 0 Td Q (B) Tj ET"), |e| matches!(e, RedactError::LayerContentUnreadable(7)));
    let mut doc = pattern(b"BT /F1 10 Tf /OC /L2 BDC (A) Tj EMC 0 0 Td (B) Tj ET");
    flatten_layers(&mut doc).unwrap();
    let cell = stream_text(&doc, 7);
    assert!(cell.contains("(B)") && !cell.contains("(A)"), "{cell}");
}

/// The other way round: hidden text between `q` and `Q` in one text object moves a position `Q`
/// throws away, so the string after `Q` owes it nothing and the cell flattens. A `q` from before
/// the text object saved no text position: the debt stands.
#[test]
fn hidden_text_between_q_and_q_owes_nothing_after_q() {
    let font = "/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> >>";
    let pattern = |content: &[u8]| {
        layered(
            "/Pattern << /P0 7 0 R >>",
            vec![stream(
                &format!(
                    "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << /Properties << /L2 6 0 R >> {font} >>"
                ),
                content,
            )],
        )
    };
    let mut doc = pattern(b"BT /F1 10 Tf q /OC /L2 BDC (A) Tj EMC Q (B) Tj ET");
    flatten_layers(&mut doc).unwrap();
    let cell = stream_text(&doc, 7);
    assert!(cell.contains("(B)") && !cell.contains("(A)"), "{cell}");
    refused_unchanged(pattern(b"q BT /F1 10 Tf /OC /L2 BDC (A) Tj EMC Q (B) Tj ET"), |e| matches!(e, RedactError::LayerContentUnreadable(7)));
}

/// At font size 0 hidden glyphs still move by their character spacing, which a `TJ` number can't
/// write: refused. With no spacing nothing moves, and the text merges.
#[test]
fn hidden_text_at_size_zero_with_spacing_refuses() {
    let font = "/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >> >>";
    let page = |content: &[u8]| Layered { contents: stream("", content), extra_res: font, ..Default::default() }.build();
    refused_unchanged(page(b"BT /F1 0 Tf 5 Tc /OC /L2 BDC (A) Tj EMC /F1 10 Tf (B) Tj ET"), |e| {
        matches!(e, RedactError::PageRefused { page: 1, .. })
    });
    let mut doc = page(b"BT /F1 0 Tf 0 Tc /OC /L2 BDC (A) Tj EMC /F1 10 Tf (B) Tj ET");
    flatten_layers(&mut doc).unwrap();
    let ops = page_ops(&doc);
    assert!(ops.iter().any(|o| o == "(B) Tj") && !ops.iter().any(|o| o.contains("(A)")), "{ops:?}");
}

/// A font that writes vertically (Identity-V) moves hidden glyphs down by vertical metrics that
/// aren't read: refused, as at size 0. The same text in Identity-H merges.
#[test]
fn hidden_text_in_a_vertical_font_refuses() {
    let page = |cmap: &'static str| {
        let contents = stream("", b"BT /F0 10 Tf /OC /L2 BDC <0001> Tj EMC <0002> Tj ET");
        let font = format!(
            "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /{cmap} /DescendantFonts [<< /Type /Font /Subtype /CIDFontType2 \
             /BaseFont /X /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW 500 >>] >>"
        );
        Layered { contents, extra_res: "/Font << /F0 7 0 R >>", extra: vec![font.into_bytes()], ..Default::default() }.build()
    };
    refused_unchanged(page("Identity-V"), |e| matches!(e, RedactError::PageRefused { page: 1, .. }));
    let mut doc = page("Identity-H");
    flatten_layers(&mut doc).unwrap();
    let ops = page_ops(&doc);
    assert!(ops.iter().any(|o| o == "<0002> Tj") && !ops.iter().any(|o| o.contains("<0001>")), "{ops:?}");
}

#[test]
fn a_shading_patterns_soft_mask_group_reads_its_layers_where_the_pattern_is_named() {
    let mut doc = Layered {
        contents: stream("", b"/Pattern cs /Sh scn 0 0 10 10 re f /OC /L2 BDC 1 1 m 9 9 l S EMC"),
        extra_res: "/Pattern << /Sh 7 0 R >>",
        extra: vec![
            b"<< /PatternType 2 /Shading << /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 10 0] \
              /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> >> \
              /ExtGState << /SMask << /S /Luminosity /G 8 0 R >> >> >>"
                .to_vec(),
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Group << /S /Transparency /CS /DeviceGray >>",
                b"/OC /L2 BDC 2 2 m 8 8 l S EMC /OC /L1 BDC 3 3 m 7 7 l S EMC",
            ),
        ],
        ..Default::default()
    }
    .build();
    flatten_layers(&mut doc).unwrap();
    let group = stream_text(&doc, 8);
    assert!(!group.contains("8 8 l"), "the hidden layer's drawing leaves the mask: {group}");
    assert!(group.contains("7 7 l") && !group.contains("BDC"), "the visible one stays, unwrapped: {group}");
}

/// A Type 3 font without `/Resources`, a glyph of which draws in `glyph` (object 8).
fn type3_without_resources(glyph: &[u8]) -> Vec<Vec<u8>> {
    vec![
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] /FontMatrix [0.1 0 0 0.1 0 0] /CharProcs << /a 8 0 R >> \
          /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [10] >>"
            .to_vec(),
        stream("", glyph),
    ]
}

/// The font is set on the page and inherited by form 9, which doesn't list it: its glyph's name
/// is a hidden layer in the form's resources, and in the page's none or a shown one. Refused.
#[test]
fn a_glyph_inherited_by_a_form_that_names_its_layer_refuses() {
    for props in ["<< /L1 5 0 R /L2 6 0 R >>", "<< /L1 5 0 R /L2 6 0 R /L9 5 0 R >>"] {
        let mut extra = type3_without_resources(b"10 0 d0 /OC /L9 BDC 0 0 5 5 re f EMC");
        extra.push(stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /Properties << /L9 6 0 R >> >>", b"BT (a) Tj ET"));
        let doc = Layered {
            props,
            contents: stream("", b"/T3 1 Tf /Fm Do /OC /L2 BDC 1 1 m 9 9 l S EMC"),
            extra_res: "/Font << /T3 7 0 R >> /XObject << /Fm 9 0 R >>",
            extra,
            ..Default::default()
        }
        .build();
        refused_unchanged(doc, |e| matches!(e, RedactError::LayerContentUnreadable(8)));
    }
}

/// The rule, not a path: form 9, without `/Resources`, is the icon (`/MK /I`) of a push button
/// with no appearance of its own, drawn in the one a reader makes for it; no resource dictionary
/// in the file names it, so the index finds no caller. Its layer name is refused, not shown.
#[test]
fn a_layer_name_no_known_caller_holds_refuses() {
    let doc = Layered {
        page_extra: "/Annots [7 0 R]",
        extra: vec![
            b"<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (B) /Rect [0 0 10 10] /MK << /I 9 0 R >> >>".to_vec(),
            b"<< >>".to_vec(),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", b"/OC /L2 BDC 0 0 5 5 re f EMC"),
        ],
        ..Default::default()
    }
    .build();
    refused_unchanged(doc, |e| matches!(e, RedactError::LayerContentUnreadable(9)));
}

/// A Type 3 font without `/Resources` whose glyph (object 8) draws `glyph`, held only in the
/// form's `/DR` for a field's `/DA` (object 9): no resource dictionary lists it, so the index
/// finds no caller and only the last check reads the glyph.
fn field_glyph(glyph: &[u8]) -> Layered {
    let mut extra = type3_without_resources(glyph);
    extra.push(b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (F) /Rect [0 0 10 10] /DA (/T3 1 Tf 0 g) /V (a) >>".to_vec());
    Layered {
        catalog_extra: "/AcroForm << /Fields [9 0 R] /DR << /Font << /T3 7 0 R >> >> /DA (/T3 1 Tf 0 g) >>",
        page_extra: "/Annots [9 0 R]",
        extra,
        ..Default::default()
    }
}

/// The result is checked, not the paths: [`field_glyph`]'s glyph keeps its hidden block, and the
/// layers it named are gone. Refused.
#[test]
fn a_layer_block_left_by_a_glyph_no_resource_lists_refuses() {
    refused_unchanged(field_glyph(b"10 0 d0 /OC /L2 BDC 0 0 5 5 re f EMC").build(), |e| matches!(e, RedactError::LayerContentUnreadable(8)));
}

/// A Type 3 font without `/Resources` set by an ExtGState's `/Font` shows its glyph under the
/// resources that hold the ExtGState: its names are read there, and its hidden block goes.
#[test]
fn a_glyph_set_by_an_extgstate_reads_its_layers_where_it_is_set() {
    let mut doc = Layered {
        contents: stream("", b"/GS gs BT (a) Tj ET /OC /L2 BDC 1 1 m 9 9 l S EMC"),
        extra_res: "/ExtGState << /GS << /Font [7 0 R 1] >> >>",
        extra: type3_without_resources(b"10 0 d0 /OC /L2 BDC 0 0 5 5 re f EMC /OC /L1 BDC 1 1 2 2 re f EMC"),
        ..Default::default()
    }
    .build();
    flatten_layers(&mut doc).unwrap();
    let glyph = stream_text(&doc, 8);
    assert!(!glyph.contains("0 0 5 5 re"), "the hidden layer's drawing leaves the glyph: {glyph}");
    assert!(glyph.contains("1 1 2 2 re") && !glyph.contains("BDC"), "the visible one stays, unwrapped: {glyph}");
}

/// The page names the glyph's layer `/L9` as hidden; form 10 sets the font through an ExtGState
/// and holds no `/L9`, so there the glyph's block shows. The two disagree: refused, not removed.
#[test]
fn a_glyph_set_by_an_extgstate_where_its_layer_is_unnamed_refuses() {
    let mut extra = type3_without_resources(b"10 0 d0 /OC /L9 BDC 0 0 5 5 re f EMC");
    extra.push(b"<< >>".to_vec());
    extra.push(stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /ExtGState << /GS << /Font [7 0 R 1] >> >> >>",
        b"/GS gs BT (a) Tj ET",
    ));
    let doc = Layered {
        props: "<< /L1 5 0 R /L2 6 0 R /L9 6 0 R >>",
        contents: stream("", b"BT /T3 1 Tf (a) Tj ET /Fm Do /OC /L2 BDC 1 1 m 9 9 l S EMC"),
        extra_res: "/Font << /T3 7 0 R >> /XObject << /Fm 10 0 R >>",
        extra,
        ..Default::default()
    }
    .build();
    refused_unchanged(doc, |e| matches!(e, RedactError::LayerContentUnreadable(8)));
}

/// The check reads content, not every stream: an attachment holding an uncompressed layered PDF's
/// block is no reader's content, and the flatten goes through.
#[test]
fn an_attachment_holding_a_layer_block_flattens() {
    let mut doc = Layered {
        catalog_extra: "/Names << /EmbeddedFiles << /Names [(a.pdf) << /Type /Filespec /F (a.pdf) /EF << /F 7 0 R >> >>] >> >>",
        extra: vec![stream("/Type /EmbeddedFile", b"/OC /L2 BDC 0 0 5 5 re f EMC")],
        ..Default::default()
    }
    .build();
    flatten_layers(&mut doc).unwrap();
    merged_as_expected(&doc);
}

/// [`field_glyph`]'s glyph stored as Flate with its last 6 bytes cut: strict decoding fails, and
/// a reader reads what comes before the damage.
fn damaged_glyph(glyph: &[u8]) -> Document {
    let mut doc = field_glyph(b"").build();
    let raw = Stream::flate(Dict::new(), glyph).raw.to_vec();
    let cut = raw.get(..raw.len() - 6).unwrap().to_vec();
    let mut dict = Dict::new();
    dict.set(b"Filter".to_vec(), Object::Name(b"FlateDecode".to_vec()));
    doc.set_existing(ObjRef::new(8, 0), Object::Stream(Stream::from_raw(dict, cut))).unwrap();
    assert!(matches!(&*doc.get(ObjRef::new(8, 0)), Object::Stream(s) if DecodeBudget::page().decode_strict(&doc, s).is_err()));
    doc
}

/// A damaged glyph is read as far as a reader reads it: a layer's block before the damage is
/// refused, and a glyph without one flattens.
#[test]
fn a_damaged_glyph_is_read_up_to_the_damage() {
    let tail = b" 1 1 m 2 2 l S 3 3 m 4 4 l S 5 5 m 6 6 l S";
    let block = [b"10 0 d0 /OC /L2 BDC 0 0 5 5 re f EMC".as_slice(), tail].concat();
    refused_unchanged(damaged_glyph(&block), |e| matches!(e, RedactError::LayerContentUnreadable(8)));
    let mut plain = damaged_glyph(&[b"10 0 d0 0 0 5 5 re f".as_slice(), tail].concat());
    flatten_layers(&mut plain).unwrap();
    merged_as_expected(&plain);
}

/// A glyph no filter can read is refused: it is content, and what it draws can't be told,
/// even when its dictionary also calls it a form with `/Resources` of its own.
#[test]
fn an_undecodable_glyph_refuses() {
    for dict in ["/Filter /NoSuchDecode", "/Filter /NoSuchDecode /Subtype /Form /Resources << >>"] {
        let mut doc = field_glyph(b"");
        doc.extra[1] = stream(dict, b"10 0 d0 0 0 5 5 re f");
        refused_unchanged(doc.build(), |e| matches!(e, RedactError::LayerContentUnreadable(8)));
    }
}

/// A glyph is content whatever its dictionary says: one marked `/Type /EmbeddedFile` that no
/// `/EF` names, or `/Subtype /Image`, is read, and its block refused.
#[test]
fn a_glyph_calling_itself_something_else_refuses() {
    for dict in ["/Type /EmbeddedFile", "/Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray"] {
        let mut doc = field_glyph(b"");
        doc.extra[1] = stream(dict, b"10 0 d0 /OC /L2 BDC 0 0 5 5 re f EMC");
        refused_unchanged(doc.build(), |e| matches!(e, RedactError::LayerContentUnreadable(8)));
    }
}

/// `/Type` is optional on an embedded file: one an `/EF` names, holding a layered PDF's block,
/// is still no reader's content, and the flatten goes through.
#[test]
fn an_attachment_without_a_type_flattens() {
    let mut doc = Layered {
        catalog_extra: "/Names << /EmbeddedFiles << /Names [(a.pdf) << /Type /Filespec /F (a.pdf) /EF << /F 7 0 R >> >>] >> >>",
        extra: vec![stream("/Subtype /application#2Fpdf", b"/OC /L2 BDC 0 0 5 5 re f EMC")],
        ..Default::default()
    }
    .build();
    flatten_layers(&mut doc).unwrap();
    merged_as_expected(&doc);
}

/// Bytes the parser couldn't read may hold a block another reader finds: a glyph whose
/// unterminated string runs over a layer's block, or whose inline image's end is a guess, is
/// refused; the block is still in its text.
#[test]
fn a_block_in_bytes_the_parser_skipped_refuses() {
    let string = b"10 0 d0 0 0 5 5 re f (a /OC /L2 BDC 1 1 2 2 re f EMC";
    let image = b"10 0 d0 BI /W 1 /H 1 /BPC 8 /CS /G ID /OC /L2 BDC 1 1 2 2 re f EMC";
    for glyph in [&string[..], image] {
        let mut doc = field_glyph(b"");
        doc.extra[1] = stream("", glyph);
        refused_unchanged(doc.build(), |e| matches!(e, RedactError::LayerContentUnreadable(8)));
    }
}

/// The form pass's verdict stands: a form no filter can read, whose own resources name no
/// layer, is kept and still drawn, while the hidden layer goes.
#[test]
fn an_undecodable_form_that_can_name_no_layer_stays() {
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Filter /NoSuchDecode /Resources << >>", b"0 0 m 1 1 l S");
    let contents = stream("", b"5 5 m 6 6 l S /OC /L1 BDC 0 0 m 10 10 l S EMC /OC /L2 BDC 1 1 m 9 9 l S EMC /Fm Do");
    let mut doc = Layered { extra_res: "/XObject << /Fm 7 0 R >>", contents, extra: vec![form], ..Default::default() }.build();
    flatten_layers(&mut doc).unwrap();
    merged_as_expected(&doc);
    let (_, saved) = saved(&doc);
    assert!(page_ops(&saved).iter().any(|o| o == "/Fm Do"), "the form is still drawn: {:?}", page_ops(&saved));
    let page = pdfcraft_model::pages(&saved).remove(0);
    let res = page.dict.get(b"Resources").and_then(|r| saved.resolve(r).as_dict().cloned()).unwrap_or_default();
    let fm = sub(&saved, &res, b"XObject").and_then(|x| x.get(b"Fm").cloned());
    let kept = fm.map(|f| saved.resolve(&f));
    assert!(matches!(kept.as_deref(), Some(Object::Stream(s)) if has_name(&saved, &s.dict, b"Filter", b"NoSuchDecode")), "{kept:?}");
}
