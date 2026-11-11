//! What may follow a signature, as one table (#7032): the workflows a later revision must be able
//! to carry out without breaking a signature, and the changes no signature permits.
//!
//! The cases here are built on one form. Older tests of the same question live beside the code
//! they were written for; this index lists them so the whole regression set can be read in one
//! place.
//!
//! Must stay Valid (legitimate workflows):
//! - `pdf::signing_an_existing_field_and_counter_signing`, `pdf::several_later_signatures_each_sign_their_own_revision`
//! - `pdf::an_indirect_annots_array_is_judged_by_the_annotations_it_gains` (signing and commenting through an indirect `/Annots`)
//! - `pdf::later_changes_are_classified_under_the_signature_permissions` (a comment under an approval signature and P=3)
//! - `pdf::a_document_timestamp_keeps_a_no_changes_certification_valid`, `pdf::sign_then_ltv_then_timestamp_makes_a_b_lta_file`
//! - `pdf::embedding_ltv_evidence_adds_a_dss_and_keeps_signatures_valid`, `pdf::ltv_writers_may_declare_the_esic_extension_but_not_change_a_signed_one`
//! - `pdf::validation_data_and_xmp_added_after_signing_are_not_tampering`, `pdf::the_catalogs_own_xmp_stream_may_still_be_rewritten_after_signing`
//! - `pdf::rewriting_a_certified_forms_xfa_data_is_a_form_fill`
//! - `pdf::a_cross_reference_stream_without_a_row_for_itself_is_no_unlisted_body` (#6768)
//! - `pdf::signing_a_field_with_a_null_value_after_another_signature_keeps_the_first_valid`
//! - `pdf::a_user_password_document_keeps_its_earlier_signature_valid_when_counter_signed`
//! - `signed_structure::a_signed_fields_widget_may_be_filled_but_not_hidden_or_moved` (the fill)
//! - `signed_structure::choosing_a_radio_button_whose_value_is_on_its_parent_is_a_form_fill` (#5291)
//! - `field_locks::without_a_lock_the_same_change_is_a_form_fill`, and the unlocked fills in `field_locks`
//! - unit tests in `src/pdf/revision_classification_tests.rs`: `a_string_respelled_in_hex_is_no_change`,
//!   `nested_strings_respelled_and_reordered_are_no_change`, `a_signature_added_with_respelled_strings_is_signing`,
//!   `a_widget_rect_saved_back_as_integers_is_a_form_fill` (#6502)
//!
//! Must not be Valid (attacks):
//! - page content, resources and the page tree: `pdf::later_changes_are_classified_under_the_signature_permissions`,
//!   `pdf::rewritten_page_resources_are_page_content_whatever_keys_they_carry`, `pdf::a_signed_page_relabelled_or_stripped_of_type_is_still_page_content`,
//!   `pdf::freeing_a_signed_object_or_rewriting_the_signed_widget_is_a_page_content_change`, `signed_structure::a_signed_page_tree_node_is_judged_as_the_page_tree`,
//!   `pdf::declaring_a_form_xfa_does_not_excuse_replacing_the_pages`
//! - the catalog, trailer and form: `signed_structure::a_signed_catalog_is_judged_as_the_catalog_whatever_its_type`,
//!   `pdf::switching_the_trailer_to_another_signed_catalog_is_not_permitted`, `signed_structure::a_field_dropped_from_the_form_is_no_change_any_level_permits`,
//!   `pdf::the_forms_signed_fields_are_not_dropped_after_signing`, `pdf::the_catalogs_perms_do_not_change_under_no_changes`
//! - relabelled objects: `pdf::an_existing_object_given_form_keys_after_signing_is_still_judged`,
//!   `pdf::a_dictionary_labelled_metadata_is_not_a_metadata_change`, `pdf::a_form_xobject_relabelled_metadata_is_not_a_metadata_change`,
//!   `pdf::a_form_xobject_relabelled_as_file_plumbing_is_still_judged`, `pdf::a_stream_already_labelled_metadata_when_signed_is_not_metadata_unless_the_catalog_names_it`
//! - the document security store: `pdf::a_dss_entry_does_not_make_the_object_it_names_part_of_the_store`,
//!   `pdf::an_existing_dictionary_is_not_a_store_part_because_the_store_names_it`, `pdf::an_existing_array_stays_what_it_is_when_the_store_points_at_it`,
//!   `pdf::a_signed_store_may_grow_but_not_lose_or_swap_entries`
//! - widgets and signatures (#6755, #6756): `pdf::an_empty_signature_widget_added_or_restyled_after_certification_is_not_signing`,
//!   `pdf::a_later_widget_reusing_an_earlier_signature_is_not_signing`, `pdf::only_an_invisible_document_timestamp_survives_a_no_changes_certification`,
//!   `pdf::an_unsigned_signature_widget_turned_into_a_comment_is_no_edit_of_it`, `pdf::removing_a_widget_from_an_inline_annots_array_is_not_a_comment_change`,
//!   `signed_structure::a_widget_restyled_or_added_without_a_value_change_is_not_a_form_fill`,
//!   `revision_classification::widget_replaced_in_place_by_comment`, `revision_classification::widget_freed_then_reused_as_comment`
//! - field locks (#6757): every `field_locks` test that asserts a locked change
//! - file plumbing (#6754, #6759): `pdf::a_cross_reference_entry_the_reader_sets_aside_does_not_hide_new_page_content`,
//!   `pdf::a_cross_reference_entry_that_misses_its_object_is_not_an_unchanged_object`,
//!   `pdf::an_unlisted_object_body_or_an_entry_that_points_at_no_object_is_not_permitted`,
//!   `revision_classification::intermediate_generation_cover_p1` (and `_p2`, `_p3`), and the unit tests in
//!   `src/pdf/revision_classification_tests.rs` and `src/pdf/changes.rs`
//! - who signed: `pdf::changes_after_an_unmatched_signed_revision_are_not_permitted`, `pdf::only_the_signature_the_catalog_names_certifies`

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, SaveOptions, Stream, write_incremental};
use pdfcraft_sign::{Modification, SignOptions, Status, Time, TrustStore, pkcs12, signatures};

