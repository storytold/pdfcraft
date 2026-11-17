//! `doc_create_portfolio`: Create a PDF ▸ Portfolio.

use pdfcraft_engine::{ExhibitStyle, PortfolioFile, PortfolioLayout, PortfolioOptions};

use super::error::{Result, ToolError, failed};
use super::json::Args;
use super::paths::read_input;
use super::{Arc, Automation, Value};

impl Automation {
    /// Bundle `paths`, unchanged and in order, into one PDF portfolio with a cover sheet that
    /// indexes them; `descriptions` and `dates` go with `paths`, item for item.
    pub(crate) fn doc_create_portfolio(&mut self, a: &Args) -> Result<Value> {
        let paths = a.strs("paths")?;
        if paths.is_empty() {
            return Err(ToolError::InvalidArgs("paths must list at least one file".into()));
        }
        let descriptions = per_file(a, "descriptions", paths.len())?;
        let dates = per_file(a, "dates", paths.len())?;
        let exhibits = match a.opt_str("exhibits")? {
            None => ExhibitStyle::Letters,
            Some(s) => ExhibitStyle::parse(s)
                .ok_or_else(|| ToolError::InvalidArgs(format!("exhibits must be \"letters\", \"numbers\" or \"none\", not {s:?}")))?,
        };
        let layout = match a.opt_str("view")? {
            None => PortfolioLayout::Details,
            Some(s) => PortfolioLayout::parse(s)
                .ok_or_else(|| ToolError::InvalidArgs(format!("view must be \"details\", \"tile\" or \"hidden\", not {s:?}")))?,
        };
        let initial = match a.opt_int("initial")? {
            None => None,
            Some(n) if n >= 1 && (n as u64) <= paths.len() as u64 => Some((n - 1) as usize),
            Some(n) => return Err(ToolError::InvalidArgs(format!("initial must be between 1 and {}, not {n}", paths.len()))),
        };
        let options = PortfolioOptions {
            title: a.opt_str("title")?.unwrap_or("PDF Portfolio").to_string(),
            note: a.opt_str("note")?.map(str::to_owned),
            exhibits,
            prefix: a.opt_str("prefix")?.unwrap_or("Exhibit").to_string(),
            layout,
            initial,
        };
        let mut files = Vec::with_capacity(paths.len());
        for ((p, description), date) in paths.into_iter().zip(descriptions).zip(dates) {
            let path = self.resolve(p, false)?;
            let bytes = read_input(&path)?;
            let modified = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|d| i64::try_from(d.as_secs()).ok());
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            files.push(PortfolioFile { name, bytes: Arc::new(bytes), description, document_date: date, modified });
        }
        let bytes = self.session.create_portfolio(&files, &options).map_err(failed)?;
        let name = format!("{}.pdf", options.title.trim());
        self.deliver(a, &name, bytes)
    }
}

/// An optional array of strings (or nulls), one per file.
fn per_file(a: &Args, key: &str, len: usize) -> Result<Vec<Option<String>>> {
    match a.get(key) {
        None | Some(Value::Null) => Ok(vec![None; len]),
        Some(Value::Array(v)) if v.len() == len && v.iter().all(|x| x.is_string() || x.is_null()) => {
            Ok(v.iter().map(|x| x.as_str().map(str::to_owned)).collect())
        }
        Some(_) => Err(ToolError::InvalidArgs(format!("{key} must list a string (or null) for each path"))),
    }
}
