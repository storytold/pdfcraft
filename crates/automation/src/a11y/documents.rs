use super::{Args, Automation, Result, ToolError, Value, child, failed, json, unused, write_atomic};

impl Automation {
    pub(crate) fn doc_export_office(&self, a: &Args) -> Result<Value> {
        let path = self.resolve(a.str("path")?, true)?;
        let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let format = pdfcraft_engine::compare::OfficeFormat::from_extension(&ext)
            .ok_or_else(|| ToolError::InvalidArgs(format!("unsupported extension {ext:?} (docx, pptx, html or rtf)")))?;
        let export = self.doc_to_copy(a)?.export_office_checked(format);
        write_atomic(&path, &export.bytes)?;
        // Text left out to keep a hostile page bounded is said, not silent (#3120).
        let truncated: Vec<usize> = export.truncated_pages.iter().map(|p| p.saturating_add(1)).collect();
        let mut out = json!({ "path": path.to_string_lossy(), "bytes": export.bytes.len(), "format": format.extension(),
            "truncated": !truncated.is_empty(), "truncated_pages": truncated });
        if !export.truncated_pages.is_empty() {
            use pdfcraft_engine::compare::{MAX_FORM_DEPTH, MAX_FORM_VISITS};
            out["warnings"] = json!([format!(
                "forms nested more than {MAX_FORM_DEPTH} deep, or past the first {MAX_FORM_VISITS} forms of a page (forms the page draws itself count too), were left out on the pages in truncated_pages: their text and images are missing from the export, although the viewer may still show them"
            )]);
        }
        Ok(out)
    }

    fn pdfa_level(&self, a: &Args) -> Result<pdfcraft_engine::pdfa::Level> {
        let l = a.opt_str("level")?.unwrap_or("2b");
        pdfcraft_engine::pdfa::Level::from_id(l).ok_or_else(|| ToolError::InvalidArgs(format!("unknown PDF/A level {l:?} (2b or 3b)")))
    }

    pub(crate) fn pdfa_verify(&self, a: &Args) -> Result<Value> {
        let level = self.pdfa_level(a)?;
        let doc = self.doc(a)?;
        let issues: Vec<Value> = doc
            .pdfa_verify(level)
            .iter()
            .map(|i| json!({ "clause": i.clause, "message": i.message, "page": i.page.map(|p| p + 1), "fixable": i.fixable }))
            .collect();
        let d = doc.standards();
        Ok(json!({
            "level": level.label(),
            "compliant": issues.is_empty(),
            "issues": issues,
            "declared": { "pdfa": d.pdfa.map(|(p, c)| format!("PDF/A-{p}{}", c.to_lowercase())), "pdfua": d.pdfua, "output_intents": d.output_intents },
        }))
    }

    pub(crate) fn print_readiness(&self, a: &Args) -> Result<Value> {
        let report = self.doc(a)?.print_readiness();
        let findings: Vec<Value> =
            report.findings.iter().map(|f| json!({ "id": f.id, "severity": f.severity, "message": f.message, "page": f.page })).collect();
        let pages: Vec<Value> = report
            .pages
            .iter()
            .map(|p| {
                json!({
                    "page": p.page,
                    "media_mm": p.media_mm,
                    "crop_mm": p.crop_mm,
                    "trim_mm": p.trim_mm,
                    "bleed_mm": p.bleed_mm,
                    "trim_set": p.trim_set,
                    "bleed_set": p.bleed_set,
                    "bleed_margin_mm": p.bleed_margin_mm,
                })
            })
            .collect();
        let images: Vec<Value> = report
            .images
            .iter()
            .map(|image| {
                json!({
                    "page": image.page,
                    "name": image.name,
                    "ppi": image.ppi,
                    "pixels": image.pixels,
                    "size_mm": image.size_mm,
                })
            })
            .collect();
        let pdfa: Vec<Value> = report
            .pdfa_issues
            .iter()
            .map(|issue| json!({ "clause": issue.clause, "message": issue.message, "page": issue.page.map(|p| p + 1) }))
            .collect();
        Ok(json!({
            "text": report.text(),
            "findings": findings,
            "pages": pages,
            "images": images,
            "output_intents": report.output_intents,
            "fonts_not_embedded": report.fonts_not_embedded,
            "pdfa_issues": pdfa,
            "resolution_unknown": report.resolution_unknown,
        }))
    }

    pub(crate) fn pdfa_convert(&mut self, a: &Args) -> Result<Value> {
        let level = self.pdfa_level(a)?;
        let id = self.doc(a)?.id;
        self.session.apply(id, pdfcraft_engine::Edit::ConvertPdfA { level }).map_err(failed)?;
        self.pdfa_verify(a)
    }

