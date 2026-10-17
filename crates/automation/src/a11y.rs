//! Accessibility tools: the full check, the accessibility report, and the automatic fixes.

use pdfcraft_engine::DocId;
use pdfcraft_engine::a11y::{Category, Options, Report, Rule, Status};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

use crate::{Args, Automation, Result, ToolError, child, failed, unused, write_atomic};

fn status_id(s: Status) -> &'static str {
    match s {
        Status::Passed => "passed",
        Status::Failed => "failed",
        Status::Manual => "manual",
        Status::Skipped => "skipped",
    }
}

fn category_id(c: Category) -> &'static str {
    match c {
        Category::Document => "document",
        Category::PageContent => "page_content",
        Category::Forms => "forms",
        Category::AlternateText => "alternate_text",
        Category::Tables => "tables",
        Category::Lists => "lists",
        Category::Headings => "headings",
    }
}

pub(crate) fn report_json(r: &Report) -> Value {
    let results: Vec<Value> = r
        .results
        .iter()
        .map(|x| {
            let findings: Vec<Value> = x
                .findings
                .iter()
                .map(|f| {
                    json!({
                        "page": f.page.map(|p| p + 1),
                        "object": f.object.map(|r| r.num),
                        "object_generation": f.object.map(|r| r.generation),
                        "bbox_user_space": f.bbox,
                        "message": f.message,
                    })
                })
                .collect();
            json!({ "rule": x.rule.id(), "name": x.rule.name(), "category": category_id(x.rule.category()), "status": status_id(x.status), "findings": findings })
        })
        .collect();
    json!({
        "passed": r.count(Status::Passed),
        "failed": r.count(Status::Failed),
        "manual": r.count(Status::Manual),
        "skipped": r.count(Status::Skipped),
        "results": results,
    })
}

fn review_hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn review_decisions(value: &Value, pages: usize) -> Result<Vec<Value>> {
    let rows = value.as_array().ok_or_else(|| ToolError::InvalidArgs("decisions must be an array".into()))?;
    let mut seen = BTreeSet::new();
    rows.iter()
        .map(|row| {
            let fields = row.as_object().ok_or_else(|| ToolError::InvalidArgs("each decision must be an object".into()))?;
            if fields.keys().any(|key| !matches!(key.as_str(), "rule" | "decision" | "page" | "note")) {
                return Err(ToolError::InvalidArgs("unknown decision property".into()));
            }
            let rule = row["rule"].as_str().and_then(Rule::from_id).ok_or_else(|| ToolError::InvalidArgs("decision has an unknown rule".into()))?;
            let decision = row["decision"].as_str().ok_or_else(|| ToolError::InvalidArgs("decision status is required".into()))?;
            if !matches!(decision, "accepted" | "needs_work" | "skipped") {
                return Err(ToolError::InvalidArgs("decision must be accepted, needs_work or skipped".into()));
            }
            let page = match row.get("page").filter(|value| !value.is_null()) {
                Some(value) => {
                    let page = value.as_u64().ok_or_else(|| ToolError::InvalidArgs("decision page must be positive".into()))?;
                    if page == 0 || page > pages as u64 {
                        return Err(ToolError::InvalidArgs("decision page is outside the document".into()));
                    }
                    Some(page)
                }
                None => None,
            };
            let note = match row.get("note").filter(|value| !value.is_null()) {
                Some(value) => {
                    let note = value.as_str().ok_or_else(|| ToolError::InvalidArgs("decision note must be text".into()))?;
                    if note.len() > 4096 {
                        return Err(ToolError::InvalidArgs("decision note exceeds 4096 bytes".into()));
                    }
                    Some(note)
                }
                None => None,
            };
            if !seen.insert((rule.id(), page)) {
                return Err(ToolError::InvalidArgs("duplicate decision for a rule and page".into()));
            }
            Ok(json!({ "rule": rule.id(), "decision": decision, "page": page, "note": note }))
        })
        .collect()
}

/// A review sidecar is a few KiB of JSON; anything larger is not one.
const MAX_REVIEW_SIDECAR_BYTES: u64 = 1 << 20;

/// Read a review sidecar without trusting its size: the path is the caller's
/// and may name any file, so the read stops past `MAX_REVIEW_SIDECAR_BYTES` and
/// an oversized file is refused as not being a sidecar.
fn read_review_sidecar(path: &std::path::Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path).and_then(|file| file.take(MAX_REVIEW_SIDECAR_BYTES.saturating_add(1)).read_to_end(&mut bytes)).map_err(failed)?;
    if bytes.len() as u64 > MAX_REVIEW_SIDECAR_BYTES {
        return Err(failed("file is not an accessibility review sidecar"));
    }
    Ok(bytes)
}

fn parse_review_sidecar(bytes: &[u8]) -> Result<Value> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| failed("file is not an accessibility review sidecar"))?;
    let hash = value["sha256"].as_str().ok_or_else(|| failed("review sidecar has no document hash"))?;
    let Some(pages) = value["pages"].as_u64().and_then(|n| usize::try_from(n).ok()).filter(|n| *n > 0) else {
        return Err(failed("unsupported or invalid accessibility review sidecar"));
    };
    if value["schema"] != 1 || hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(failed("unsupported or invalid accessibility review sidecar"));
    }
    review_decisions(&value["decisions"], pages).map_err(failed)?;
    Ok(value)
}

