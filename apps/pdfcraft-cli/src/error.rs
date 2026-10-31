//! Every way the command line can fail, as types (RFC-0012 §2.2).
//!
//! Three kinds, three exit codes: a value clap's parser rejects is a [`ValueError`] inside a clap
//! error, a combination of flags it cannot express is a [`UsageError`], and both exit 2; a command
//! that ran and failed is a [`CliError`] and exits 1.

use std::io;
use std::path::PathBuf;

use clap::error::ErrorKind;

/// Why a command that was parsed correctly failed. Printed as `error: <message>`, exit 1.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// A file or folder could not be read or written.
    #[error("{}: {source}", path.display())]
    File { path: PathBuf, source: io::Error },
    /// Writing a result to stdout failed. A closed pipe is not a failure ([`CliError::is_closed_pipe`]).
    #[error("stdout: {0}")]
    Stdout(io::Error),
    /// A document could not be opened, read or inspected.
    #[error("{}: {source}", path.display())]
    Document { path: PathBuf, source: Box<dyn std::error::Error + Send + Sync> },
    /// The engine refused an open, an edit or a save.
    #[error("{0}")]
    Engine(String),
    /// The renderer or an image encoder failed.
    #[error("{0}")]
    Render(String),
    #[error(transparent)]
    Tool(#[from] pdfcraft_automation::ToolError),
    #[error("{command}: {message}")]
    Command { command: &'static str, message: String },
    #[error("text: the document's permissions don't allow copying its text; pass the owner password with --password to extract it")]
    CopyingNotAllowed,
    #[error("text: the document could not be parsed")]
    NotParsed,
    #[error("text: page(s) {} could not be read", list(.0))]
    PagesUnreadable(Vec<usize>),
    #[error("{option}: page {page} does not exist; after the earlier options the document has {count} pages")]
    PageMissing { option: &'static str, page: usize, count: usize },
    #[error("{option} {at} is past the end: it must be between 1 and {max} (the document has {count} pages)")]
    PositionPastEnd { option: &'static str, at: usize, max: usize, count: usize },
    #[error("check: no PDF files found")]
    NoPdfFiles,
    #[error("audit: {0} faux redaction(s) found")]
    FauxRedactions(usize),
    #[error("batch: one or more files failed; see the JSON report")]
    BatchFailed,
    #[error("run: {path}: {source}", path = path.display())]
    Script { path: PathBuf, source: serde_json::Error },
    #[error("step {step}: missing \"tool\"")]
    StepWithoutTool { step: usize },
    #[error("step {step} ({tool}): {source}")]
    Step { step: usize, tool: String, source: Box<CliError> },
    #[error("--root {}: {source}", path.display())]
    Root { path: PathBuf, source: io::Error },
    #[error("mcp: {0}")]
    Roots(String),
    #[error("mcp: this build leaves out the MCP server (built without the `mcp` feature)")]
    McpLeftOut,
    #[error("ui: {0}")]
    Ui(String),
}

fn list(pages: &[usize]) -> String {
    pages.iter().map(usize::to_string).collect::<Vec<_>>().join(", ")
}

impl CliError {
    /// `head | …` closed our stdout: the reader has what it wanted.
    pub(crate) fn is_closed_pipe(&self) -> bool {
        matches!(self, Self::Stdout(e) if e.kind() == io::ErrorKind::BrokenPipe)
    }

    pub(crate) fn file(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::File { path: path.into(), source }
    }

    pub(crate) fn command(command: &'static str, message: impl Into<String>) -> Self {
        Self::Command { command, message: message.into() }
    }
}

/// A value a flag was given that cannot be used. clap prints it after
/// `invalid value '<input>' for '<flag>':`.
#[derive(Debug, thiserror::Error)]
pub enum ValueError {
    #[error("expected a positive page number")]
    PageNumber,
    #[error("bad page number {0:?}")]
    PageItem(String),
    #[error("bad page range {0:?}: it ends before it starts")]
    RangeBackwards(String),
    #[error("bad page range {0:?}: more than {1} pages")]
    RangeTooLong(String, usize),
    #[error("expected PAGES:DEGREES, such as 1,3:90")]
    RotateShape,
    #[error("expected a whole number of degrees, got {0:?}")]
    RotateNumber(String),
    #[error("expected a multiple of 90 (such as 90, -90 or 180), got {0}")]
    RotateMultiple(i64),
    #[error("expected PAGES:TO, such as 5:1")]
    MoveShape,
    #[error("expected a page number of at least 1, got {0:?}")]
    PageAtLeastOne(String),
    #[error("expected a positive number of dots per inch")]
    Dpi,
    #[error("expected a number from {min} to {max}")]
    Between { min: f64, max: f64 },
    #[error("expected key=value")]
    KeyValue,
    #[error("use a .png, .jpg, .tif or .pam name")]
    ImageName,
    #[error("a path cannot be empty")]
    EmptyPath,
    #[error("the path is not valid UTF-8, and the automation tools name files as text")]
    NotUtf8,
}

/// A combination of flags that is wrong although each is fine alone. Exit 2, like clap's own
/// conflicts; [`UsageError::kind`] picks the clap error kind it is reported as.
#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    #[error("`{option}` only applies to the `{step}` step, which --steps leaves out")]
    OptionNeedsStep { option: &'static str, step: &'static str },
    #[error("--report needs --script")]
    ReportNeedsScript,
}

impl UsageError {
    pub(crate) fn kind(&self) -> ErrorKind {
        match self {
            Self::OptionNeedsStep { .. } => ErrorKind::ArgumentConflict,
            Self::ReportNeedsScript => ErrorKind::MissingRequiredArgument,
        }
    }

    pub(crate) fn into_clap(self, cmd: &mut clap::Command) -> clap::Error {
        let kind = self.kind();
        cmd.error(kind, self)
    }
}
