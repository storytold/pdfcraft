//! `doc_unembed_fonts`: take the programs out of fonts a reader can substitute, or report what
//! that would do.

use pdfcraft_engine::Edit;
use pdfcraft_engine::optimize::unembed::{Outcome, Plan, Request};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError};

fn fonts(plan: &Plan) -> Vec<Value> {
    plan.fonts
        .iter()
        .map(|f| {
            let (outcome, reason) = match f.outcome {
                Outcome::Unembed => ("unembed", None),
                Outcome::NotEmbedded => ("not-embedded", None),
                Outcome::NotSelected => ("not-selected", None),
                Outcome::Refused(r) => (r.token(), Some(r.reason())),
            };
            json!({
                "object": f.font.map(|r| r.num),
                "name": f.name,
                "subtype": f.subtype,
                "outcome": outcome,
                "reason": reason,
                "renamed_to": f.renamed_to,
                "program_bytes": f.program_bytes,
                "freed": f.freed,
            })
        })
        .collect()
}

impl Automation {
    pub(crate) fn unembed_fonts(&mut self, a: &Args) -> Result<Value> {
        let names = match a.get("fonts") {
            Some(_) => Some(a.strs("fonts")?.into_iter().map(str::to_string).collect::<Vec<_>>()),
            None => None,
        };
        if names.as_ref().is_some_and(Vec::is_empty) {
            return Err(ToolError::InvalidArgs("fonts must name at least one font; leave it out for every font".into()));
        }
        let keep_subset_tag = a.opt_bool("keep_subset_tag")?.unwrap_or(false);
        let dry_run = a.opt_bool("dry_run")?.unwrap_or(false);
        let doc = self.doc(a)?;
        let plan = doc.unembed_fonts_plan(&Request { fonts: names.clone(), keep_subset_tag });
        // PDF/A requires every font embedded, and so does PDF/X, which always has an output intent.
        let declared = doc.standards();
        let pdfa = declared.pdfa.map(|(part, level)| format!("PDF/A-{part}{}", level.to_lowercase()));
        let loses: Vec<String> = pdfa.into_iter().chain(declared.output_intents.iter().map(|i| format!("output intent {i}"))).collect();
        let mut out = json!({
            "dry_run": dry_run,
            "unembedded": plan.unembedded(),
            "bytes_freed": plan.bytes_freed,
            "unmatched": plan.unmatched,
            "fonts": fonts(&plan),
            "loses_conformance": loses,
        });
        if !dry_run {
            let r = self.apply(a, Edit::UnembedFonts { fonts: names, keep_subset_tag })?;
            out["undo"] = r["undo"].clone();
        }
        Ok(out)
    }
}
