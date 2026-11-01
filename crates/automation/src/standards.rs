//! Standards tools beyond PDF/A: verifying PDF/X (ISO 15930), and Preflight (profiles of checks,
//! their results, reports and fixups).

use pdfcraft_engine::pdfx::Level;
use pdfcraft_engine::preflight::{self, Fixup, Profile};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed, write_atomic};

/// A profile as JSON: its checks with their parameters and severities.
fn profile_json(p: &Profile) -> Value {
    let checks: Vec<Value> = p
        .checks
        .iter()
        .map(|c| {
            let mut v = serde_json::to_value(c).unwrap_or(Value::Null);
            v["title"] = json!(c.check.said(&pdfcraft_engine::pdfa::in_english));
            v
        })
        .collect();
    json!({ "id": p.id, "name": p.name, "description": p.description, "checks": checks })
}

impl Automation {
    /// The profile a tool names: a built-in profile or single check by id, or a whole profile
    /// written as JSON.
    fn preflight_profile(&self, a: &Args) -> Result<Profile> {
        match a.get("profile") {
            None => Err(ToolError::InvalidArgs("missing argument profile (an id from preflight_profiles, or a profile object)".into())),
            Some(Value::String(id)) => {
                preflight::find(id).ok_or_else(|| ToolError::InvalidArgs(format!("no preflight profile {id:?} (see preflight_profiles)")))
            }
            Some(v @ Value::Object(_)) => Profile::from_json(&v.to_string()).map_err(ToolError::InvalidArgs),
            Some(_) => Err(ToolError::InvalidArgs("profile must be an id or a profile object".into())),
        }
    }

    fn preflight_results(&self, a: &Args) -> Result<preflight::Results> {
        let profile = self.preflight_profile(a)?;
        self.doc(a)?.preflight(&profile).ok_or_else(|| failed("the document has no object graph to preflight"))
    }

    pub(crate) fn preflight_profiles(&self) -> Result<Value> {
        let fixups: Vec<Value> = preflight::FIXUPS
            .iter()
            .map(|f| json!({ "fixup": f.id(), "params": serde_json::to_value(f).unwrap_or(Value::Null), "label": f.label() }))
            .collect();
        Ok(json!({
            "profiles": preflight::builtin().iter().map(profile_json).collect::<Vec<_>>(),
            "single_checks": preflight::single_checks().iter().map(profile_json).collect::<Vec<_>>(),
            "fixups": fixups,
        }))
    }

    pub(crate) fn preflight_run(&self, a: &Args) -> Result<Value> {
        let results = self.preflight_results(a)?;
        Ok(preflight::json(&results, &self.doc(a)?.name))
    }

    pub(crate) fn preflight_report(&self, a: &Args) -> Result<Value> {
        // Where the report goes is checked first: a path outside the root is refused before any work.
        let path = self.resolve(a.str("path")?, true)?;
        let results = self.preflight_results(a)?;
        let doc = self.doc(a)?;
        let format = a.opt_str("format")?.unwrap_or("text");
        let (y, m, d) = self.session.today();
        let date = format!("{y}-{m:02}-{d:02}");
        // A script's report is in English, like the rest of what the tools say.
        let words = pdfcraft_engine::pdfa::in_english;
        let bytes = match format {
            "text" => preflight::text(&results, &doc.name, &date, &words).into_bytes(),
            "json" => serde_json::to_vec_pretty(&preflight::json(&results, &doc.name)).map_err(failed)?,
            "pdf" => self.session.preflight_report_pdf(&results, &doc.name, &date, &words).map_err(failed)?.to_vec(),
            other => return Err(ToolError::InvalidArgs(format!("unknown report format {other:?} (text, json or pdf)"))),
        };
        write_atomic(&path, &bytes)?;
        Ok(json!({
            "path": path.to_string_lossy(),
            "format": format,
            "bytes": bytes.len(),
            "passed": results.passed(),
            "summary": {
                "errors": results.count(preflight::Severity::Error),
                "warnings": results.count(preflight::Severity::Warning),
                "info": results.count(preflight::Severity::Info),
            },
        }))
    }

