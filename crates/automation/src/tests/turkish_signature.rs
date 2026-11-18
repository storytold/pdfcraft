//! Typed signature graphics preserve shaped ink; they are not searchable text.
use super::*;

const NFC: &str = "Çağrı İstanbul Şişli Özgür Âdem";
const NFD: &str = "C\u{327}ag\u{306}rı I\u{307}stanbul S\u{327}is\u{327}li O\u{308}zgu\u{308}r A\u{302}dem";

fn blank(a: &mut Automation, degrees: i64) -> u64 {
    let id = json(a, "doc_create", json!({"from":"blank","width":800,"height":600}))["doc"].as_u64().unwrap();
    if degrees != 0 {
        json(a, "page_rotate", json!({"doc":id,"pages":[1],"degrees":degrees}));
    }
    id
}

fn add(a: &mut Automation, id: u64, kind: &str, text: &str) {
    let result = json(a, "fill_sign_add", json!({"doc":id,"page":1,"type":kind,"text":text,"at":[50,150]}));
    assert!(result["warnings"].as_array().is_none_or(Vec::is_empty), "{result}");
}

#[test]
fn nfd_signature_graphics_match_composed_ink_and_survive_save_reopen_undo() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = Automation::new().with_root(dir.path()).unwrap().with_clock(|| 1_700_000_000);
    for (kind, nfc, nfd) in [("signature", NFC, NFD), ("initials", "İŞ", "I\u{307}S\u{327}")] {
        let composed = blank(&mut a, 0);
        add(&mut a, composed, kind, nfc);
        let expected = a.render_pixels(composed, 1, 1.0).unwrap();
        let id = blank(&mut a, 0);
        let empty = a.render_pixels(id, 1, 1.0).unwrap();
        add(&mut a, id, kind, nfd);
        assert_eq!(a.render_pixels(id, 1, 1.0).unwrap(), expected, "{kind}: equivalent spelling must have the same shaped ink");
        assert_ne!(expected, empty);
        json(&mut a, "edit_undo", json!({"doc":id}));
        assert_eq!(a.render_pixels(id, 1, 1.0).unwrap(), empty);
        json(&mut a, "edit_redo", json!({"doc":id}));
        assert_eq!(a.render_pixels(id, 1, 1.0).unwrap(), expected);
        let filename = format!("Çalışmalar – {kind} NFD.pdf");
        json(&mut a, "doc_save", json!({"doc":id,"path":filename}));
        let mut reopened = Automation::new().with_root(dir.path()).unwrap();
        let re = json(&mut reopened, "doc_open", json!({"path":filename}))["doc"].as_u64().unwrap();
        assert_eq!(reopened.render_pixels(re, 1, 1.0).unwrap(), expected);
        assert_eq!(
            json(&mut reopened, "text_extract", json!({"doc":re}))["pages"][0]["text"],
            "",
            "an outline signature must not claim text semantics"
        );
        json(&mut reopened, "doc_flatten", json!({"doc":re,"comments":true,"fields":false}));
        assert_eq!(reopened.render_pixels(re, 1, 1.0).unwrap(), expected);
    }
}

#[test]
fn nfd_signature_graphics_remain_upright_on_rotated_pages() {
    let mut a = Automation::new();
    for degrees in [90, 180, 270] {
        let nfc = blank(&mut a, degrees);
        let nfd = blank(&mut a, degrees);
        add(&mut a, nfc, "signature", NFC);
        add(&mut a, nfd, "signature", NFD);
        assert_eq!(a.render_pixels(nfd, 1, 1.0).unwrap(), a.render_pixels(nfc, 1, 1.0).unwrap(), "page rotation {degrees}");
    }
}

#[test]
fn nfd_signature_bad_clusters_and_scalar_limit_refuse_without_mutating() {
    let mut a = Automation::new();
    let id = blank(&mut a, 0);
    let before = a.session.save_bytes(DocId(id)).unwrap();
    let too_long = format!("{}g\u{306}", "a".repeat(pdfcraft_engine::MAX_SIGNATURE_CHARS - 1));
    for kind in ["signature", "initials"] {
        for text in ["\u{306}g", "g\u{1ab0}", "missing 🦀", too_long.as_str()] {
            let error = a.call("fill_sign_add", &json!({"doc":id,"page":1,"type":kind,"text":text,"at":[50,150]})).unwrap_err();
            if text == too_long {
                assert!(error.to_string().contains("at most 256 characters"), "{error}");
            }
            assert_eq!(a.session.save_bytes(DocId(id)).unwrap(), before, "a refused signature must keep the document intact");
        }
    }
}
