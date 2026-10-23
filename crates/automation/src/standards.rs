//! Standards tools beyond PDF/A: verifying PDF/X (ISO 15930).

use pdfcraft_engine::pdfx::Level;
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError};

impl Automation {
    pub(crate) fn pdfx_verify(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        // Without a level, the one the document declares; PDF/X-4 when it declares none.
        let level = match a.opt_str("level")? {
            Some(l) => Level::from_id(l)
                .ok_or_else(|| ToolError::InvalidArgs(format!("unknown PDF/X level {l:?} (x1a:2001, x1a:2003, x3:2002, x3:2003 or x4)")))?,
            None => doc.pdfx_declared().unwrap_or(Level::X4),
        };
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
}
