//! What goes beyond editing a page: form fields to add, passwords and permissions, layers, a
//! signature to draw on the page, and a digital signature made with a certificate. Boxes and points
//! are displayed points from the page's top-left, like the rest of the module.

use std::sync::Arc;

use pdfcraft_engine::{Changes, Edit, FieldProps, MarkFile, NewField, Printing, Protection, SignOptions};
use serde_json::{Value, json};

use crate::core::Document;

fn boxed(v: &Value, key: &str) -> Result<[f64; 4], String> {
    let a = v.get(key).and_then(Value::as_array).filter(|a| a.len() == 4).ok_or_else(|| format!("{key} is [x0, y0, x1, y1]"))?;
    let mut out = [0.0; 4];
    for (slot, n) in out.iter_mut().zip(a) {
        *slot = n.as_f64().filter(|n| n.is_finite()).ok_or("a box is finite numbers")?;
    }
    Ok(out)
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).take(500).collect()).unwrap_or_default()
}

impl Document {
    /// Adds a form field over a displayed box. `spec` is `{"type": "text"|"multiline"|"date"|"check"|"radio"|
    /// "combo"|"list"|"button", "name"?, "options"?, "group"?, "export"?, "caption"?}`.
    pub fn add_field(&mut self, page: usize, view: [f64; 4], spec: &str) -> Result<(), String> {
        let v: Value = serde_json::from_str(spec).map_err(|e| e.to_string())?;
        if !view.iter().all(|n| n.is_finite()) {
            return Err("a box is finite numbers".into());
        }
        let rect = self.page_info_ref(page)?.view_rect_to_user([view[0] as f32, view[1] as f32, view[2] as f32, view[3] as f32]).map(f64::from);
        let kind = match v["type"].as_str().ok_or("a field has a type")? {
            "text" => NewField::Text { multiline: false },
            "multiline" => NewField::Text { multiline: true },
            "date" => NewField::Date,
            "check" => NewField::CheckBox,
            "radio" => {
                NewField::Radio { group: v["group"].as_str().map(str::to_string), export: v["export"].as_str().unwrap_or("Choice").to_string() }
            }
            "combo" => NewField::Combo { options: strings(&v, "options"), editable: v["editable"].as_bool().unwrap_or(false) },
            "list" => NewField::List { options: strings(&v, "options"), multi: v["multi"].as_bool().unwrap_or(false) },
            "button" => NewField::Button { caption: v["caption"].as_str().unwrap_or("Button").to_string() },
            other => return Err(format!("unknown field type {other:?}")),
        };
        let name = v["name"].as_str().filter(|n| !n.is_empty()).map(str::to_string);
        self.apply(Edit::AddField { page, rect, kind, name })
    }

    pub fn delete_field(&mut self, name: &str) -> Result<(), String> {
        self.apply(Edit::DeleteField { name: name.to_string() })
    }

    /// Changes a field: `{"name"?, "tooltip"?, "required"?, "readOnly"?, "options"?, "fontSize"?, "box"?}`; the box
    /// (displayed points) moves and resizes its first widget.
    pub fn set_field_props(&mut self, name: &str, spec: &str) -> Result<(), String> {
        let v: Value = serde_json::from_str(spec).map_err(|e| e.to_string())?;
        // where its first widget is: a displayed box, as the other calls take
        let rect = match v.get("box") {
            None | Some(Value::Null) => None,
            Some(_) => {
                let b = boxed(&v, "box")?;
                let page = self.doc()?.info.fields.iter().find(|f| f.name == name).and_then(|f| f.page).ok_or("that field is not on a page")?;
                Some((0, self.page_info_ref(page)?.view_rect_to_user([b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32]).map(f64::from)))
            }
        };
        let props = FieldProps {
            rect,
            name: v["name"].as_str().map(str::to_string),
            tooltip: v["tooltip"].as_str().map(str::to_string),
            required: v["required"].as_bool(),
            read_only: v["readOnly"].as_bool(),
            options: v.get("options").map(|_| strings(&v, "options")),
            font_size: v["fontSize"].as_f64().filter(|n| n.is_finite() && (0.0..=200.0).contains(n)),
            ..FieldProps::default()
        };
        self.apply(Edit::SetFieldProps { name: name.to_string(), props: Box::new(props) })
    }