use crate::pdf::{TestTsa, tsa_id};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn open(b: &[u8]) -> Document {
    Document::open(Arc::new(b.to_vec())).unwrap()
}

fn r(num: u32) -> ObjRef {
    ObjRef { num, generation: 0 }
}

/// A classic-xref PDF of `objs`, numbered from 1, with object 1 as the catalog.
fn write_pdf(objs: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

const NAME: u32 = 5;
const TOTAL: u32 = 7;
const TOTAL_WIDGETS: [u32; 2] = [8, 9];
const PAGE: u32 = 3;

/// One page with: text field "Name" (field and widget in one, object 5), the empty signature
/// field "Approval" (6), and text field "Total" (7) shown by two widgets (8 and 9). Every widget
/// has an appearance, and the form's resources hold Helvetica (12).
fn form() -> Vec<u8> {
    write_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 6 0 R 7 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 12 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 12 0 R >> >> /Annots [5 0 R 6 0 R 8 0 R 9 0 R] >>",
        "<< /Length 44 >>\nstream\nBT /F1 14 Tf 20 250 Td (Contract text) Tj ET\nendstream",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /V (Ann) /Rect [20 200 120 220] /P 3 0 R /F 4 /AP << /N 10 0 R >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /F 4 >>",
        "<< /FT /Tx /T (Total) /V (1) /Kids [8 0 R 9 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /Parent 7 0 R /Rect [20 170 120 190] /P 3 0 R /F 4 /AP << /N 11 0 R >> >>",
        "<< /Type /Annot /Subtype /Widget /Parent 7 0 R /Rect [20 140 120 160] /P 3 0 R /F 4 /AP << /N 11 0 R >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << /Font << /Helv 12 0 R >> >> /Length 33 >>\nstream\n/Tx BMC BT /Helv 12 Tf (Ann) Tj ET\nendstream",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources << /Font << /Helv 12 0 R >> >> /Length 31 >>\nstream\n/Tx BMC BT /Helv 12 Tf (1) Tj ET\nendstream",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ])
}

fn id() -> pkcs12::DigitalId {
    pkcs12::open(&data("rsa-aes.p12"), "test").unwrap()
}

fn trust() -> TrustStore {
    TrustStore { certs: vec![id().certificate.clone(), tsa().id.certificate.clone()] }
}

/// One authority for every case, so the trust above covers its stamps.
fn tsa() -> &'static TestTsa {
    static TSA: std::sync::OnceLock<TestTsa> = std::sync::OnceLock::new();
    TSA.get_or_init(|| TestTsa { id: tsa_id(), time: Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 } })
}