fn html_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// A self-contained, human-readable view of the same evidence as the JSON review record.
fn review_html(record: &Value) -> Result<String> {
    let str_at = |value: &Value, key: &str| -> Result<String> {
        value.get(key).and_then(Value::as_str).map(html_escape).ok_or_else(|| failed(format!("review record is missing {key}")))
    };
    let count = |value: &Value, key: &str| -> Result<u64> {
        value.get(key).and_then(Value::as_u64).ok_or_else(|| failed(format!("review record is missing {key}")))
    };
    let before = &record["before"];
    let after = &record["after"];
    let mut html = format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><title>Accessibility review</title><style>\
         body{{font:15px/1.5 system-ui,sans-serif;max-width:1100px;margin:2rem auto;padding:0 1rem;color:#1d1d1f}}\
         table{{border-collapse:collapse;width:100%;margin:1rem 0}}th,td{{border:1px solid #aaa;padding:.45rem;text-align:left;vertical-align:top}}\
         th{{background:#f2f2f2}}.hash{{overflow-wrap:anywhere;font-family:monospace}}\
         </style></head><body><main><h1>Accessibility review</h1><p>Record created: {}</p>\n",
        str_at(record, "date")?
    );
    html.push_str("<h2>Before and after</h2><table><caption>Accessibility checker outcomes</caption><thead><tr><th scope=\"col\">Version</th><th scope=\"col\">Document</th><th scope=\"col\">Passed</th><th scope=\"col\">Failed</th><th scope=\"col\">Needs manual check</th><th scope=\"col\">Skipped</th></tr></thead><tbody>");
    for (label, version) in [("Before", before), ("After", after)] {
        let summary = &version["summary"];
        html.push_str(&format!(
            "<tr><th scope=\"row\">{label}</th><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            str_at(version, "name")?,
            count(summary, "passed")?,
            count(summary, "failed")?,
            count(summary, "manual")?,
            count(summary, "skipped")?
        ));
    }
    html.push_str("</tbody></table>");
    html.push_str(&format!(
        "<p>Before SHA-256: <span class=\"hash\">{}</span><br>After SHA-256: <span class=\"hash\">{}</span></p>",
        str_at(before, "sha256")?,
        str_at(after, "sha256")?
    ));
    html.push_str("<h2>Rule comparison</h2><table><caption>Before and after checker results</caption><thead><tr><th scope=\"col\">Rule</th><th scope=\"col\">Before</th><th scope=\"col\">Before findings</th><th scope=\"col\">After</th><th scope=\"col\">After findings</th><th scope=\"col\">Change</th></tr></thead><tbody>");
    let rules = record["rules"].as_array().ok_or_else(|| failed("review record is missing rules"))?;
    for row in rules {
        let id = row["rule"].as_str().ok_or_else(|| failed("review record has a rule without an id"))?;
        let name = Rule::from_id(id).map(Rule::name).unwrap_or(id);
        let old = &row["before"];
        let new = &row["after"];
        let old_status = str_at(old, "status")?;
        let new_status = str_at(new, "status")?;
        let old_findings = count(old, "findings")?;
        let new_findings = count(new, "findings")?;
        let changed = old_status != new_status || old_findings != new_findings;
        html.push_str(&format!(
            "<tr><th scope=\"row\">{}</th><td>{old_status}</td><td>{old_findings}</td><td>{new_status}</td><td>{new_findings}</td><td>{}</td></tr>",
            html_escape(name),
            if changed { "Changed" } else { "Unchanged" }
        ));
    }
    html.push_str("</tbody></table><h2>Still requiring review</h2><ul>");
    for id in record["review_required"].as_array().ok_or_else(|| failed("review record is missing review_required"))? {
        let id = id.as_str().ok_or_else(|| failed("review record has an invalid review_required entry"))?;
        html.push_str(&format!("<li>{}</li>", html_escape(Rule::from_id(id).map(Rule::name).unwrap_or(id))));
    }
    html.push_str("</ul>");
    html.push_str(&format!("<p>{}</p></main></body></html>\n", str_at(record, "note")?));
    Ok(html)
}

/// Tags in one page of `accessibility_tags`, by default and at most.
const TAG_PAGE: usize = 200;
const MAX_TAG_PAGE: usize = 1000;

impl Automation {
    pub(crate) fn accessibility_autotag_apply(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        self.session.apply(id, pdfcraft_engine::Edit::AutoTagText).map_err(failed)?;
        let tags = self.accessibility_tags(a)?;
        Ok(json!({
            "tags": tags["tags"], "count": tags["count"],
            "note": "Text objects were tagged as paragraphs. Review heading roles, reading order and remaining accessibility findings manually.",
        }))
    }

    pub(crate) fn accessibility_autotag_form_fields(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        self.session.apply(id, pdfcraft_engine::Edit::AutoTagFormFields).map_err(failed)?;
        let report = self
            .doc(a)?
            .accessibility_check(&Options { rules: [Rule::TaggedFormFields].into(), pages: None })
            .ok_or_else(|| failed("the document can't be read"))?;
        let result = report.result(Rule::TaggedFormFields).ok_or_else(|| failed("the form field check is unavailable"))?;
        Ok(
            json!({ "status": status_id(result.status), "remaining_findings": result.findings.len(), "note": "Only form widgets were tagged. Review text, figures, links, reading order, and other document content separately." }),
        )
    }

    pub(crate) fn accessibility_autotag_preview(&self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let preview = self.doc(a)?.autotag_preview(page).map_err(failed)?;
        let candidates: Vec<Value> = preview
            .candidates
            .into_iter()
            .map(|c| {
                json!({
                    "type": c.kind, "reason": c.reason, "text": c.text,
                    "text_truncated": c.text_truncated, "bbox_user_space": c.rect,
                    "font_size": c.size, "bold": c.bold, "needs_review": true,
                })
            })
            .collect();
        Ok(json!({
            "page": page + 1, "candidates": candidates,
            "unclassified_images": preview.unclassified_images,
            "unclassified_text_blocks": preview.unclassified_text_blocks,
            "existing_structure": preview.existing_structure,
            "note": "Text roles are proposals only. Review images, tables, lists, annotations and existing tags before applying structure.",
        }))
    }

    fn a11y_options(&self, a: &Args) -> Result<Options> {
        let mut o = Options::default();
        if a.opt_bool("all")?.unwrap_or(false) {
            o.rules = Rule::ALL.into_iter().collect();
        }
        if a.get("rules").is_some() {
            o.rules = a
                .strs("rules")?
                .iter()
                .map(|id| Rule::from_id(id).ok_or_else(|| ToolError::InvalidArgs(format!("unknown rule {id:?}"))))
                .collect::<Result<_>>()?;
        }
        if a.get("categories").is_some() {
            let cats: Vec<&str> = a.strs("categories")?;
            for c in &cats {
                if !Category::ALL.iter().any(|k| category_id(*k) == *c) {
                    return Err(ToolError::InvalidArgs(format!("unknown category {c:?}")));
                }
            }
            o.rules.retain(|r| cats.contains(&category_id(r.category())));
        }
        if a.opt_ints("pages")?.is_some() {
            o.pages = Some(self.pages(a, "pages")?);
        }
        Ok(o)
    }

    pub(crate) fn a11y_check(&self, a: &Args) -> Result<Value> {
        let o = self.a11y_options(a)?;
        let r = self.doc(a)?.accessibility_check(&o).ok_or_else(|| failed("the document can't be read for checking"))?;
        Ok(report_json(&r))
    }

    /// Report only untagged page content, annotations, and AcroForm widgets.
    pub(crate) fn a11y_find_untagged(&self, a: &Args) -> Result<Value> {
        let pages = a.opt_ints("pages")?.map(|_| self.pages(a, "pages")).transpose()?;
        let rules = [Rule::TaggedContent, Rule::TaggedAnnotations, Rule::TaggedFormFields].into_iter().collect();
        let report = self.doc(a)?.accessibility_check(&Options { rules, pages }).ok_or_else(|| failed("the document can't be read for checking"))?;
        Ok(report_json(&report))
    }

    pub(crate) fn a11y_report(&self, a: &Args) -> Result<Value> {
        let o = self.a11y_options(a)?;
        let doc = self.doc(a)?;
        let r = doc.accessibility_check(&o).ok_or_else(|| failed("the document can't be read for checking"))?;
        let path = self.resolve(a.str("path")?, true)?;
        let (y, m, d) = self.session.today();
        // A script's report is in English, like the rest of what the tools say and answer with.
        let words = pdfcraft_engine::a11y::in_english;
        let html = pdfcraft_engine::a11y::report_html(&r, &doc.name, &format!("{y}-{m:02}-{d:02}"), "en", &words);
        write_atomic(&path, html.as_bytes())?;
        let mut out = report_json(&r);
        out["path"] = json!(path.to_string_lossy());
        Ok(out)
    }

    fn review_record_value(&self, a: &Args) -> Result<Value> {
        let id = |key| -> Result<DocId> {
            let n = u64::try_from(a.int(key)?).map_err(|_| ToolError::InvalidArgs(format!("{key} must be positive")))?;
            if n == 0 {
                return Err(ToolError::InvalidArgs(format!("{key} must be positive")));
            }
            Ok(DocId(n))
        };
        let before_id = id("before")?;
        let after_id = id("after")?;
        if before_id == after_id {
            return Err(ToolError::InvalidArgs("before and after must be separate open documents".into()));
        }
        let before = self.session.get(before_id).ok_or_else(|| failed(format!("no open document with id {}", before_id.0)))?;
        let after = self.session.get(after_id).ok_or_else(|| failed(format!("no open document with id {}", after_id.0)))?;
        let mut options = Options::default();
        if a.opt_bool("all")?.unwrap_or(false) {
            options.rules = Rule::ALL.into_iter().collect();
        }
        let before_report = before.accessibility_check(&options).ok_or_else(|| failed("the original document can't be read for checking"))?;
        let after_report = after.accessibility_check(&options).ok_or_else(|| failed("the edited document can't be read for checking"))?;
        if before_report.results.len() != after_report.results.len() {
            return Err(failed("the two accessibility checks returned different rule sets"));
        }
        let mut rules = Vec::new();
        let mut changed = Vec::new();
        let mut review_required = Vec::new();
        for (prior, current) in before_report.results.iter().zip(&after_report.results) {
            if prior.rule != current.rule {
                return Err(failed("the two accessibility checks returned different rule sets"));
            }
            let rule = prior.rule.id();
            let before_status = status_id(prior.status);
            let after_status = status_id(current.status);
            let before_findings = prior.findings.len();
            let after_findings = current.findings.len();
            if before_status != after_status || before_findings != after_findings {
                changed.push(rule);
            }
            if matches!(current.status, Status::Failed | Status::Manual) {
                review_required.push(rule);
            }
            rules.push(json!({
                "rule": rule,
                "before": { "status": before_status, "findings": before_findings },
                "after": { "status": after_status, "findings": after_findings },
            }));
        }
        let (y, m, d) = self.session.today();
        let hash = |bytes: &[u8]| Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect::<String>();
        let record = json!({
            "schema": 1,
            "date": format!("{y}-{m:02}-{d:02}"),
            "before": { "name": before.name, "sha256": hash(before.bytes.as_slice()), "summary": {
                "passed": before_report.count(Status::Passed), "failed": before_report.count(Status::Failed),
                "manual": before_report.count(Status::Manual), "skipped": before_report.count(Status::Skipped),
            } },
            "after": { "name": after.name, "sha256": hash(after.bytes.as_slice()), "summary": {
                "passed": after_report.count(Status::Passed), "failed": after_report.count(Status::Failed),
                "manual": after_report.count(Status::Manual), "skipped": after_report.count(Status::Skipped),
            } },
            "rules": rules,
            "changed": changed,
            "review_required": review_required,
            "note": "Automated checks cover only part of accessibility. Review remaining findings manually and validate the final PDF independently.",
        });
        Ok(record)
    }

    pub(crate) fn a11y_review_record(&self, a: &Args) -> Result<Value> {
        let record = self.review_record_value(a)?;
        let path = self.resolve(a.str("path")?, true)?;
        let bytes = serde_json::to_vec_pretty(&record).map_err(failed)?;
        write_atomic(&path, &bytes)?;
        Ok(json!({ "path": path.to_string_lossy(), "record": record }))
    }

    pub(crate) fn a11y_review_save(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let decisions = review_decisions(a.get("decisions").ok_or_else(|| Args::missing("decisions"))?, doc.info.pages.len())?;
        let hash = review_hash(doc.bytes.as_slice());
        let path = self.resolve(a.str("path")?, true)?;
        if path.exists() {
            let prior = read_review_sidecar(&path)?;
            let prior = parse_review_sidecar(&prior)?;
            if prior["sha256"] != hash && !a.opt_bool("replace")?.unwrap_or(false) {
                return Err(failed("review sidecar belongs to a different PDF revision; pass replace: true to replace it"));
            }
        }
        let (y, m, d) = self.session.today();
        let sidecar =
            json!({ "schema": 1, "date": format!("{y}-{m:02}-{d:02}"), "sha256": hash, "pages": doc.info.pages.len(), "decisions": decisions });
        write_atomic(&path, &serde_json::to_vec_pretty(&sidecar).map_err(failed)?)?;
        Ok(json!({ "path": path.to_string_lossy(), "review": sidecar }))
    }

    pub(crate) fn a11y_review_load(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let path = self.resolve(a.str("path")?, false)?;
        let review = parse_review_sidecar(&read_review_sidecar(&path)?)?;
        if review["sha256"] != review_hash(doc.bytes.as_slice()) {
            return Err(failed("review sidecar belongs to a different PDF revision"));
        }
        Ok(json!({ "path": path.to_string_lossy(), "review": review }))
    }

    pub(crate) fn a11y_review_html(&self, a: &Args) -> Result<Value> {
        let record = self.review_record_value(a)?;
        let html = review_html(&record)?;
        let path = self.resolve(a.str("path")?, true)?;
        write_atomic(&path, html.as_bytes())?;
        Ok(json!({ "path": path.to_string_lossy(), "record": record }))
    }

    pub(crate) fn a11y_fix(&mut self, a: &Args) -> Result<Value> {
        let id = a.str("rule")?;
        let rule = Rule::from_id(id).ok_or_else(|| ToolError::InvalidArgs(format!("unknown rule {id:?}")))?;
        let edit = self.doc(a)?.accessibility_fix(rule, a.opt_str("value")?).map_err(failed)?;
        let doc = self.doc(a)?.id;
        self.session.apply(doc, edit).map_err(failed)?;
        let r =
            self.doc(a)?.accessibility_check(&Options { rules: [rule].into(), pages: None }).ok_or_else(|| failed("the document can't be read"))?;
        Ok(json!({ "rule": id, "status": status_id(r.results.iter().find(|x| x.rule == rule).map_or(Status::Skipped, |x| x.status)) }))
    }

    pub(crate) fn accessibility_figures(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let list: Vec<Value> = doc
            .figures()
            .iter()
            .map(|f| {
                // Top-left-origin points on the displayed page.
                let rect = f.page.zip(f.bbox).and_then(|(p, b)| {
                    let info = doc.info.pages.get(p)?;
                    let (u, v) = (info.user_to_view(b[0] as f32, b[1] as f32), info.user_to_view(b[2] as f32, b[3] as f32));
                    let r = |x: f32| (x as f64 * 100.0).round() / 100.0;
                    Some([r(u[0].min(v[0])), r(u[1].min(v[1])), r(u[0].max(v[0])), r(u[1].max(v[1]))])
                });
                json!({ "figure": f.obj.num, "page": f.page.map(|p| p + 1), "alt": f.alt, "rect": rect })
            })
            .collect();
        Ok(json!({ "count": list.len(), "figures": list }))
    }

    pub(crate) fn accessibility_set_alt(&mut self, a: &Args) -> Result<Value> {
        let figure = u32::try_from(a.int("figure")?)
            .map_err(|_| ToolError::InvalidArgs("figure must be a figure number from accessibility_figures".into()))?;
        let edit = if a.opt_bool("decorative")?.unwrap_or(false) {
            pdfcraft_engine::Edit::MarkDecorative { figure }
        } else {
            pdfcraft_engine::Edit::SetAltText { figure, alt: a.opt_str("alt")?.map(str::to_owned) }
        };
        let id = self.doc(a)?.id;
        self.session.apply(id, edit).map_err(failed)?;
        self.accessibility_figures(a)
    }

    /// The structure tags, one page at a time (`offset`, `limit`), optionally only some types, pages
    /// (1-based) or depths. `count` is how many match; `next` is where the following page starts.
    pub(crate) fn accessibility_tags(&self, a: &Args) -> Result<Value> {
        let offset = usize::try_from(a.opt_int("offset")?.unwrap_or(0)).map_err(|_| ToolError::InvalidArgs("offset must be 0 or more".into()))?;
        let limit = a.opt_int("limit")?.unwrap_or(TAG_PAGE as i64);
        let limit = usize::try_from(limit)
            .ok()
            .filter(|l| (1..=MAX_TAG_PAGE).contains(l))
            .ok_or_else(|| ToolError::InvalidArgs(format!("limit must be from 1 to {MAX_TAG_PAGE}")))?;
        let types: Option<Vec<&str>> = a.get("types").map(|_| a.strs("types")).transpose()?;
        let pages: Option<Vec<usize>> = a
            .opt_ints("pages")?
            .map(|p| p.into_iter().map(|n| usize::try_from(n).ok().filter(|n| *n >= 1).map(|n| n - 1)).collect::<Option<Vec<_>>>())
            .map(|p| p.ok_or_else(|| ToolError::InvalidArgs("pages must be 1-based page numbers".into())))
            .transpose()?;
        let max_depth = a
            .opt_int("max_depth")?
            .map(|d| usize::try_from(d).map_err(|_| ToolError::InvalidArgs("max_depth must be 0 or more".into())))
            .transpose()?;
        let matching: Vec<_> = self
            .doc(a)?
            .tags()
            .into_iter()
            .filter(|t| types.as_ref().is_none_or(|ty| ty.contains(&t.kind.as_str())))
            .filter(|t| pages.as_ref().is_none_or(|p| t.page.is_some_and(|page| p.contains(&page))))
            .filter(|t| max_depth.is_none_or(|d| t.depth <= d))
            .collect();
        let count = matching.len();
        let next = offset.checked_add(limit).filter(|n| *n < count);
        let tags: Vec<Value> = matching
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|t| {
                json!({
                    "object": t.object.num, "generation": t.object.generation, "type": t.kind,
                    "parent": t.parent.map(|p| json!({ "object": p.num, "generation": p.generation })), "depth": t.depth,
                    "title": t.title, "actual_text": t.actual_text, "alternate_text": t.alt,
                    "language": t.language, "page": t.page.map(|p| p + 1),
                })
            })
            .collect();
        Ok(json!({ "count": count, "tags": tags, "next": next }))
    }

    pub(crate) fn accessibility_tag_content(&self, a: &Args) -> Result<Value> {
        let object = u32::try_from(a.int("object")?).map_err(|_| ToolError::InvalidArgs("object must be a positive tag object number".into()))?;
        let generation =
            u16::try_from(a.int("generation")?).map_err(|_| ToolError::InvalidArgs("generation must be a PDF object generation".into()))?;
        let content = self.doc(a)?.tag_content(pdfcraft_cos::ObjRef { num: object, generation }).map_err(failed)?;
        Ok(
            json!({ "count": content.len(), "content": content.into_iter().map(|item| json!({ "page": item.page.map(|p| p + 1), "mcid": item.mcid, "text": item.text })).collect::<Vec<_>>() }),
        )
    }

    pub(crate) fn accessibility_set_tag_property(&mut self, a: &Args) -> Result<Value> {
        let object = u32::try_from(a.int("object")?).map_err(|_| ToolError::InvalidArgs("object must be a positive tag object number".into()))?;
        if object == 0 {
            return Err(ToolError::InvalidArgs("object must be a positive tag object number".into()));
        }
        let generation = u16::try_from(a.int("generation")?)
            .map_err(|_| ToolError::InvalidArgs("generation must be a PDF object generation from accessibility_tags".into()))?;
        let property_id = a.str("property")?;
        let property = pdfcraft_engine::a11y::TagProperty::from_id(property_id)
            .ok_or_else(|| ToolError::InvalidArgs(format!("unknown tag property {property_id:?}")))?;
        let value = a.opt_str("value")?.map(str::to_owned);
        let id = self.doc(a)?.id;
        self.session
            .apply(id, pdfcraft_engine::Edit::SetTagProperty { object: pdfcraft_cos::ObjRef::new(object, generation), property, value })
            .map_err(failed)?;
        self.accessibility_tags(a)
    }

    pub(crate) fn accessibility_mark_tag_artifact(&mut self, a: &Args) -> Result<Value> {
        let object = u32::try_from(a.int("object")?).map_err(|_| ToolError::InvalidArgs("object must be a positive tag object number".into()))?;
        if object == 0 {
            return Err(ToolError::InvalidArgs("object must be a positive tag object number".into()));
        }
        let generation = u16::try_from(a.int("generation")?)
            .map_err(|_| ToolError::InvalidArgs("generation must be a PDF object generation from accessibility_tags".into()))?;
        let id = self.doc(a)?.id;
        self.session.apply(id, pdfcraft_engine::Edit::MarkTagArtifact { object: pdfcraft_cos::ObjRef::new(object, generation) }).map_err(failed)?;
        self.accessibility_tags(a)
    }

    pub(crate) fn accessibility_move_tag(&mut self, a: &Args) -> Result<Value> {
        let reference = |number: &str, generation: &str| -> Result<pdfcraft_cos::ObjRef> {
            let n = u32::try_from(a.int(number)?).map_err(|_| ToolError::InvalidArgs(format!("{number} must be a positive tag object number")))?;
            if n == 0 {
                return Err(ToolError::InvalidArgs(format!("{number} must be a positive tag object number")));
            }
            let g = u16::try_from(a.int(generation)?)
                .map_err(|_| ToolError::InvalidArgs(format!("{generation} must be a PDF object generation from accessibility_tags")))?;
            Ok(pdfcraft_cos::ObjRef::new(n, g))
        };
        let object = reference("object", "generation")?;
        let anchor = reference("anchor", "anchor_generation")?;
        let placement = match a.str("placement")? {
            "before" => pdfcraft_engine::a11y::TagPlacement::Before,
            "after" => pdfcraft_engine::a11y::TagPlacement::After,
            other => return Err(ToolError::InvalidArgs(format!("placement must be before or after, not {other:?}"))),
        };
        let id = self.doc(a)?.id;
        self.session.apply(id, pdfcraft_engine::Edit::MoveTag { object, anchor, placement }).map_err(failed)?;
        self.accessibility_tags(a)
    }
}

impl Automation {
    pub(crate) fn ocr_settings(&self, a: &Args) -> Result<pdfcraft_engine::ocr::OcrSettings> {
        let mut settings = pdfcraft_engine::ocr::OcrSettings::default();
        if let Some(d) = a.opt_num("dpi")? {
            settings.dpi = d.clamp(72.0, 600.0) as f32;
        }
        if let Some(l) = a.opt_str("language")? {
            if !pdfcraft_engine::ocr::LANGUAGES.iter().any(|x| x.0 == l) {
                return Err(ToolError::InvalidArgs(format!("unsupported language {l:?}")));
            }
            settings.language = l.into();
        }
        if let Some(s) = a.opt_bool("skip_text_pages")? {
            settings.skip_text_pages = s;
        }
        Ok(settings)
    }

    pub(crate) fn ocr_recognize_files(&mut self, a: &Args) -> Result<Value> {
        let settings = self.ocr_settings(a)?;
        let folder = self.resolve(a.str("folder")?, true)?;
        // A path that leaves the root refuses the whole call before any work starts, like the
        // other tools (the write check passes missing files, which stay per-file errors below).
        let paths = a.strs("paths")?;
        for p in &paths {
            self.resolve_for(p, crate::file_access::Access::Read, true)?;
        }
        std::fs::create_dir_all(&folder).map_err(|e| failed(e.to_string()))?;
        let ocr = pdfcraft_engine::ocr::engine().map_err(failed)?;
        let mut out = Vec::new();
        for p in paths {
            let name = std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "document.pdf".into());
            let result =
                self.resolve(p, false).map_err(|e| e.to_string()).and_then(|src| std::fs::read(&src).map_err(|e| e.to_string())).and_then(|bytes| {
                    pdfcraft_engine::ocr::recognize_file(&name, std::sync::Arc::new(bytes), None, settings.clone(), &ocr, |_, _| true)
                });
            out.push(match result {
                Ok(r) => {
                    let target = child(&folder, &name);
                    write_atomic(&target, &r.bytes)?;
                    let skipped: Vec<usize> = r.pages.iter().filter(|p| p.skipped.is_some()).map(|p| p.page + 1).collect();
                    json!({ "path": p, "output": target.to_string_lossy(), "words": r.words(), "skipped_pages": skipped })
                }
                Err(e) => json!({ "path": p, "error": e }),
            });
        }
        Ok(json!({ "files": out }))
    }

    pub(crate) fn ocr_recognize(&mut self, a: &Args) -> Result<Value> {
        let settings = self.ocr_settings(a)?;
        let pages = if a.opt_ints("pages")?.is_some() { self.pages(a, "pages")? } else { Vec::new() };
        let id = self.doc(a)?.id;
        let found = self.session.recognize_text(id, &pages, settings).map_err(failed)?;
        let list: Vec<Value> = found
            .iter()
            .map(|p| match &p.skipped {
                Some(why) => json!({ "page": p.page + 1, "skipped": why }),
                None => json!({ "page": p.page + 1, "words": p.words.len(), "already_on_page": p.already_on_page, "text": p.text() }),
            })
            .collect();
        Ok(json!({ "words": found.iter().map(|p| p.words.len()).sum::<usize>(), "pages": list }))
    }

    pub(crate) fn ocr_status(&self) -> Result<Value> {
        use pdfcraft_engine::ocr;
        let dirs: Vec<String> = ocr::Models::search_dirs().iter().map(|d| d.to_string_lossy().into_owned()).collect();
        let langs: Vec<Value> = ocr::LANGUAGES.iter().map(|(c, n)| json!({ "code": c, "name": n })).collect();
        Ok(json!({ "available": ocr::available(), "search_dirs": dirs, "languages": langs }))
    }
}

fn request_json(r: &pdfcraft_engine::js::Request) -> Value {
    use pdfcraft_engine::js::Request as R;
    match r {
        R::Reset(n) => json!({ "reset": n }),
        R::Print => json!({ "print": true }),
        R::GoToPage(p) => json!({ "page": p + 1 }),
        R::LaunchUrl(u) => json!({ "url": u }),
        R::Submit(u) => json!({ "submit": u }),
        R::Focus(f) => json!({ "focus": f }),
        R::Beep => json!({ "beep": true }),
        R::SaveAs => json!({ "save_as": true }),
    }
}

impl Automation {
    pub(crate) fn js_run(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        let o = self.session.run_javascript(id, a.str("script")?, a.opt_str("field")?).map_err(failed)?;
        let reqs: Vec<Value> = o.requests.iter().map(request_json).collect();
        Ok(json!({ "alerts": o.alerts, "console": o.console, "requests": reqs, "error": o.error, "result": o.result }))
    }

    pub(crate) fn js_document_scripts(&self, a: &Args) -> Result<Value> {
        let list: Vec<Value> = self.doc(a)?.document_scripts().into_iter().map(|(n, s)| json!({ "name": n, "script": s })).collect();
        Ok(json!({ "scripts": list }))
    }

    pub(crate) fn js_set_document_script(&mut self, a: &Args) -> Result<Value> {
        let edit = pdfcraft_engine::Edit::SetDocumentScript { name: a.str("name")?.into(), script: a.opt_str("script")?.map(str::to_string) };
        let id = self.doc(a)?.id;
        self.session.apply(id, edit).map_err(failed)?;
        self.js_document_scripts(a)
    }

    pub(crate) fn form_set_script(&mut self, a: &Args) -> Result<Value> {
        let edit = pdfcraft_engine::Edit::SetFieldScript {
            name: a.str("field")?.into(),
            event: a.str("event")?.into(),
            script: a.opt_str("script")?.map(str::to_string),
        };
        let id = self.doc(a)?.id;
        self.session.apply(id, edit).map_err(failed)?;
        let out = self.session.take_js_output(id);
        Ok(json!({ "field": a.str("field")?, "event": a.str("event")?, "console": out.console, "errors": out.errors }))
    }

    pub(crate) fn form_merge_data(&mut self, a: &Args) -> Result<Value> {
        let mut files = Vec::new();
        for p in a.strs("paths")? {
            let path = self.resolve(p, false)?;
            files.push((p.to_string(), std::fs::read(&path).map_err(|e| failed(format!("{p}: {e}")))?));
        }
        let csv = pdfcraft_engine::merge_data_files(&files).map_err(failed)?;
        let target = self.resolve(a.str("path")?, true)?;
        write_atomic(&target, csv.as_bytes())?;
        let columns = csv.lines().next().map_or(0, |h| h.split(',').count());
        Ok(json!({ "path": target.to_string_lossy(), "rows": files.len(), "columns": columns }))
    }

    pub(crate) fn doc_export_office(&self, a: &Args) -> Result<Value> {
        let path = self.resolve(a.str("path")?, true)?;
        let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let format = pdfcraft_engine::compare::OfficeFormat::from_extension(&ext)
            .ok_or_else(|| ToolError::InvalidArgs(format!("unsupported extension {ext:?} (docx, xlsx, pptx, html or rtf)")))?;
        // Excel: one sheet for the whole document (the default) or a sheet per page.
        let sheets = match (a.opt_str("sheets")?, format) {
            (None, _) => pdfcraft_engine::compare::XlsxSheets::default(),
            (Some("document"), pdfcraft_engine::compare::OfficeFormat::Xlsx) => pdfcraft_engine::compare::XlsxSheets::WholeDocument,
            (Some("page"), pdfcraft_engine::compare::OfficeFormat::Xlsx) => pdfcraft_engine::compare::XlsxSheets::PerPage,
            (Some(s), pdfcraft_engine::compare::OfficeFormat::Xlsx) => {
                return Err(ToolError::InvalidArgs(format!("unknown sheets {s:?} (document or page)")));
            }
            (Some(_), _) => return Err(ToolError::InvalidArgs("sheets applies to .xlsx only".into())),
        };
        let doc = self.doc_to_copy(a)?;
        let bytes = match format {
            pdfcraft_engine::compare::OfficeFormat::Xlsx => doc.export_xlsx(sheets, None),
            _ => doc.export_office(format),
        };
        write_atomic(&path, &bytes)?;
        Ok(json!({ "path": path.to_string_lossy(), "bytes": bytes.len(), "format": format.extension() }))
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
                .and_then(|src| std::fs::read(&src).map_err(|e| e.to_string()))
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

    pub(crate) fn form_detect_fields(&mut self, a: &Args) -> Result<Value> {
        let pages = if a.opt_ints("pages")?.is_some() { self.pages(a, "pages")? } else { Vec::new() };
        let id = self.doc(a)?.id;
        let found = self.session.detect_fields(id, &pages);
        let list: Vec<Value> = found
            .iter()
            .map(|(p, c)| {
                let kind = match c.kind {
                    pdfcraft_engine::detect::Kind::Text => "text",
                    pdfcraft_engine::detect::Kind::CheckBox => "checkbox",
                };
                json!({ "page": p + 1, "kind": kind, "name": c.name, "rect": c.rect.map(|v| (v * 100.0).round() / 100.0) })
            })
            .collect();
        if a.opt_bool("add")?.unwrap_or(true) && !found.is_empty() {
            self.session.auto_detect_fields(id, &pages).map_err(failed)?;
        }
        Ok(json!({ "fields": list }))
    }

    pub(crate) fn form_actions(&self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::FieldAction as A;
        let list: Vec<Value> = self
            .doc(a)?
            .field_actions(a.str("field")?)
            .iter()
            .map(|(t, act)| {
                let mut v = match act {
                    A::JavaScript(j) => json!({ "javascript": j }),
                    A::Uri(u) => json!({ "url": u }),
                    A::Reset(f) => json!({ "reset": f }),
                    A::Named(n) => json!({ "menu": n }),
                    A::GoTo(p) => json!({ "page": p + 1 }),
                    A::ShowHide { fields, hide: true } => json!({ "hide": fields }),
                    A::ShowHide { fields, hide: false } => json!({ "show": fields }),
                    A::Submit(u) => json!({ "submit": u }),
                    A::Other(s) => json!({ "other": s }),
                };
                v["trigger"] = json!(t.id());
                v
            })
            .collect();
        Ok(json!({ "field": a.str("field")?, "actions": list }))
    }

    pub(crate) fn form_set_actions(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::{FieldAction as A, FieldTrigger as T};
        let items = a.get("actions").and_then(Value::as_array).ok_or_else(|| ToolError::InvalidArgs("actions must be an array".into()))?;
        let bad = |m: String| ToolError::InvalidArgs(m);
        let names = |v: &Value| -> Vec<String> {
            v.as_array().map(|x| x.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        let mut acts = Vec::new();
        for it in items {
            let tid = it["trigger"].as_str().unwrap_or("");
            let t = T::from_id(tid).ok_or_else(|| bad(format!("unknown trigger {tid:?}")))?;
            let act = if let Some(j) = it["javascript"].as_str() {
                A::JavaScript(j.into())
            } else if let Some(u) = it["url"].as_str() {
                A::Uri(u.into())
            } else if it.get("reset").is_some() {
                A::Reset(names(&it["reset"]))
            } else if let Some(m) = it["menu"].as_str() {
                A::Named(m.into())
            } else if let Some(p) = it["page"].as_u64() {
                A::GoTo((p.max(1) - 1) as usize)
            } else if it.get("show").is_some() {
                A::ShowHide { fields: names(&it["show"]), hide: false }
            } else if it.get("hide").is_some() {
                A::ShowHide { fields: names(&it["hide"]), hide: true }
            } else if let Some(u) = it["submit"].as_str() {
                A::Submit(u.into())
            } else {
                return Err(bad(format!("{tid}: give one action (javascript, url, reset, menu, page, show, hide or submit)")));
            };
            acts.push((t, act));
        }
        let name = a.str("field")?.to_string();
        let edit =
            pdfcraft_engine::Edit::SetFieldProps { name, props: Box::new(pdfcraft_engine::FieldProps { actions: Some(acts), ..Default::default() }) };
        let id = self.doc(a)?.id;
        self.session.apply(id, edit).map_err(failed)?;
        self.form_actions(a)
    }

    pub(crate) fn js_enabled(&mut self, a: &Args) -> Result<Value> {
        if let Some(on) = a.opt_bool("enabled")? {
            self.session.set_javascript(on);
        }
        Ok(json!({ "enabled": self.session.javascript() }))
    }
}

#[cfg(test)]
mod review_html_tests {
    use super::*;

    #[test]
    fn document_names_are_escaped_in_review_html() {
        let record = json!({
            "date": "2026-10-12",
            "before": { "name": "<script>alert('x')</script>.pdf", "sha256": "a", "summary": { "passed": 0, "failed": 1, "manual": 0, "skipped": 0 } },
            "after": { "name": "safe.pdf", "sha256": "b", "summary": { "passed": 1, "failed": 0, "manual": 0, "skipped": 0 } },
            "rules": [{ "rule": "primary-language", "before": { "status": "failed", "findings": 1 }, "after": { "status": "passed", "findings": 0 } }],
            "review_required": [],
            "note": "Review independently.",
        });
        let html = review_html(&record).unwrap();
        assert!(html.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;.pdf"));
        assert!(!html.contains("<script>"));
    }
}