    /// Protects with passwords: `{"open"?, "owner"?, "printing": "none"|"low"|"high", "changes":
    /// "none"|"pages"|"fill"|"comment"|"any", "copy", "accessibility"}`. The next save is encrypted.
    pub fn protect(&mut self, spec: &str) -> Result<(), String> {
        let v: Value = serde_json::from_str(spec).map_err(|e| e.to_string())?;
        let non_empty = |key: &str| v[key].as_str().filter(|s| !s.is_empty()).map(str::to_string);
        let protection = Protection {
            open_password: non_empty("open"),
            permissions_password: non_empty("owner"),
            printing: match v["printing"].as_str().unwrap_or("high") {
                "none" => Printing::None,
                "low" => Printing::Low,
                "high" => Printing::High,
                other => return Err(format!("unknown printing level {other:?}")),
            },
            changes: match v["changes"].as_str().unwrap_or("none") {
                "none" => Changes::None,
                "pages" => Changes::Pages,
                "fill" => Changes::FillSign,
                "comment" => Changes::CommentFillSign,
                "any" => Changes::AnyExceptExtract,
                other => return Err(format!("unknown changes level {other:?}")),
            },
            copy: v["copy"].as_bool().unwrap_or(false),
            accessibility: v["accessibility"].as_bool().unwrap_or(true),
            ..Protection::default()
        };
        if protection.open_password.is_none() && protection.permissions_password.is_none() {
            return Err("give a password to open the file, to change its permissions, or both".into());
        }
        self.apply(Edit::Protect(protection))
    }

    pub fn remove_protection(&mut self) -> Result<(), String> {
        self.apply(Edit::RemoveProtection)
    }

    /// The layers (optional content groups) as JSON: `index`, `name`, `visible`.
    pub fn layers_json(&self) -> Result<String, String> {
        let list: Vec<Value> =
            self.doc()?.info.layers.iter().enumerate().map(|(index, l)| json!({ "index": index, "name": l.name, "visible": l.visible })).collect();
        serde_json::to_string(&list).map_err(|e| e.to_string())
    }

    /// Shows or hides a layer for viewing; true when that changed what is shown. The file keeps its own choice.
    pub fn set_layer_visible(&mut self, index: usize, visible: bool) -> bool {
        let changed = self.session.set_layer_visible(self.id, index, visible);
        if changed {
            self.drop_renderer();
        }
        changed
    }

    /// Puts a picture on the page as a stamp over a displayed box (an image file: PNG, JPEG, TIFF, GIF, BMP).
    pub fn add_picture_stamp(&mut self, page: usize, view: [f64; 4], name: &str, bytes: &[u8]) -> Result<(), String> {
        if !view.iter().all(|n| n.is_finite()) {
            return Err("a box is finite numbers".into());
        }
        let rect = self.page_info_ref(page)?.view_rect_to_user([view[0] as f32, view[1] as f32, view[2] as f32, view[3] as f32]).map(f64::from);
        self.apply(Edit::AddCustomStamp {
            page,
            rect,
            name: name.to_string(),
            file: MarkFile { name: name.to_string(), bytes: Arc::new(bytes.to_vec()), page },
            author: String::new(),
        })
    }

    /// Bakes the typed text, marks and signatures added so far into the page, so they can no longer be moved.
    pub fn flatten_fill_sign(&mut self) -> Result<(), String> {
        self.apply(Edit::FlattenFillSign)
    }

    /// The signatures of the document as JSON: `field`, `signed`, `signer`, `date`, `reason`, `status`
    /// (`valid`, `invalid`, `unknown` or `unsigned`), `summary`, `page`.
    pub fn signatures_json(&self) -> Result<String, String> {
        let list: Vec<Value> = self
            .doc()?
            .signatures
            .iter()
            .map(|s| {
                json!({
                    "field": s.field,
                    "signed": s.signed,
                    "signer": s.signer,
                    "date": s.date,
                    "reason": s.reason,
                    "page": s.page,
                    "summary": s.summary(),
                    "status": format!("{:?}", s.status).to_lowercase(),
                })
            })
            .collect();
        serde_json::to_string(&list).map_err(|e| e.to_string())
    }