/// Signing options: a new visible field at `rect`, or the named field.
fn opts(certify: Option<u8>, field: Option<&str>, rect: [f64; 4]) -> SignOptions {
    SignOptions {
        page: 0,
        rect: Some(rect),
        date: "D:20261002120000+01'00'".into(),
        certify,
        field: field.map(str::to_owned),
        ..SignOptions::default()
    }
}

fn sign(bytes: &[u8], certify: Option<u8>, field: Option<&str>) -> Vec<u8> {
    pdfcraft_sign::sign(&open(bytes), &id(), &opts(certify, field, [150.0, 100.0, 280.0, 140.0])).unwrap()
}

/// A second signature in a new field of its own, placed apart from the first.
fn counter_sign(bytes: &[u8]) -> Vec<u8> {
    pdfcraft_sign::sign(&open(bytes), &id(), &opts(None, None, [150.0, 230.0, 280.0, 270.0])).unwrap()
}

fn edit_after(signed: &[u8], f: impl FnOnce(&mut Document)) -> Vec<u8> {
    let mut doc = open(signed);
    f(&mut doc);
    write_incremental(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap()
}

/// A new appearance stream showing `text`, as a form filler draws it.
fn appearance(doc: &mut Document, text: &str) -> Object {
    let body = format!("/Tx BMC BT /Helv 12 Tf ({text}) Tj ET EMC").into_bytes();
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array([0, 0, 100, 20].map(Object::Int).to_vec()));
    d.set(b"Length".to_vec(), Object::Int(body.len() as i64));
    let s = doc.add(Object::Stream(Stream::from_raw(d, body)));
    Object::Dict(Dict::from_iter([(b"N".to_vec(), Object::Ref(s))]))
}

/// Fill "Name" with `text`: its value and the appearance that shows it.
fn fill_name(doc: &mut Document, text: &str) {
    let ap = appearance(doc, text);
    doc.update_dict(r(NAME), |w| {
        w.set(b"V".to_vec(), PdfString::text(text));
        w.set(b"AP".to_vec(), ap);
    })
    .unwrap();
}

/// A text note added to the page.
fn add_comment(doc: &mut Document) {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Annot"));
    d.set(b"Subtype".to_vec(), Object::name("Text"));
    d.set(b"Rect".to_vec(), Object::Array([10, 10, 30, 30].map(Object::Int).to_vec()));
    d.set(b"Contents".to_vec(), PdfString::text("A note"));
    let note = doc.add(Object::Dict(d));
    doc.update_dict(r(PAGE), |p| {
        if let Some(Object::Array(a)) = p.get_mut(b"Annots") {
            a.push(Object::Ref(note));
        }
    })
    .unwrap();
}

/// The catalog's form dictionary, edited in place.
fn edit_form(doc: &mut Document, f: impl FnOnce(&mut Dict)) {
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| {
        if let Some(Object::Dict(form)) = c.get_mut(b"AcroForm") {
            f(form);
        }
    })
    .unwrap();
}

/// What a row must be.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Expect {
    Valid,
    NotValid,
}

/// One file, built by a workflow, and the rows of its signature list it is judged by: each named
/// field (the certification is `Signature1` when the case certifies) and what it must be.
struct Case {
    name: &'static str,
    file: fn() -> Vec<u8>,
    rows: &'static [(&'static str, Expect)],
}

use Expect::{NotValid, Valid};