    pub(crate) fn preflight_fix(&mut self, a: &Args) -> Result<Value> {
        let fixups = match a.opt_str("fixup")? {
            Some(id) => vec![
                Fixup::from_id(id, a.opt_bool("trapped")?)
                    .ok_or_else(|| ToolError::InvalidArgs(format!("unknown fixup {id:?} (set-trim-box, set-trapped or remove-scripts)")))?,
            ],
            // Analyze and fix: the fixups that address what the profile finds.
            None => self.preflight_results(a)?.fixups(),
        };
        if fixups.is_empty() {
            return Err(ToolError::Failed("the profile found nothing a fixup addresses".into()));
        }
        let id = self.doc(a)?.id;
        self.session.apply(id, pdfcraft_engine::Edit::PreflightFix { fixups: fixups.clone() }).map_err(failed)?;
        let mut out = json!({ "applied": fixups.iter().map(|f| serde_json::to_value(f).unwrap_or(Value::Null)).collect::<Vec<_>>() });
        if a.get("profile").is_some() {
            let results = self.preflight_results(a)?;
            out["results"] = preflight::json(&results, &self.doc(a)?.name);
        }
        Ok(out)
    }

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

    /// A print job: RGB content, a font that isn't embedded, a 100 ppi image, a JavaScript open
    /// action, no TrimBox and no Trapped key.
    fn print_job() -> Vec<u8> {
        let content = "1 0 0 rg 0 0 10 10 re f BT /F1 12 Tf (a) Tj ET q 216 0 0 216 10 10 cm /Im Do Q";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R /OpenAction 5 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /CropBox [10 10 590 790] /Contents 4 0 R \
             /Resources << /Font << /F1 6 0 R >> /XObject << /Im 7 0 R >> >> >>"
                .to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Action /S /JavaScript /JS (print()) >>".to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            "<< /Type /XObject /Subtype /Image /Width 300 /Height 300 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 0 >>\nstream\n\nendstream"
                .to_string(),
        ];
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

