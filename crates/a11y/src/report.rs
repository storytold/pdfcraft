//! The accessibility report: a self-contained HTML page with the summary and, per category,
//! each rule's status, description and problems.

use std::fmt::Write as _;

use crate::{Category, Report, Status};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The report for `file_name`, dated `date` (as the caller formats it).
#[must_use]
pub fn report_html(report: &Report, file_name: &str, date: &str) -> String {
    let mut h = String::new();
    h.push_str("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><title>Accessibility Report</title>\n<style>\n");
    h.push_str(
        "body{font:14px/1.45 system-ui,sans-serif;margin:24px;color:#1d1d1f}h1{font-size:22px}h2{font-size:17px;margin-top:28px}\
         table{border-collapse:collapse;width:100%}th,td{border:1px solid #ccc;padding:6px 8px;text-align:left;vertical-align:top}\
         th{background:#f2f2f2}.Failed{color:#b3261e;font-weight:600}.Passed{color:#1e7b34}ul{margin:4px 0 0;padding-left:18px}\n",
    );
    h.push_str("</style></head><body>\n<h1>Accessibility Report</h1>\n");
    let _ = write!(h, "<p>Filename: <b>{}</b><br>Report created: {}</p>\n", esc(file_name), esc(date));
    h.push_str("<h2>Summary</h2>\n<p>");
    h.push_str(if report.count(Status::Failed) > 0 {
        "The checker found problems which may prevent the document from being fully accessible."
    } else {
        "The checker found no problems in this document."
    });
    h.push_str("</p>\n<ul>\n");
    for s in [Status::Manual, Status::Skipped, Status::Passed, Status::Failed] {
        let _ = write!(h, "<li>{}: {}</li>\n", s.label(), report.count(s));
    }
    h.push_str("</ul>\n<h2>Detailed Report</h2>\n");
    for c in Category::ALL {
        let _ = write!(h, "<h3>{}</h3>\n<table><tr><th>Rule Name</th><th>Status</th><th>Description</th></tr>\n", c.label());
        for r in report.in_category(c) {
            let _ = write!(
                h,
                "<tr><td>{}</td><td class=\"{:?}\">{}</td><td>{}",
                esc(r.rule.name()),
                r.status,
                r.status.label(),
                esc(r.rule.description())
            );
            if !r.findings.is_empty() {
                h.push_str("<ul>");
                for f in &r.findings {
                    let _ = write!(h, "<li>{}</li>", esc(&f.message));
                }
                h.push_str("</ul>");
            }
            h.push_str("</td></tr>\n");
        }
        h.push_str("</table>\n");
    }
    h.push_str("</body></html>\n");
    h
}