/// Workflows that must leave every signature Valid.
const LEGITIMATE: &[Case] = &[
    Case {
        name: "an approval signature, then a comment",
        file: || edit_after(&sign(&form(), None, None), add_comment),
        rows: &[("Signature1", Valid)],
    },
    Case {
        name: "a P=3 certification, then a comment",
        file: || edit_after(&sign(&form(), Some(3), None), add_comment),
        rows: &[("Signature1", Valid)],
    },
    Case {
        name: "a P=2 certification, then the existing empty field signed",
        file: || sign(&sign(&form(), Some(2), None), None, Some("Approval")),
        rows: &[("Signature1", Valid), ("Approval", Valid)],
    },
    Case {
        name: "an approval signature, then a counter-signature",
        file: || counter_sign(&sign(&form(), None, None)),
        rows: &[("Signature1", Valid), ("Signature2", Valid)],
    },
    Case {
        name: "an approval signature, then a form fill",
        file: || edit_after(&sign(&form(), None, None), |doc| fill_name(doc, "Bob")),
        rows: &[("Signature1", Valid)],
    },
    Case {
        name: "a P=2 certification, then a text field filled, then signed",
        file: || counter_sign(&edit_after(&sign(&form(), Some(2), None), |doc| fill_name(doc, "Bob"))),
        rows: &[("Signature1", Valid), ("Signature2", Valid)],
    },
    Case {
        name: "a P=2 certification, then a field with two widgets filled",
        file: || {
            edit_after(&sign(&form(), Some(2), None), |doc| {
                doc.update_dict(r(TOTAL), |f| f.set(b"V".to_vec(), PdfString::text("2"))).unwrap();
                for w in TOTAL_WIDGETS {
                    let ap = appearance(doc, "2");
                    doc.update_dict(r(w), |d| d.set(b"AP".to_vec(), ap)).unwrap();
                }
            })
        },
        rows: &[("Signature1", Valid)],
    },
    Case {
        name: "a P=2 certification, then a fill that sets /NeedAppearances and adds a font to /DR",
        file: || {
            edit_after(&sign(&form(), Some(2), None), |doc| {
                fill_name(doc, "Bob");
                let mut font = Dict::new();
                font.set(b"Type".to_vec(), Object::name("Font"));
                font.set(b"Subtype".to_vec(), Object::name("Type1"));
                font.set(b"BaseFont".to_vec(), Object::name("ZapfDingbats"));
                let font = doc.add(Object::Dict(font));
                edit_form(doc, |form| {
                    form.set(b"NeedAppearances".to_vec(), Object::Bool(true));
                    if let Some(Object::Dict(dr)) = form.get_mut(b"DR")
                        && let Some(Object::Dict(fonts)) = dr.get_mut(b"Font")
                    {
                        fonts.set(b"ZaDb".to_vec(), Object::Ref(font));
                    }
                });
            })
        },
        rows: &[("Signature1", Valid)],
    },
    Case {
        name: "a P=1 certification, then revocation data, then a document timestamp",
        file: || {
            let certified = sign(&form(), Some(1), None);
            let evidence = pdfcraft_sign::dss::Evidence { certs: vec![id().certificate.raw.clone()], ocsps: Vec::new(), crls: Vec::new() };
            let ltv = pdfcraft_sign::dss::embed(&open(&certified), &evidence).unwrap();
            pdfcraft_sign::timestamp_document(&open(&ltv), tsa(), "D:20261006120000Z").unwrap()
        },
        rows: &[("Signature1", Valid), ("DocumentTimestamp", Valid)],
    },
    Case {
        name: "an approval signature, then a save that respells the signed field and its value in hexadecimal",
        file: || {
            let signed = sign(&form(), None, Some("Approval"));
            let value = open(&signed).get(r(6)).as_dict().and_then(|f| f.get(b"V").and_then(Object::as_ref)).unwrap();
            edit_after(&signed, |doc| {
                for (at, key) in [(r(6), b"T".as_slice()), (value, b"M".as_slice())] {
                    doc.update_dict(at, |d| {
                        if let Some(Object::String(s)) = d.get(key).cloned() {
                            d.set(key.to_vec(), Object::String(PdfString { bytes: s.bytes, hex: true }));
                        }
                    })
                    .unwrap();
                }
            })
        },
        rows: &[("Approval", Valid)],
    },
];