    pub(crate) fn action_list(&self) -> Result<Value> {
        use pdfcraft_engine::actions::{Step, builtin};
        let step_json = |s: &Step| json!({ "step": s.id(), "arg": s.arg() });
        let actions: Vec<Value> = builtin()
            .iter()
            .map(|a| json!({ "name": a.name, "description": a.description, "steps": a.steps.iter().map(step_json).collect::<Vec<_>>() }))
            .collect();
        let steps: Vec<Value> = Step::all().iter().map(|s| json!({ "step": s.id(), "label": s.label(), "takes_arg": s.arg().is_some() })).collect();
        Ok(json!({ "actions": actions, "steps": steps }))
    }

    pub(crate) fn action_run(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::actions::{Action, Step, builtin, run_on};
        let action = if let Some(name) = a.opt_str("action")? {
            builtin()
                .into_iter()
                .find(|x| x.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| ToolError::InvalidArgs(format!("no built-in action {name:?}")))?
        } else {
            let items = a.get("steps").and_then(Value::as_array).ok_or_else(|| ToolError::InvalidArgs("give action or steps".into()))?;
            let steps = items
                .iter()
                .map(|it| {
                    let id = it["step"].as_str().unwrap_or("");
                    Step::from_id(id, it["arg"].as_str().unwrap_or("")).ok_or_else(|| ToolError::InvalidArgs(format!("unknown step {id:?}")))
                })
                .collect::<Result<Vec<_>>>()?;
            Action { name: "Custom".into(), description: String::new(), steps, builtin: false }
        };
        let folder = self.resolve(a.str("folder")?, true)?;
        std::fs::create_dir_all(&folder).map_err(|e| failed(e.to_string()))?;
        let mut out = Vec::new();
        for p in a.strs("paths")? {
            let name = std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "document.pdf".into());
            let result = self
                .resolve(p, false)
                .map_err(|e| e.to_string())
                .and_then(|src| crate::read_input_bytes(&src).map_err(|e| e.to_string()))
                .and_then(|bytes| run_on(&action, &name, std::sync::Arc::new(bytes), |_, _| {}));
            out.push(match result {
                Ok(r) => {
                    let target = child(&folder, &name);
                    let target = if target.exists() {
                        let stem =
                            std::path::Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
                        unused(&folder, &stem)
                    } else {
                        target
                    };
                    if target.exists() {
                        return Err(failed(format!("{}: no unused output name available", target.display())));
                    }
                    write_atomic(&target, &r.bytes)?;
                    json!({ "path": p, "output": target.to_string_lossy(), "log": r.log })
                }
                Err(e) => json!({ "path": p, "error": e }),
            });
        }
        Ok(json!({ "action": action.name, "files": out }))
    }

    fn compare_ids(&self, a: &Args) -> Result<(pdfcraft_engine::DocId, pdfcraft_engine::DocId)> {
        let new = self.doc(a)?.id;
        let other = a.int("other")?;
        let old = u64::try_from(other)
            .ok()
            .and_then(|o| self.session.get(pdfcraft_engine::DocId(o)))
            .map(|d| d.id)
            .ok_or_else(|| ToolError::InvalidArgs(format!("no open document {other}")))?;
        Ok((old, new))
    }

    pub(crate) fn doc_compare(&self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::compare::Kind;
        let (old, new) = self.compare_ids(a)?;
        let c = self.session.compare(old, new).map_err(failed)?;
        let limit = a.opt_int("limit")?.unwrap_or(500).max(1) as usize;
        let r2 = |r: &[f64; 4]| r.map(|v| (v * 100.0).round() / 100.0);
        let side =
            |s: &pdfcraft_engine::compare::Side| json!({ "text": s.text, "page": s.page + 1, "rects": s.rects.iter().map(r2).collect::<Vec<_>>() });
        let list: Vec<Value> = c
            .changes
            .iter()
            .take(limit)
            .map(|ch| json!({ "kind": ch.kind.label().to_lowercase(), "old": side(&ch.old), "new": side(&ch.new) }))
            .collect();
        let visual: Vec<Value> = if a.opt_bool("visual")?.unwrap_or(false) {
            self.session.compare_visual(old, new, 72.0).map_err(failed)?.iter().map(|(p, r)| json!({ "page": p + 1, "rect": r2(r) })).collect()
        } else {
            Vec::new()
        };
        Ok(json!({
            "visual": visual,
            "identical": c.identical(),
            "replaced": c.count(Kind::Replaced),
            "inserted": c.count(Kind::Inserted),
            "deleted": c.count(Kind::Deleted),
            "old_words": c.old_words,
            "new_words": c.new_words,
            "changes": list,
        }))
    }

    pub(crate) fn doc_compare_report(&self, a: &Args) -> Result<Value> {
        let (old, new) = self.compare_ids(a)?;
        let bytes = self.session.compare_report(old, new).map_err(failed)?;
        let path = self.resolve(a.str("path")?, true)?;
        write_atomic(&path, &bytes)?;
        Ok(json!({ "path": path.to_string_lossy(), "bytes": bytes.len() }))
    }

    pub(crate) fn doc_compare_mark(&mut self, a: &Args) -> Result<Value> {
        let (old, new) = self.compare_ids(a)?;
        let n = self.session.mark_differences(old, new).map_err(failed)?;
        Ok(json!({ "comments": n }))
    }
}
