//! Standards tools beyond PDF/A: verifying PDF/X (ISO 15930), Save as PDF/X, the printing
//! conditions they name, and the conversion part of Save as PDF/A.

use pdfcraft_engine::pdfa::output::{CHARACTERIZATIONS, ICC_REGISTRY, IccProfile, MAX_PROFILE, OutputCondition};
use pdfcraft_engine::pdfx::Level;
use pdfcraft_engine::{DocId, Edit, EditError};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed};

impl Automation {
    fn pdfx_level(a: &Args, declared: Option<Level>) -> Result<Level> {
        match a.opt_str("level")? {
            Some(l) => Level::from_id(l)
                .ok_or_else(|| ToolError::InvalidArgs(format!("unknown PDF/X level {l:?} (x1a:2001, x1a:2003, x3:2002, x3:2003 or x4)"))),
            // Without a level, the one the document declares; PDF/X-4 when it declares none.
            None => Ok(declared.unwrap_or(Level::X4)),
        }
    }

    pub(crate) fn pdfx_verify(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let level = Self::pdfx_level(a, doc.pdfx_declared())?;
        self.pdfx_result(doc.id, level)
    }

    fn pdfx_result(&self, id: DocId, level: Level) -> Result<Value> {
        let doc = self.session.get(id).ok_or_else(|| ToolError::Failed("no such document".into()))?;
        let findings = doc.pdfx_verify(level).ok_or_else(|| ToolError::Failed("the document has no object graph to verify".into()))?;
        let findings: Vec<Value> = findings
            .iter()
            .map(|f| json!({ "rule": f.rule, "clause": f.clause, "message": f.message, "page": f.page.map(|p| p + 1), "object": f.object }))
            .collect();
        let d = doc.standards();
        Ok(json!({
            "level": level.label(),
            "standard": level.standard(),
            "compliant": findings.is_empty(),
            "findings": findings,
            "declared": {
                "pdfx": d.pdfx,
                "pdfa": d.pdfa.map(|(p, c)| format!("PDF/A-{p}{}", c.to_lowercase())),
                "pdfua": d.pdfua,
                "output_intents": d.output_intents,
            },
        }))
    }

    /// The printing condition `icc` and `condition` name: the ICC profile file `icc` (inside the
    /// root), called `condition` when that is given; else the ICC registry characterization
    /// `condition`; else none (the level's default).
    pub(crate) fn output_condition(&self, a: &Args) -> Result<Option<OutputCondition>> {
        let name = a.opt_str("condition")?;
        let Some(icc) = a.opt_str("icc")? else {
            return match name {
                None => Ok(None),
                Some(n) => OutputCondition::registered(n).map(Some).ok_or_else(|| {
                    let known: Vec<&str> = CHARACTERIZATIONS.iter().map(|c| c.id).collect();
                    ToolError::InvalidArgs(format!(
                        "{n:?} isn't a characterization PdfCraft knows ({}); give its ICC profile with icc to use another printing condition",
                        known.join(", ")
                    ))
                }),
            };
        };
        let path = self.resolve(icc, false)?;
        let bytes = read_profile(&path).map_err(|e| ToolError::InvalidArgs(format!("{icc}: {e}")))?;
        let profile = IccProfile::read(bytes).map_err(|e| ToolError::InvalidArgs(format!("{icc}: {e}")))?;
        Ok(Some(OutputCondition::profile(profile, name.map(str::to_owned))))
    }

    /// Apply a Save as PDF/A or PDF/X conversion: what it changed, or what stands in its way
    /// (the document then unchanged).
    pub(crate) fn conversion(&mut self, id: DocId, edit: Edit) -> Result<Value> {
        match self.session.apply(id, edit) {
            Ok(()) => {
                let fixed = self.session.get(id).map(|d| d.converted.clone()).unwrap_or_default();
                Ok(json!({ "converted": true, "fixed": fixed, "blockers": [] }))
            }
            Err(EditError::Refused(r)) => {
                let blockers: Vec<Value> =
                    r.blockers.iter().map(|b| json!({ "message": b.message, "rule": b.rule, "page": b.page.map(|p| p + 1) })).collect();
                Ok(json!({ "converted": false, "fixed": [], "blockers": blockers }))
            }
            Err(e) => Err(failed(e)),
        }
    }

