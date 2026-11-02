/// Why a tool call failed.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolError {
    /// No tool has this name.
    UnknownTool(String),
    /// The arguments don't match the tool's schema.
    InvalidArgs(String),
    /// The tool ran and failed (the message is for the agent to read).
    Failed(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::UnknownTool(t) => write!(f, "unknown tool {t:?}"),
            ToolError::InvalidArgs(m) | ToolError::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ToolError {}

pub(crate) type Result<T> = std::result::Result<T, ToolError>;

pub(crate) fn failed(e: impl std::fmt::Display) -> ToolError {
    ToolError::Failed(e.to_string())
}

impl From<pdfcraft_engine::EditError> for ToolError {
    fn from(e: pdfcraft_engine::EditError) -> Self {
        failed(e)
    }
}
