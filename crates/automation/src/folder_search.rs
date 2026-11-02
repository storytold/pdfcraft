//! `folder_search`: the engine's folder search as a tool.

use std::sync::atomic::AtomicBool;

use pdfcraft_engine::folder_search::{self, Event, Options, Skip, Stop};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, failed};

/// The most matches one call returns.
const MAX_LIMIT: usize = 10_000;

fn detail(reason: &Skip) -> Option<String> {
    match reason {
        Skip::Unreadable(detail) | Skip::Damaged(detail) => Some(detail.clone()),
        Skip::PagesUnread(pages) => Some(format!("{pages} pages could not be read")),
        _ => None,
    }
}

impl Automation {
    pub(crate) fn folder_search(&self, args: &Args) -> Result<Value> {
        let mut options = Options::new(self.resolve(args.str("path")?, false)?, args.str("query")?);
        options.subfolders = args.opt_bool("subfolders")?.unwrap_or(true);
        options.case_sensitive = args.opt_bool("case_sensitive")?.unwrap_or(false);
        options.whole_words = args.opt_bool("whole_words")?.unwrap_or(false);
        if let Some(limit) = args.opt_int("limit")? {
            options.limit = usize::try_from(limit).ok().filter(|n| (1..=MAX_LIMIT).contains(n)).ok_or_else(|| Args::wrong("limit", "1 to 10000"))?;
        }
        let (mut files, mut skipped) = (Vec::new(), Vec::new());
        let summary = folder_search::search(&options, &AtomicBool::new(false), |event| match event {
            Event::Matches(file) => {
                let matches: Vec<Value> = file
                    .matches
                    .iter()
                    .map(|m| json!({ "page": m.page.saturating_add(1), "before": m.before, "text": m.text, "after": m.after }))
                    .collect();
                files.push(json!({ "path": file.path.to_string_lossy(), "matches": matches }));
            }
            Event::Skipped(file) => {
                skipped.push(json!({ "path": file.path.to_string_lossy(), "reason": file.reason.name(), "detail": detail(&file.reason) }));
            }
            Event::Found(_) | Event::Reading { .. } => {}
        })
        .map_err(failed)?;
        let stopped = summary.stopped.map(|stop| match stop {
            Stop::Cancelled => "cancelled",
            Stop::MatchLimit => "match_limit",
            Stop::TooManySlowFiles => "too_many_slow_files",
        });
        Ok(json!({
            "found": summary.found,
            "searched": summary.searched,
            "files_with_matches": summary.files_with_matches,
            "matches": summary.matches,
            "folder_cut_short": summary.folder_cut_short,
            "stopped": stopped,
            "files": files,
            "skipped": skipped,
        }))
    }
}