    pub(crate) fn pdfx_convert(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let id = doc.id;
        let level = Self::pdfx_level(a, Some(Level::X4))?;
        let condition = self.output_condition(a)?;
        let flatten = a.opt_bool("flatten")?.unwrap_or(false);
        let mut out = self.conversion(id, Edit::ConvertPdfX { level, condition, flatten })?;
        let verified = self.pdfx_result(id, level)?;
        if let (Some(out), Value::Object(v)) = (out.as_object_mut(), verified) {
            out.extend(v);
        }
        Ok(out)
    }

    pub(crate) fn output_conditions(&self) -> Result<Value> {
        let conditions: Vec<Value> = CHARACTERIZATIONS.iter().map(|c| json!({ "id": c.id, "condition": c.condition })).collect();
        Ok(json!({ "registry": ICC_REGISTRY, "default": CHARACTERIZATIONS.first().map(|c| c.id), "conditions": conditions }))
    }
}

/// An ICC profile file's bytes: refused when the path isn't a regular file (a device or a pipe
/// would never end), or when it holds more than [`MAX_PROFILE`] bytes, even if it grew after it
/// was measured.
pub(crate) fn read_profile(path: &std::path::Path) -> std::result::Result<Vec<u8>, String> {
    use std::io::Read;
    let too_large = || format!("an ICC profile can't be larger than {} MB", MAX_PROFILE / (1024 * 1024));
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > MAX_PROFILE as u64 {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path).and_then(|f| f.take(MAX_PROFILE as u64 + 1).read_to_end(&mut bytes)).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_PROFILE {
        return Err(too_large());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::{Automation, Content, ToolError};

    /// A page that conforms to PDF/X-1a:2003: CMYK content, a characterization in the ICC
    /// registry as its output intent, a TrimBox, `Trapped` and a document ID.
    fn pdfx1a() -> Vec<u8> {
        let content = "0 0 0 1 k 20 20 100 100 re f";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R /OutputIntents [<< /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (CGATS TR 001) \
             /RegistryName (http://www.color.org) >>] >>"
                .to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /TrimBox [10 10 190 290] /Contents 4 0 R /Resources << >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size 5 /Root 1 0 R /Info << /GTS_PDFXVersion (PDF/X-1a:2003) /Trapped /False >> /ID [<0A0B> <0A0B>] >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        out
    }

    fn json(a: &mut Automation, tool: &str, args: Value) -> Value {
        match a.call(tool, &args).map(|mut c| c.remove(0)) {
            Ok(Content::Json(v)) => v,
            other => panic!("{tool} {args}: {other:?}"),
        }
    }

    fn rules(r: &Value) -> Vec<&str> {
        r["findings"].as_array().unwrap().iter().map(|f| f["rule"].as_str().unwrap()).collect()
    }

    #[test]
    fn pdfx_verify_checks_the_declared_level_or_the_one_asked_for() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-automation-pdfx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x1a.pdf"), pdfx1a()).unwrap();
        let mut a = Automation::new().with_root(&dir).unwrap();
        let doc = json(&mut a, "doc_open", json!({ "path": "x1a.pdf" }))["doc"].clone();

        // Without a level, the one the document declares.
        let r = json(&mut a, "pdfx_verify", json!({ "doc": doc }));
        assert_eq!(
            (r["level"].as_str(), r["standard"].as_str(), r["compliant"].as_bool()),
            (Some("PDF/X-1a:2003"), Some("ISO 15930-4:2003"), Some(true)),
            "{r}"
        );
        assert_eq!(r["declared"]["pdfx"], "PDF/X-1a:2003", "{r}");
        assert_eq!(r["declared"]["output_intents"], json!(["CGATS TR 001"]), "{r}");

        // As PDF/X-4 it lacks the metadata, and its output intent embeds no profile.
        let r = json(&mut a, "pdfx_verify", json!({ "doc": doc, "level": "x4" }));
        assert_eq!((r["level"].as_str(), r["compliant"].as_bool()), (Some("PDF/X-4"), Some(false)), "{r}");
        assert!(rules(&r).contains(&"pdfx.identification") && rules(&r).contains(&"pdfx.output-intent"), "{r}");

        // A document that declares nothing is checked as PDF/X-4; pages count from 1.
        let plain = json(&mut a, "doc_create", json!({ "from": "text", "text": "Proof" }))["doc"].clone();
        let r = json(&mut a, "pdfx_verify", json!({ "doc": plain }));
        assert_eq!(r["level"], "PDF/X-4", "{r}");
        let boxes = r["findings"].as_array().unwrap().iter().find(|f| f["rule"] == "pdfx.page-boxes").unwrap_or_else(|| panic!("{r}"));
        assert_eq!(boxes["page"], 1, "{r}");
        assert!(boxes["object"].as_u64().is_some() && boxes["clause"] == "CGATS notes 2.10", "{r}");

        // It changes nothing, and an unknown level is refused.
        let listed = json(&mut a, "doc_list", json!({}));
        let entry = listed["documents"].as_array().unwrap().iter().find(|d| d["doc"] == doc).unwrap_or_else(|| panic!("{listed}"));
        assert_eq!(entry["dirty"], false, "{listed}");
        assert!(matches!(a.call("pdfx_verify", &json!({ "doc": doc, "level": "x5" })), Err(ToolError::InvalidArgs(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A working folder holding `page.pdf` (one page with `content`) and `press.icc`, a synthetic
    /// CMYK printer profile.
    fn folder(name: &str, content: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfcraft-automation-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let page = pdfcraft_testkit::Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
            .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Contents 4 0 R /Resources << >> >>")
            .stream("", content)
            .build();
        std::fs::write(dir.join("page.pdf"), page).unwrap();
        std::fs::write(dir.join("press.icc"), pdfcraft_testkit::icc::cmyk_printer_profile("Test press (synthetic)")).unwrap();
        dir
    }

    fn messages(r: &Value) -> Vec<&str> {
        r["blockers"].as_array().unwrap().iter().map(|b| b["message"].as_str().unwrap()).collect()
    }

    #[test]
    fn pdfx_convert_writes_a_file_that_verifies_or_says_why_not() {
        let dir = folder("pdfx-convert", b"1 0 0 rg 20 20 100 100 re f");
        fn ticking() -> i64 {
            static NOW: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1_700_000_000);
            NOW.fetch_add(61, std::sync::atomic::Ordering::SeqCst)
        }
        let mut a = Automation::new().with_root(&dir).unwrap().with_clock(ticking);
        let doc = json(&mut a, "doc_open", json!({ "path": "page.pdf" }))["doc"].clone();

        // PDF/X-4 without a profile, and PDF/X-1a with RGB but no profile to convert it with:
        // refused, and the document is as it was.
        let r = json(&mut a, "pdfx_convert", json!({ "doc": doc }));
        assert_eq!((r["converted"].as_bool(), r["compliant"].as_bool()), (Some(false), Some(false)), "{r}");
        assert!(messages(&r).contains(&"PDF/X-4 embeds the printing condition's ICC profile: choose a profile file"), "{r}");
        let r = json(&mut a, "pdfx_convert", json!({ "doc": doc, "level": "x1a:2003", "condition": "FOGRA51" }));
        assert!(messages(&r).contains(&"Converting colour to CMYK needs the printing condition's ICC profile: choose a profile file"), "{r}");
        let listed = json(&mut a, "doc_list", json!({}));
        assert_eq!(listed["documents"][0]["dirty"], false, "{listed}");

        // With the press's profile the red turns into ink, and the file saved verifies.
        let r = json(&mut a, "pdfx_convert", json!({ "doc": doc, "level": "x1a:2003", "icc": "press.icc" }));
        assert_eq!((r["converted"].as_bool(), r["compliant"].as_bool()), (Some(true), Some(true)), "{r}");
        assert_eq!(r["declared"]["pdfx"], "PDF/X-1a:2003", "{r}");
        assert_eq!(r["declared"]["output_intents"], json!(["Test press (synthetic)"]), "{r}");
        assert!(r["fixed"].as_array().unwrap().iter().any(|f| f.as_str().unwrap().starts_with("Converted 1 colours")), "{r}");
        json(&mut a, "doc_save", json!({ "doc": doc, "path": "press.pdf" }));
        let saved = std::fs::read(dir.join("press.pdf")).unwrap();
        assert!(saved.starts_with(b"%PDF-1.4"), "{:?}", String::from_utf8_lossy(&saved[..8]));
        // The save didn't stamp a later modification date into the document information alone:
        // it still says what the metadata says (the clock moves on with every reading).
        let file = pdfcraft_cos::Document::open(std::sync::Arc::new(saved.clone())).unwrap();
        let info = file.trailer().get(b"Info").map(|i| file.resolve(i)).unwrap();
        let mod_date = info.as_dict().unwrap().get(b"ModDate").and_then(|d| d.as_string()).unwrap().to_text();
        let catalog = file.get(file.root().unwrap());
        let metadata = file.resolve(catalog.as_dict().unwrap().get(b"Metadata").unwrap());
        let pdfcraft_cos::Object::Stream(metadata) = &*metadata else { panic!("no metadata stream") };
        let xmp = String::from_utf8(metadata.decoded().unwrap()).unwrap();
        let modify = xmp.split("<xmp:ModifyDate>").nth(1).and_then(|r| r.split('<').next()).unwrap();
        let digits = |s: &str| s.chars().filter(char::is_ascii_digit).take(14).collect::<String>();
        assert_eq!(digits(&mod_date), digits(modify), "{mod_date} / {modify}");
        let reopened = json(&mut a, "doc_open", json!({ "path": "press.pdf" }))["doc"].clone();
        let r = json(&mut a, "pdfx_verify", json!({ "doc": reopened }));
        assert_eq!((r["level"].as_str(), r["compliant"].as_bool()), (Some("PDF/X-1a:2003"), Some(true)), "{r}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn printing_conditions_are_named_and_profiles_stay_inside_the_root() {
        let dir = folder("pdfx-conditions", b"0 0 0 1 k");
        let mut a = Automation::new().with_root(&dir).unwrap();
        let r = json(&mut a, "output_conditions", json!({}));
        assert_eq!((r["default"].as_str(), r["registry"].as_str()), (Some("FOGRA39"), Some("http://www.color.org")), "{r}");
        assert!(r["conditions"].as_array().unwrap().iter().any(|c| c["id"] == "CGATS TR 006"), "{r}");
        let doc = json(&mut a, "doc_open", json!({ "path": "page.pdf" }))["doc"].clone();
        // An unknown condition, a profile outside the root, and a file that isn't a profile.
        assert!(matches!(a.call("pdfx_convert", &json!({ "doc": doc, "condition": "FOGRA99" })), Err(ToolError::InvalidArgs(_))));
        assert!(a.call("pdfx_convert", &json!({ "doc": doc, "level": "x4", "icc": "../press.icc" })).is_err());
        assert!(matches!(a.call("pdfx_convert", &json!({ "doc": doc, "icc": "page.pdf" })), Err(ToolError::InvalidArgs(_))));
        // A folder is no profile file: refused before anything is read.
        std::fs::create_dir_all(dir.join("profiles.icc")).unwrap();
        match a.call("pdfx_convert", &json!({ "doc": doc, "icc": "profiles.icc" })) {
            Err(ToolError::InvalidArgs(m)) => assert!(m.contains("not a file"), "{m}"),
            other => panic!("{other:?}"),
        }
        // A CMYK page and a registered condition need no profile for PDF/X-1a.
        let r = json(&mut a, "pdfx_convert", json!({ "doc": doc, "level": "x1a:2001", "condition": "cgats tr 006" }));
        assert_eq!((r["converted"].as_bool(), r["compliant"].as_bool()), (Some(true), Some(true)), "{r}");
        assert_eq!(r["declared"]["output_intents"], json!(["CGATS TR 006"]), "{r}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pdfa_convert_takes_a_level_and_a_cmyk_profile() {
        let dir = folder("pdfa-convert", b"0 0 0 1 k 20 20 100 100 re f");
        let mut a = Automation::new().with_root(&dir).unwrap();
        let doc = json(&mut a, "doc_open", json!({ "path": "page.pdf" }))["doc"].clone();
        let r = json(&mut a, "pdfa_convert", json!({ "doc": doc, "level": "2u" }));
        assert_eq!(r["converted"], false, "{r}");
        assert!(
            messages(&r).contains(&"The document uses CMYK colour: choose the ICC profile of a CMYK printing condition for its output intent"),
            "{r}"
        );
        let r = json(&mut a, "pdfa_convert", json!({ "doc": doc, "level": "2u", "icc": "press.icc", "condition": "My press" }));
        assert_eq!((r["converted"].as_bool(), r["compliant"].as_bool(), r["level"].as_str()), (Some(true), Some(true), Some("PDF/A-2u")), "{r}");
        assert_eq!(r["declared"]["pdfa"], "PDF/A-2u", "{r}");
        assert_eq!(r["declared"]["output_intents"], json!(["My press"]), "{r}");
        assert!(matches!(a.call("pdfa_convert", &json!({ "doc": doc, "level": "4f" })), Err(ToolError::InvalidArgs(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
