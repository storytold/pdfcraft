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
            contents: stream("", b"5 5 m 6 6 l S /OC /L1 BDC 0 0 m 10 10 l S EMC /OC /L2 BDC 1 1 m 9 9 l S EMC"),
            extra: Vec::new(),
        }
    }
}

impl Layered {
    fn build(self) -> Document {
        let mut objs = vec![
            format!("<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R] /D {} >> >>", self.d).into_bytes(),
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
    assert!(!has(&bytes, b"HIDDENPATTERN") && !has(&bytes, b"BDC"), "the pattern's layers are gone");
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