    /// Makes a new digital ID (a self-signed certificate with a fresh P-256 key) and returns it as a PKCS #12
    /// file protected by `password`. `now` is the Unix time in seconds, which the certificate starts at.
    pub fn create_digital_id(name: &str, organisation: &str, email: &str, password: &str, years: u32, now: i64) -> Result<Vec<u8>, String> {
        use pdfcraft_engine::sign::{Certificate, DigitalId, PrivateKey, Time, pkcs12, x509::Name};
        if name.trim().is_empty() || name.len() > 200 || password.is_empty() || !(1..=30).contains(&years) {
            return Err("a digital ID needs a name, a password and between 1 and 30 years".into());
        }
        let key = PrivateKey::generate_p256().map_err(|e| e.to_string())?;
        let subject = Name::build(name, "", organisation, email, "");
        let serial = pdfcraft_engine::sign::keys::DigestAlg::Sha256.digest(&[name.as_bytes(), &now.to_be_bytes()])[..8].to_vec();
        let certificate = Certificate::self_signed(&subject, &key, Time::from_unix(now), years, &serial).map_err(|e| e.to_string())?;
        let id = DigitalId { key, certificate, chain: Vec::new(), friendly_name: Some(name.to_string()) };
        pkcs12::write(&id, password).map_err(|e| e.to_string())
    }

