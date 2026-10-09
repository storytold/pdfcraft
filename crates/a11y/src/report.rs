//! The accessibility report: a self-contained HTML page with the summary and, per category,
//! each rule's status, description and problems.
//!
//! The page is written in whichever language `words` translates into, down to its `lang` attribute,
//! so it reads the way the interface that saved it reads. Only the file name, the date and what the
//! findings say about the document itself come from elsewhere.

use crate::{Category, Report, Status, Words};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The page's own words, in the order it says them: its heading, what it calls the file and the
/// date, the two things the summary can say, and the headers of the table.
///
/// The rest of what the page says it gets from elsewhere — the categories, the rules and the
/// findings name themselves — so an interface has these to translate and those to translate; a test
/// in this crate keeps the list in step with the page.
pub const REPORT_WORDS: &[&str] = &[
    "Accessibility Report",
    "Filename",
    "Report created",
    "Summary",
    "The checker found problems which may prevent the document from being fully accessible.",
    "The checker found no problems in this document.",
    "Detailed Report",
    "Rule Name",
    "Status",
    "Description",
];

/// The report for `file_name`, dated `date` (as the caller formats it), written in the language
/// `words` translates into and marked `lang` (`"en"` and `&|s: &str| s.to_owned()` for English).
pub fn report_html(report: &Report, file_name: &str, date: &str, lang: &str, words: Words) -> String {
    let title = esc(&words("Accessibility Report"));
    let mut h = String::new();
    h.push_str(&format!("<!doctype html>\n<html lang=\"{}\"><head><meta charset=\"utf-8\"><title>{title}</title>\n<style>\n", esc(lang)));
    h.push_str(
        "body{font:14px/1.45 system-ui,sans-serif;margin:24px;color:#1d1d1f}h1{font-size:22px}h2{font-size:17px;margin-top:28px}\
         table{border-collapse:collapse;width:100%}th,td{border:1px solid #ccc;padding:6px 8px;text-align:left;vertical-align:top}\
         th{background:#f2f2f2}.Failed{color:#b3261e;font-weight:600}.Passed{color:#1e7b34}ul{margin:4px 0 0;padding-left:18px}\n",
    );
    h.push_str(&format!("</style></head><body>\n<h1>{title}</h1>\n"));
    h.push_str(&format!("<p>{}: <b>{}</b><br>{}: {}</p>\n", esc(&words("Filename")), esc(file_name), esc(&words("Report created")), esc(date)));
    h.push_str(&format!("<h2>{}</h2>\n<p>", esc(&words("Summary"))));
    // Each outcome is a whole sentence, and each is handed over on its own, so the list in
    // `REPORT_WORDS` can be read off this file.
    h.push_str(&if report.count(Status::Failed) > 0 {
        esc(&words("The checker found problems which may prevent the document from being fully accessible."))
    } else {
        esc(&words("The checker found no problems in this document."))
    });
    h.push_str("</p>\n<ul>\n");
    for s in [Status::Manual, Status::Skipped, Status::Passed, Status::Failed] {
        h.push_str(&format!("<li>{}: {}</li>\n", esc(&words(s.label())), report.count(s)));
    }
    h.push_str(&format!("</ul>\n<h2>{}</h2>\n", esc(&words("Detailed Report"))));
    let (name, status, description) = (esc(&words("Rule Name")), esc(&words("Status")), esc(&words("Description")));
    for c in Category::ALL {
        h.push_str(&format!("<h3>{}</h3>\n<table><tr><th>{name}</th><th>{status}</th><th>{description}</th></tr>\n", esc(&words(c.label()))));
        for r in report.in_category(c) {
            h.push_str(&format!(
                "<tr><td>{}</td><td class=\"{:?}\">{}</td><td>{}",
                esc(&words(r.rule.name())),
                r.status,
                esc(&words(r.status.label())),
                esc(&words(r.rule.description()))
            ));
            if !r.findings.is_empty() {
                h.push_str("<ul>");
                for f in &r.findings {
                    h.push_str(&format!("<li>{}</li>", esc(&f.said(words))));
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