    fn check<'a>(r: &'a Value, id: &str) -> &'a Value {
        r["checks"].as_array().unwrap().iter().find(|c| c["check"] == id).unwrap_or_else(|| panic!("no {id} in {r}"))
    }

    /// The print job opened in a folder of its own named after `test`: the folder, the tools and
    /// the document's id.
    fn opened(test: &str) -> (std::path::PathBuf, Automation, Value) {
        let dir = std::env::temp_dir().join(format!("pdfcraft-automation-preflight-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("job.pdf"), print_job()).unwrap();
        let mut a = Automation::new().with_root(&dir).unwrap();
        let doc = json(&mut a, "doc_open", json!({ "path": "job.pdf" }))["doc"].clone();
        (dir, a, doc)
    }

    #[test]
    fn preflight_tools_list_profiles_and_run_them() {
        let (dir, mut a, doc) = opened("run");
        // The profiles, as the JSON a custom profile is written in.
        let listed = json(&mut a, "preflight_profiles", json!({}));
        let ids: Vec<&str> = listed["profiles"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
        assert_eq!(
            ids,
            ["commercial-print", "pdfx-1a-2001", "pdfx-1a-2003", "pdfx-3-2002", "pdfx-3-2003", "pdfx-4", "pdfa-2b", "pdfa-3b", "accessibility"]
        );
        assert_eq!(listed["profiles"][0]["checks"][3]["check"], "image-resolution");
        assert_eq!(listed["profiles"][0]["checks"][3]["colour_ppi"], 300.0);
        assert_eq!(listed["profiles"][0]["checks"][3]["title"], "Image resolution");
        assert!(listed["single_checks"].as_array().unwrap().iter().any(|p| p["id"] == "check:hairlines"));
        assert_eq!(listed["fixups"].as_array().unwrap().len(), 4);

        // Analyze: results by check, pages from 1; nothing changes.
        let r = json(&mut a, "preflight_run", json!({ "doc": doc, "profile": "commercial-print" }));
        assert_eq!((r["passed"].as_bool(), r["summary"]["errors"].as_u64()), (Some(false), Some(3)), "{r}");
        let fonts = check(&r, "fonts-embedded");
        assert_eq!((fonts["severity"].as_str(), fonts["status"].as_str()), (Some("error"), Some("found")));
        assert_eq!(fonts["hits"][0]["message"], "The font Helvetica isn't embedded");
        assert_eq!((fonts["hits"][0]["page"].as_u64(), fonts["hits"][0]["object"].as_u64()), (Some(1), Some(6)));
        assert_eq!(check(&r, "image-resolution")["hits"][0]["bbox"], json!([10.0, 10.0, 226.0, 226.0]));
        assert_eq!(check(&r, "trim-box")["fixups"], json!([{ "fixup": "set-trim-box" }]));
        // The accessibility profile runs the checker through the engine.
        let r = json(&mut a, "preflight_run", json!({ "doc": doc, "profile": "accessibility" }));
        assert_ne!(check(&r, "accessibility")["status"], "skipped", "{r}");
        // A single check, and a profile written as JSON.
        let r = json(&mut a, "preflight_run", json!({ "doc": doc, "profile": "check:scripts" }));
        assert_eq!(check(&r, "scripts")["hits"][0]["message"], "A JavaScript action isn't allowed");
        let custom = json!({ "id": "ours", "name": "Our press", "checks": [{ "check": "image-resolution", "severity": "warning", "colour_ppi": 90, "gray_ppi": 90, "bitmap_ppi": 90 }] });
        let r = json(&mut a, "preflight_run", json!({ "doc": doc, "profile": custom }));
        assert_eq!((r["passed"].as_bool(), check(&r, "image-resolution")["status"].as_str()), (Some(true), Some("passed")), "{r}");
        for bad in [json!("no-such-profile"), json!({ "id": "x", "name": "x", "checks": [] }), json!(7)] {
            assert!(matches!(a.call("preflight_run", &json!({ "doc": doc, "profile": bad })), Err(ToolError::InvalidArgs(_))), "{bad}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preflight_reports_are_written_inside_the_root_and_change_nothing() {
        let (dir, mut a, doc) = opened("report");
        // Reports: text, JSON and PDF, inside the root only.
        for (format, file) in [("text", "report.txt"), ("json", "report.json"), ("pdf", "report.pdf")] {
            let r = json(&mut a, "preflight_report", json!({ "doc": doc, "profile": "commercial-print", "format": format, "path": file }));
            assert_eq!((r["format"].as_str(), r["passed"].as_bool()), (Some(format), Some(false)), "{r}");
            let bytes = std::fs::read(dir.join(file)).unwrap();
            match format {
                "text" => {
                    assert!(String::from_utf8(bytes).unwrap().starts_with("Preflight report\nDocument: job.pdf\nProfile: Commercial print (CMYK)\n"))
                }
                "json" => assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap()["profile"]["id"], "commercial-print"),
                _ => assert!(bytes.starts_with(b"%PDF-")),
            }
        }
        assert!(a.call("preflight_report", &json!({ "doc": doc, "profile": "commercial-print", "path": "../escape.txt" })).is_err());
        assert!(matches!(
            a.call("preflight_report", &json!({ "doc": doc, "profile": "commercial-print", "path": "r.xml", "format": "xml" })),
            Err(ToolError::InvalidArgs(_))
        ));
        let listed = json(&mut a, "doc_list", json!({}));
        assert_eq!(listed["documents"][0]["dirty"], false, "analyzing and reporting change nothing: {listed}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preflight_fixups_are_undoable_steps() {
        let (dir, mut a, doc) = opened("fix");
        // One fixup: an undoable step.
        let r = json(&mut a, "preflight_fix", json!({ "doc": doc, "fixup": "set-trim-box" }));
        assert_eq!(r["applied"], json!([{ "fixup": "set-trim-box" }]));
        let r = json(&mut a, "preflight_run", json!({ "doc": doc, "profile": "check:trim-box" }));
        assert_eq!(check(&r, "trim-box")["status"], "passed");
        assert!(matches!(a.call("preflight_fix", &json!({ "doc": doc, "fixup": "set-trim-box" })), Err(ToolError::Failed(_))), "nothing left to fix");
        let undone = json(&mut a, "edit_undo", json!({ "doc": doc }));
        assert_eq!(undone["undone"], "Set missing TrimBoxes from the CropBox");
        // Analyze and fix: every fixup that addresses the results, as one step, and the results after.
        let r = json(&mut a, "preflight_fix", json!({ "doc": doc, "profile": "commercial-print" }));
        assert_eq!(r["applied"], json!([{ "fixup": "set-trim-box" }, { "fixup": "remove-scripts" }, { "fixup": "set-trapped", "trapped": false }]));
        for id in ["trim-box", "scripts", "trapped"] {
            assert_eq!(check(&r["results"], id)["status"], "passed", "{id}: {r}");
        }
        assert_eq!(check(&r["results"], "fonts-embedded")["status"], "found", "fonts can't be fixed here");
        let undone = json(&mut a, "edit_undo", json!({ "doc": doc }));
        assert_eq!(undone["undone"], "Apply preflight fixups");
        let r = json(&mut a, "preflight_fix", json!({ "doc": doc, "fixup": "set-trapped", "trapped": true }));
        assert_eq!(r["applied"], json!([{ "fixup": "set-trapped", "trapped": true }]));
        assert!(matches!(a.call("preflight_fix", &json!({ "doc": doc, "fixup": "convert-colours" })), Err(ToolError::InvalidArgs(_))));
        assert!(matches!(a.call("preflight_fix", &json!({ "doc": doc })), Err(ToolError::InvalidArgs(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