/// Changes that must leave the earlier signature not Valid.
const ATTACKS: &[Case] = &[
    Case {
        name: "a P=2 certification, then a comment",
        file: || edit_after(&sign(&form(), Some(2), None), add_comment),
        rows: &[("Signature1", NotValid)],
    },
    Case {
        name: "a P=2 certification, then a field's appearance repainted without a new value",
        file: || {
            edit_after(&sign(&form(), Some(2), None), |doc| {
                let ap = appearance(doc, "Mallory");
                doc.update_dict(r(NAME), |w| w.set(b"AP".to_vec(), ap)).unwrap();
            })
        },
        rows: &[("Signature1", NotValid)],
    },
    Case {
        name: "an approval signature, then the page's text rewritten",
        file: || {
            edit_after(&sign(&form(), None, None), |doc| {
                let body = b"BT /F1 14 Tf 20 250 Td (Other text) Tj ET".to_vec();
                let mut d = Dict::new();
                d.set(b"Length".to_vec(), Object::Int(body.len() as i64));
                doc.set(r(4), Object::Stream(Stream::from_raw(d, body)));
            })
        },
        rows: &[("Signature1", NotValid)],
    },
    Case {
        name: "an approval signature, then a link added",
        file: || {
            edit_after(&sign(&form(), None, None), |doc| {
                let mut d = Dict::new();
                d.set(b"Type".to_vec(), Object::name("Annot"));
                d.set(b"Subtype".to_vec(), Object::name("Link"));
                d.set(b"Rect".to_vec(), Object::Array([20, 240, 200, 270].map(Object::Int).to_vec()));
                let uri = Dict::from_iter([
                    (b"S".to_vec(), Object::name("URI")),
                    (b"URI".to_vec(), Object::String(PdfString::text("https://example.com/"))),
                ]);
                d.set(b"A".to_vec(), Object::Dict(uri));
                let link = doc.add(Object::Dict(d));
                doc.update_dict(r(PAGE), |p| {
                    if let Some(Object::Array(a)) = p.get_mut(b"Annots") {
                        a.push(Object::Ref(link));
                    }
                })
                .unwrap();
            })
        },
        rows: &[("Signature1", NotValid)],
    },
    Case {
        name: "a P=1 certification, then a store whose dictionary carries a key no store has",
        file: || {
            edit_after(&sign(&form(), Some(1), None), |doc| {
                let mut dss = Dict::new();
                dss.set(b"Type".to_vec(), Object::name("DSS"));
                dss.set(b"Certs".to_vec(), Object::Array(Vec::new()));
                dss.set(b"Extra".to_vec(), PdfString::text("not a store key"));
                let dss = doc.add(Object::Dict(dss));
                let root = doc.root().unwrap();
                doc.update_dict(root, |c| c.set(b"DSS".to_vec(), Object::Ref(dss))).unwrap();
            })
        },
        rows: &[("Signature1", NotValid)],
    },
    Case {
        name: "a P=1 certification, then a form fill",
        file: || edit_after(&sign(&form(), Some(1), None), |doc| fill_name(doc, "Bob")),
        rows: &[("Signature1", NotValid)],
    },
    Case {
        name: "a P=1 certification, then an approval signature",
        file: || counter_sign(&sign(&form(), Some(1), None)),
        rows: &[("Signature1", NotValid)],
    },
];

/// Sign `doc` as it is, with edits not yet saved: they go into the signing revision.
fn sign_edited(bytes: &[u8], certify: Option<u8>, field: Option<&str>, edit: impl FnOnce(&mut Document)) -> Vec<u8> {
    let mut doc = open(bytes);
    edit(&mut doc);
    pdfcraft_sign::sign(&doc, &id(), &opts(certify, field, [150.0, 230.0, 280.0, 270.0])).unwrap()
}

