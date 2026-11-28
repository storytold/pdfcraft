//! Print production ▸ Convert spots to process as a tool (`doc_convert_spots`).

use pdfcraft_engine::{Edit, SpotsConverted};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError};

/// The counts and disclosures of a conversion, as the tool returns them.
fn report(done: &SpotsConverted) -> Value {
    let refused: Vec<Value> =
        done.refused.iter().map(|r| json!({ "ink": r.ink, "object": r.object.map(|o| o.to_string()), "reason": r.reason })).collect();
    json!({
        "converted": done.converted(),
        "spaces": done.spaces,
        "paints": done.paints,
        "images": done.images,
        "lookups": done.lookups,
        "shadings": done.shadings,
        "objects": done.objects,
        "refused": refused,
        "overprint_changed": done.overprint_changed.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "notes": done.notes,
    })
}

impl Automation {
    /// The spot inks named in `inks` (every spot ink when it is absent or empty) become the CMYK
    /// their tint transforms give. Nothing to convert is not an error: `converted` is 0, the
    /// refusals say why, and no edit is made.
    pub(crate) fn convert_spots(&mut self, a: &Args) -> Result<Value> {
        let inks: Vec<String> = match a.get("inks") {
            None => Vec::new(),
            Some(v) => {
                let wrong = || ToolError::InvalidArgs("inks must be an array of ink names".into());
                let arr = v.as_array().ok_or_else(wrong)?;
                arr.iter().map(|x| x.as_str().map(str::to_owned).ok_or_else(wrong)).collect::<Result<_>>()?
            }
        };
        let id = self.doc(a)?.id;
        let found = self.session.count_spots_to_process(id, &inks).map_err(ToolError::InvalidArgs)?;
        let mut out = report(&found);
        if found.converted() > 0 {
            let r = self.apply(a, Edit::ConvertSpotsToProcess { inks })?;
            out["undo"] = r["undo"].clone();
        }
        Ok(out)
    }
}