    /// Signs the document with a certificate file (PKCS #12: .p12 or .pfx) and its password, and returns the
    /// signed PDF. `spec`: `{"page"?, "box"?` (visible when given, displayed points), `"field"?`, `"reason"?`,
    /// `"location"?`, `"contact"?`, `"certify"?` (1 to 3), `"name"?` (a new field's name)`}`. The document is
    /// final once signed: change nothing after this.
    pub fn sign_with_certificate(&mut self, p12: &[u8], password: &str, spec: &str) -> Result<Vec<u8>, String> {
        let v: Value = serde_json::from_str(spec).map_err(|e| e.to_string())?;
        let id = pdfcraft_engine::sign::pkcs12::open(p12, password).map_err(|e| e.to_string())?;
        let page = v["page"].as_u64().map_or(0, |p| p as usize);
        let rect = match v.get("box") {
            None | Some(Value::Null) => None,
            Some(_) => {
                let b = boxed(&v, "box")?;
                Some(self.page_info_ref(page)?.view_rect_to_user([b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32]).map(f64::from))
            }
        };
        let opts = SignOptions {
            field: v["field"].as_str().map(str::to_string),
            page,
            rect,
            new_field_name: v["name"].as_str().map(str::to_string),
            reason: v["reason"].as_str().map(str::to_string),
            location: v["location"].as_str().map(str::to_string),
            contact: v["contact"].as_str().map(str::to_string),
            certify: v["certify"].as_u64().filter(|c| (1..=3).contains(c)).map(|c| c as u8),
            ..SignOptions::default()
        };
        let signed = self.session.sign(self.id, &id, opts).map_err(|e| e.to_string())?;
        Ok(signed.as_ref().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tests::fixture;

    fn open(pages: usize) -> Document {
        Document::open(&fixture(pages), None, || 1_700_000_000).expect("opens")
    }

    #[test]
    fn adds_changes_and_deletes_fields() {
        let mut doc = open(1);
        let kinds = [
            r#"{"type":"text","name":"Name"}"#,
            r#"{"type":"multiline","name":"Notes"}"#,
            r#"{"type":"date","name":"When"}"#,
            r#"{"type":"check","name":"Agree"}"#,
            r#"{"type":"radio","group":"Size","export":"Small"}"#,
            r#"{"type":"combo","name":"Pick","options":["a","b"]}"#,
            r#"{"type":"list","name":"Many","options":["x","y","z"],"multi":true}"#,
            r#"{"type":"button","name":"Go","caption":"Send"}"#,
        ];
        for (i, spec) in kinds.iter().enumerate() {
            let top = 10.0 + 30.0 * i as f64;
            doc.add_field(0, [20.0, top, 120.0, top + 22.0], spec).unwrap_or_else(|e| panic!("{spec}: {e}"));
        }
        let fields: Vec<Value> = serde_json::from_str(&doc.fields_json(0).expect("fields")).expect("json");
        assert_eq!(fields.len(), kinds.len());
        doc.set_field_props("Name", r#"{"tooltip":"Your name","required":true}"#).expect("changes it");
        doc.set_field_props("Name", r#"{"box":[30,40,150,70]}"#).expect("moves and resizes it");
        let fields: Vec<Value> = serde_json::from_str(&doc.fields_json(0).expect("fields")).expect("json");
        let moved = fields.iter().find(|f| f["name"] == "Name").expect("still there");
        let v: Vec<f64> = moved["view"].as_array().expect("view").iter().map(|n| n.as_f64().expect("n")).collect();
        assert!(
            (v[0] - 30.0).abs() < 1.0 && (v[1] - 40.0).abs() < 1.0 && (v[2] - 150.0).abs() < 1.0 && (v[3] - 70.0).abs() < 1.0,
            "where it was put: {v:?}"
        );
        assert!(doc.set_field_props("Nobody", r#"{"box":[0,0,10,10]}"#).is_err());
        doc.delete_field("Notes").expect("deletes it");
        let fields: Vec<Value> = serde_json::from_str(&doc.fields_json(0).expect("fields")).expect("json");
        assert_eq!(fields.len(), kinds.len() - 1);
        assert!(doc.add_field(0, [0.0, 0.0, 10.0, 10.0], r#"{"type":"slider"}"#).is_err());
        assert!(doc.add_field(0, [0.0, 0.0, f64::NAN, 10.0], r#"{"type":"text"}"#).is_err());
        assert!(doc.add_field(3, [0.0, 0.0, 10.0, 10.0], r#"{"type":"text"}"#).is_err(), "no such page");
    }

    #[test]
    fn protects_and_the_saved_file_asks_for_the_password() {
        let mut doc = open(1);
        assert!(doc.protect(r#"{}"#).is_err(), "a password is needed");
        assert!(doc.protect(r#"{"open":"secret","printing":"loud"}"#).is_err());
        doc.protect(r#"{"open":"secret","owner":"boss","printing":"low","changes":"fill","copy":false}"#).expect("protects");
        let saved = doc.save_full().expect("saves");
        assert!(matches!(Document::open(&saved, None, || 1), Err(crate::core::OpenFailure::NeedsPassword)), "a password is asked for");
        let mut again = Document::open(&saved, Some("secret"), || 1).expect("opens with the password");
        assert_eq!(again.page_count(), 1);
        again.remove_protection().expect_err("the owner password is needed to lift it");
    }

    #[test]
    fn lists_layers_and_draws_signatures() {
        let mut doc = open(1);
        assert_eq!(doc.layers_json().expect("layers"), "[]");
        assert!(!doc.set_layer_visible(0, true), "no such layer");
        doc.add_annotation(0, r#"{"type":"signature","strokes":[[[20,100],[40,120],[60,100]]],"color":[0,0,0.5]}"#).expect("draws a signature");
        doc.add_annotation(0, r#"{"type":"mark","box":[20,150,36,166],"mark":"check"}"#).expect("adds a mark");
        doc.add_annotation(0, r#"{"type":"typewriter","box":[20,180,150,200],"fontSize":12,"contents":"Typed"}"#).expect("types text");
        assert!(doc.add_annotation(0, r#"{"type":"signature","strokes":[]}"#).is_err());
        doc.flatten_fill_sign().expect("bakes them in");
    }

    #[test]
    fn makes_a_digital_id_and_signs_with_it() {
        assert!(Document::create_digital_id("", "", "", "pw", 1, 1_700_000_000).is_err());
        assert!(Document::create_digital_id("Ada", "", "", "", 1, 1_700_000_000).is_err());
        let p12 = Document::create_digital_id("Ada Example", "Example Ltd", "ada@example.org", "pw", 2, 1_700_000_000).expect("makes an ID");
        let mut doc = open(1);
        assert!(doc.sign_with_certificate(&p12, "wrong", "{}").is_err(), "the wrong password");
        let signed = doc.sign_with_certificate(&p12, "pw", r#"{"page":0,"box":[20,200,160,260],"reason":"I agree"}"#).expect("signs");
        assert!(signed.starts_with(b"%PDF"));
        let reopened = Document::open(&signed, None, || 1_700_000_000).expect("the signed file opens");
        let sigs: Vec<Value> = serde_json::from_str(&reopened.signatures_json().expect("signatures")).expect("json");
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0]["signed"], true);
        assert_eq!(sigs[0]["signer"], "Ada Example");
    }
}