/// An unsigned signature field "Witness", on the page and in the form.
fn add_unsigned_signature_field(doc: &mut Document) {
    let mut w = Dict::new();
    w.set(b"Type".to_vec(), Object::name("Annot"));
    w.set(b"Subtype".to_vec(), Object::name("Widget"));
    w.set(b"FT".to_vec(), Object::name("Sig"));
    w.set(b"T".to_vec(), PdfString::text("Witness"));
    w.set(b"Rect".to_vec(), Object::Array([20, 20, 140, 60].map(Object::Int).to_vec()));
    w.set(b"P".to_vec(), Object::Ref(r(PAGE)));
    let w = doc.add(Object::Dict(w));
    doc.update_dict(r(PAGE), |p| {
        if let Some(Object::Array(a)) = p.get_mut(b"Annots") {
            a.push(Object::Ref(w));
        }
    })
    .unwrap();
    edit_form(doc, |form| {
        if let Some(Object::Array(fields)) = form.get_mut(b"Fields") {
            fields.push(Object::Ref(w));
        }
    });
}

/// Changes the classifier before #7032 permitted, which the operations recogniser refuses: each
/// refusal is intended. Run beside it in shadow, the recogniser was never more permissive, and
/// these were the only cases of this table where it was stricter.
const NEWLY_REFUSED: &[Case] = &[
    Case {
        name: "an approval signature, then an unsigned signature field added (as in PDFBOX-6236)",
        file: || edit_after(&sign(&form(), None, None), add_unsigned_signature_field),
        rows: &[("Signature1", NotValid)],
    },
    Case {
        name: "an approval signature, then a signing step that also repaints a field whose value is unchanged (as in PDFBOX-4907)",
        file: || {
            sign_edited(&sign(&form(), None, None), None, None, |doc| {
                let ap = appearance(doc, "Ann");
                doc.update_dict(r(NAME), |w| w.set(b"AP".to_vec(), ap)).unwrap();
            })
        },
        rows: &[("Signature1", NotValid)],
    },
    Case {
        name: "a P=2 certification, then the empty field signed with its name and actions changed",
        file: || {
            sign_edited(&sign(&form(), Some(2), None), None, Some("Approver"), |doc| {
                doc.update_dict(r(6), |w| {
                    w.set(b"T".to_vec(), PdfString::text("Approver"));
                    w.set(b"AA".to_vec(), Object::Dict(Dict::new()));
                })
                .unwrap();
            })
        },
        rows: &[("Signature1", NotValid)],
    },
    Case {
        name: "a P=2 certification, then the form's /SigFlags changed with nothing signed",
        file: || edit_after(&sign(&form(), Some(2), None), |doc| edit_form(doc, |form| form.set(b"SigFlags".to_vec(), Object::Int(1)))),
        rows: &[("Signature1", NotValid)],
    },
];

/// Every case's rows, as each one is and as it must be; the ones that differ, said in full.
fn judge(cases: &[Case]) -> Vec<String> {
    let trust = trust();
    let mut wrong = Vec::new();
    for case in cases {
        let bytes = (case.file)();
        let rows = signatures(&open(&bytes), &bytes, &trust);
        for (field, expect) in case.rows {
            let Some(row) = rows.iter().find(|s| s.field == *field && s.signed) else {
                wrong.push(format!("{}: no signed row {field}", case.name));
                continue;
            };
            let valid = row.status == Status::Valid;
            if valid != (*expect == Valid) {
                wrong.push(format!("{}: {field} is {:?}, {:?}, expected {expect:?}; {:?}", case.name, row.status, row.modification, row.details));
            }
        }
    }
    wrong
}

#[test]
fn legitimate_workflows_keep_every_signature_valid() {
    let wrong = judge(LEGITIMATE);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn changes_no_signature_permits_are_not_valid() {
    let wrong = judge(ATTACKS);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn changes_refused_since_operations_explain_them_are_not_valid() {
    let wrong = judge(NEWLY_REFUSED);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The fill cases leave the earlier signature with permitted changes, not with none: the table
/// tests what follows a signature, not an untouched file.
#[test]
fn every_legitimate_case_changes_the_file_after_its_first_signature() {
    let trust = trust();
    for case in LEGITIMATE {
        let bytes = (case.file)();
        let first = signatures(&open(&bytes), &bytes, &trust).into_iter().find(|s| s.field == case.rows[0].0).unwrap();
        assert!(matches!(first.modification, Modification::Allowed(_)), "{}: {:?}", case.name, first.modification);
    }
}
