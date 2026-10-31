//! The command line, as clap sees it. Everything the parser can check is checked here, before any
//! file is opened: unknown and repeated flags, missing and conflicting ones, and the type and
//! range of each value (RFC-0012 §1.4, §2.1).
//!
//! A value that starts with `-` is written `--out=-x.png` (or `./-x.png`): a flag never takes the
//! next flag as its value, so `--out --password` is an error and not a file called `--password`
//! (photocraft#6944). Negative numbers are the exception where a number is expected.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use clap::builder::{NonEmptyStringValueParser, TypedValueParser as _};
use clap::{ArgAction, ArgGroup, ArgMatches, Args, Parser, Subcommand, ValueEnum};

use crate::error::ValueError;

/// pdfcraft-cli: headless PdfCraft.
#[derive(Parser, Debug)]
#[command(name = "pdfcraft-cli", arg_required_else_help = true, propagate_version = true, after_help = AFTER_HELP)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Cmd,
}

const AFTER_HELP: &str = "Exit status: 0 success, 1 the command failed, 2 usage error.\n\
A value that starts with `-` is written `--flag=-value` or `./-name`.\n\
help and feedback: https://discord.gg/artcraft";

/// The most pages one `A-B` range may name: a bound on what a typed range can allocate.
pub const MAX_RANGE: usize = 1_000_000;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Document summary as JSON (includes any declared PDF/A or PDF/UA level)
    Info(InfoArgs),
    /// Render one page to an image
    Render(RenderArgs),
    /// Extracted text (pages separated by form feeds)
    Text(TextArgs),
    /// Edit pages and metadata and save; the options run in order, and page numbers refer to the document after the earlier options
    Edit(EditArgs),
    /// Combine PDFs into one
    Combine(CombineArgs),
    /// Copy pages (and ranges, in the order given) into a new PDF
    Extract(ExtractArgs),
    /// Split a PDF into parts
    Split(SplitArgs),
    /// Open, inspect and render every PDF in child processes, reporting hangs and crashes
    Check(CheckArgs),
    /// The robustness check of one file; run by `check` in a child process
    #[command(name = "check-one", hide = true)]
    CheckOne(CheckOneArgs),
    /// Find faux redactions (fails when found)
    Audit(AuditArgs),
    /// Convert, OCR and optimize a folder of files
    Batch(BatchArgs),
    /// The automation tools and their JSON Schemas
    Tools,
    /// Run an automation tool, or a script of them
    Run(RunArgs),
    /// Drive a running app started with `pdfcraft --control FILE`
    Ui(UiArgs),
    /// MCP server on stdin/stdout (opt-in)
    #[cfg(feature = "mcp")]
    Mcp(McpArgs),
    /// Left out of this build (built without the `mcp` feature)
    #[cfg(not(feature = "mcp"))]
    #[command(hide = true)]
    Mcp(LeftOutArgs),
}

/// Shared by every command that opens a document.
#[derive(Args, Debug)]
pub struct PasswordArg {
    /// The document's password (user or owner)
    #[arg(long, value_name = "PW")]
    pub password: Option<String>,
}

#[derive(Args, Debug)]
pub struct InfoArgs {
    pub file: PathBuf,
    #[command(flatten)]
    pub password: PasswordArg,
}

#[derive(Args, Debug)]
pub struct AuditArgs {
    pub file: PathBuf,
    #[command(flatten)]
    pub password: PasswordArg,
}

#[derive(Args, Debug)]
pub struct TextArgs {
    pub file: PathBuf,
    /// Only this page (from 1)
    #[arg(long, value_name = "N", value_parser = page_number)]
    pub page: Option<NonZeroUsize>,
    #[command(flatten)]
    pub password: PasswordArg,
}

#[derive(Args, Debug)]
pub struct RenderArgs {
    pub file: PathBuf,
    /// The page to render (from 1)
    #[arg(long, value_name = "N", default_value = "1", value_parser = page_number)]
    pub page: NonZeroUsize,
    /// Resolution in dots per inch
    #[arg(long, value_name = "DPI", default_value = "96", value_parser = dpi, allow_negative_numbers = true)]
    pub dpi: f32,
    /// The image to write: a .png, .jpg, .tif or .pam name
    #[arg(long, value_name = "FILE", required = true, value_parser = image_out)]
    pub out: ImageOut,
    #[command(flatten)]
    pub password: PasswordArg,
}

#[derive(Args, Debug)]
pub struct EditArgs {
    /// The PDF to edit
    pub file: PathBuf,
    /// Where to save the result
    #[arg(long, value_name = "FILE", required = true, value_parser = path)]
    pub out: PathBuf,
    /// Rotate pages by a multiple of 90 degrees, e.g. 1,3:90; repeatable
    #[arg(long, value_name = "PAGES:DEGREES", action = ArgAction::Append, value_parser = rotate_spec, allow_negative_numbers = true)]
    pub rotate: Vec<Rotate>,
    /// Delete pages, e.g. 2,4 or 5-8; repeatable
    #[arg(long, value_name = "PAGES", action = ArgAction::Append, value_parser = page_list)]
    pub delete: Vec<PageList>,
    /// Move pages to a position, e.g. 5:1; repeatable
    #[arg(long = "move", value_name = "PAGES:TO", action = ArgAction::Append, value_parser = move_spec)]
    pub move_pages: Vec<Move>,
    /// Insert a blank page at a position (from 1); repeatable
    #[arg(long, value_name = "N", action = ArgAction::Append, value_parser = page_number)]
    pub insert_blank: Vec<NonZeroUsize>,
    /// Set the Title; repeatable
    #[arg(long, value_name = "T", action = ArgAction::Append)]
    pub title: Vec<String>,
    /// Set the Author; repeatable
    #[arg(long, value_name = "A", action = ArgAction::Append)]
    pub author: Vec<String>,
    /// Rewrite the whole file instead of saving incrementally
    #[arg(long)]
    pub full: bool,
    #[command(flatten)]
    pub password: PasswordArg,
}

/// One `edit` option, in the place the command line gave it.
#[derive(Debug, PartialEq)]
pub enum EditStep {
    Rotate(Rotate),
    Delete(Vec<usize>),
    Move(Move),
    InsertBlank(usize),
    Title(String),
    Author(String),
}

impl EditArgs {
    /// The edit options in command-line order. clap keeps each option's values apart, so the order
    /// across options comes from the position of every value in the arguments.
    pub fn steps(&self, matches: &ArgMatches) -> Vec<EditStep> {
        let mut steps: Vec<(usize, EditStep)> = Vec::new();
        let mut add = |id: &str, items: Vec<EditStep>| {
            let positions = matches.indices_of(id).into_iter().flatten();
            steps.extend(positions.zip(items));
        };
        add("rotate", self.rotate.iter().cloned().map(EditStep::Rotate).collect());
        add("delete", self.delete.iter().map(|p| EditStep::Delete(p.0.clone())).collect());
        add("move_pages", self.move_pages.iter().cloned().map(EditStep::Move).collect());
        add("insert_blank", self.insert_blank.iter().map(|n| EditStep::InsertBlank(n.get() - 1)).collect());
        add("title", self.title.iter().cloned().map(EditStep::Title).collect());
        add("author", self.author.iter().cloned().map(EditStep::Author).collect());
        steps.sort_by_key(|(position, _)| *position);
        steps.into_iter().map(|(_, step)| step).collect()
    }
}

#[derive(Args, Debug)]
pub struct CombineArgs {
    /// The PDFs to join, in order (at least two)
    #[arg(required = true, num_args = 2..)]
    pub inputs: Vec<PathBuf>,
    /// Where to save the result
    #[arg(long, value_name = "FILE", required = true, value_parser = path)]
    pub out: PathBuf,
}

#[derive(Args, Debug)]
pub struct ExtractArgs {
    pub file: PathBuf,
    /// Pages and ranges, in the order given: 1,3,5-8
    #[arg(long, value_name = "PAGES", required = true, value_parser = page_list)]
    pub pages: PageList,
    /// Where to save the result
    #[arg(long, value_name = "FILE", required = true, value_parser = path)]
    pub out: PathBuf,
    #[command(flatten)]
    pub password: PasswordArg,
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("by").args(["every", "before"]).required(true)))]
pub struct SplitArgs {
    pub file: PathBuf,
    /// A part every N pages
    #[arg(long, value_name = "N", value_parser = page_number)]
    pub every: Option<NonZeroUsize>,
    /// Start a new part before these pages, e.g. 3,7
    #[arg(long, value_name = "PAGES", value_parser = page_list)]
    pub before: Option<PageList>,
    /// Where to write the parts (created when missing)
    #[arg(long, value_name = "DIR", default_value = ".", value_parser = path)]
    pub out_dir: PathBuf,
    #[command(flatten)]
    pub password: PasswordArg,
}

#[derive(Args, Debug)]
pub struct CheckArgs {
    /// PDF files, or folders searched for them
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,
    /// Seconds a file may take before it counts as a hang
    #[arg(long, value_name = "SECS", default_value = "20", value_parser = clap::value_parser!(u64).range(1..))]
    pub timeout: u64,
    /// Resolution the pages are rendered at
    #[arg(long, value_name = "DPI", default_value = "36", value_parser = dpi, allow_negative_numbers = true)]
    pub dpi: f32,
    /// Also write the results here as JSON
    #[arg(long, value_name = "FILE", value_parser = path)]
    pub json: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct CheckOneArgs {
    pub file: PathBuf,
    #[arg(long, value_name = "DPI", default_value = "36", value_parser = dpi, allow_negative_numbers = true)]
    pub dpi: f32,
    /// Also run the object layer end to end (fuzzing)
    #[arg(long)]
    pub edit: bool,
}

/// The steps of `batch`, in the order they run.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Convert,
    Ocr,
    Optimize,
}

impl Step {
    pub fn name(self) -> &'static str {
        match self {
            Self::Convert => "convert",
            Self::Ocr => "ocr",
            Self::Optimize => "optimize",
        }
    }
}

#[derive(Args, Debug)]
pub struct BatchArgs {
    /// Files or folders
    #[arg(required = true, value_parser = clap::builder::OsStringValueParser::new().try_map(utf8_path))]
    pub paths: Vec<PathBuf>,
    /// What to do, comma separated, convert first when present
    #[arg(long, value_name = "STEPS", required = true, value_delimiter = ',', value_enum)]
    pub steps: Vec<Step>,
    /// Where to write the results
    #[arg(long, value_name = "DIR", required = true, value_parser = clap::builder::OsStringValueParser::new().try_map(utf8_path))]
    pub out_dir: PathBuf,
    /// Descend into folders
    #[arg(long)]
    pub recursive: bool,
    /// Confine every path to this folder
    #[arg(long, value_name = "DIR", value_parser = path)]
    pub root: Option<PathBuf>,
    /// Stop at the first file that fails
    #[arg(long)]
    pub stop_on_error: bool,
    /// Optimize: the resolution images are reduced to (9 to 1600)
    #[arg(long, value_name = "PPI", value_parser = number_between(9.0, 1600.0), allow_negative_numbers = true)]
    pub ppi: Option<f64>,
    /// Optimize: JPEG quality (1 to 100)
    #[arg(long, value_name = "1-100", value_parser = clap::value_parser!(u8).range(1..=100), allow_negative_numbers = true)]
    pub quality: Option<u8>,
    /// OCR: the resolution pages are recognised at (72 to 600)
    #[arg(long, value_name = "DPI", value_parser = number_between(72.0, 600.0), allow_negative_numbers = true)]
    pub dpi: Option<f64>,
    /// OCR: the recognition language
    #[arg(long, value_name = "LANG", value_parser = NonEmptyStringValueParser::new())]
    pub language: Option<String>,
    /// OCR: also pages that already have text
    #[arg(long)]
    pub ocr_all_pages: bool,
}

impl BatchArgs {
    /// Options that belong to a step `--steps` leaves out. `doc_batch` refuses them too; this
    /// reports them as the usage errors they are.
    pub fn check(&self) -> Result<(), crate::error::UsageError> {
        use crate::error::UsageError::OptionNeedsStep;
        let has = |step| self.steps.contains(&step);
        let optimize = [("--ppi", self.ppi.is_some()), ("--quality", self.quality.is_some())];
        let ocr = [("--dpi", self.dpi.is_some()), ("--language", self.language.is_some()), ("--ocr-all-pages", self.ocr_all_pages)];
        for (option, given) in optimize {
            if given && !has(Step::Optimize) {
                return Err(OptionNeedsStep { option, step: "optimize" });
            }
        }
        for (option, given) in ocr {
            if given && !has(Step::Ocr) {
                return Err(OptionNeedsStep { option, step: "ocr" });
            }
        }
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// The tool to call (see `pdfcraft-cli tools`)
    #[arg(conflicts_with = "script", required_unless_present = "script")]
    pub tool: Option<String>,
    /// Its arguments as key=value; values parse as JSON when they can (pages=[1,3], degrees=90)
    #[arg(requires = "tool", value_name = "KEY=VALUE", value_parser = key_value)]
    pub args: Vec<KeyValue>,
    /// A JSON file of steps: [{"tool": "doc_open", "args": {…}}, …]
    #[arg(long, value_name = "FILE", value_parser = path)]
    pub script: Option<PathBuf>,
    /// Print one JSON document for the whole script
    #[arg(long)]
    pub report: bool,
    /// Confine every path a tool names to this folder
    #[arg(long, value_name = "DIR", value_parser = path)]
    pub root: Option<PathBuf>,
    /// Save the tool's image here
    #[arg(long, value_name = "FILE", conflicts_with = "script", value_parser = clap::builder::OsStringValueParser::new().try_map(utf8_path))]
    pub out: Option<PathBuf>,
}

impl RunArgs {
    /// `--report` is the report of a script. (clap's `requires` is satisfied here by the tool.)
    pub fn check(&self) -> Result<(), crate::error::UsageError> {
        if self.report && self.script.is_none() {
            return Err(crate::error::UsageError::ReportNeedsScript);
        }
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct UiArgs {
    /// The control file the app wrote
    #[arg(long, value_name = "FILE", required = true, value_parser = path)]
    pub control: PathBuf,
    /// state, inspect, click, drag, type, key, command, commands, set, open or screenshot
    pub method: String,
    #[arg(value_name = "KEY=VALUE", value_parser = key_value)]
    pub args: Vec<KeyValue>,
    /// Save a screenshot here
    #[arg(long, value_name = "FILE", value_parser = path)]
    pub out: Option<PathBuf>,
}

#[cfg(feature = "mcp")]
#[derive(Args, Debug)]
pub struct McpArgs {
    /// Confine every path a tool names to this folder
    #[arg(long, value_name = "DIR", value_parser = path)]
    pub root: Option<PathBuf>,
    /// List a core set of tools plus tool_search and tool_call
    #[arg(long)]
    pub compact: bool,
    /// Confine the files the agent's tools read (open, combine, import, fonts, certificates…) to
    /// DIR, links followed; a root left out grants none of that access. --root applies as well
    #[arg(long, value_name = "DIR", value_parser = path)]
    pub automation_read_root: Option<PathBuf>,
    /// Confine the files the agent's tools write (save, export, extract, split, reports…) to DIR
    #[arg(long, value_name = "DIR", value_parser = path)]
    pub automation_write_root: Option<PathBuf>,
}

/// What `mcp` is given in a build without the server: anything, to say it is not there.
#[cfg(not(feature = "mcp"))]
#[derive(Args, Debug)]
#[command(disable_help_flag = true)]
pub struct LeftOutArgs {
    #[arg(num_args = 0.., allow_hyphen_values = true, trailing_var_arg = true)]
    pub rest: Vec<String>,
}

/// Pages as 0-based indices. A newtype: clap reads a `Vec` field as a flag given many times.
#[derive(Clone, Debug, PartialEq)]
pub struct PageList(pub Vec<usize>);

/// A `key=value` of `run` and `ui`: JSON when it parses as JSON, else a string.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyValue {
    pub key: String,
    pub value: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rotate {
    pub pages: Vec<usize>,
    pub degrees: i64,
}

/// `to` is 0-based.
#[derive(Clone, Debug, PartialEq)]
pub struct Move {
    pub pages: Vec<usize>,
    pub to: usize,
}

/// The image `render` writes, and the format its name asks for (`None`: netpbm PAM).
#[derive(Clone, Debug)]
pub struct ImageOut {
    pub path: PathBuf,
    pub format: Option<pdfcraft_engine::export::ImageFormat>,
}

// ---- value parsers -----------------------------------------------------------------------------

/// A path the automation tools take: they name files as strings, so one that is not UTF-8 is
/// refused here and never renamed by a lossy conversion on the way.
fn utf8_path(s: std::ffi::OsString) -> Result<PathBuf, ValueError> {
    let s = s.to_str().ok_or(ValueError::NotUtf8)?;
    path(s)
}

fn path(s: &str) -> Result<PathBuf, ValueError> {
    if s.is_empty() { Err(ValueError::EmptyPath) } else { Ok(PathBuf::from(s)) }
}

fn page_number(s: &str) -> Result<NonZeroUsize, ValueError> {
    s.parse::<usize>().ok().and_then(NonZeroUsize::new).ok_or(ValueError::PageNumber)
}

fn dpi(s: &str) -> Result<f32, ValueError> {
    s.parse::<f32>().ok().filter(|dpi| dpi.is_finite() && *dpi > 0.0).ok_or(ValueError::Dpi)
}

fn number_between(min: f64, max: f64) -> impl Fn(&str) -> Result<f64, ValueError> + Clone + Send + Sync + 'static {
    move |s| s.parse::<f64>().ok().filter(|n| (min..=max).contains(n)).ok_or(ValueError::Between { min, max })
}

/// 1-based pages ("1,3,5-8": single pages and inclusive ranges, in the order given) → 0-based
/// indices (#1923).
pub fn page_list(s: &str) -> Result<PageList, ValueError> {
    let number = |t: &str, item: &str| t.trim().parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| ValueError::PageItem(item.to_owned()));
    let mut out = Vec::new();
    for item in s.split(',').map(str::trim) {
        match item.split_once('-') {
            Some((from, to)) => {
                let (from, to) = (number(from, item)?, number(to, item)?);
                if to < from {
                    return Err(ValueError::RangeBackwards(item.to_owned()));
                }
                if to - from >= MAX_RANGE {
                    return Err(ValueError::RangeTooLong(item.to_owned(), MAX_RANGE));
                }
                out.extend(from - 1..to);
            }
            None => out.push(number(item, item)? - 1),
        }
    }
    Ok(PageList(out))
}

fn rotate_spec(s: &str) -> Result<Rotate, ValueError> {
    let (pages, degrees) = s.split_once(':').ok_or(ValueError::RotateShape)?;
    let degrees: i64 = degrees.parse().map_err(|_| ValueError::RotateNumber(degrees.to_owned()))?;
    // The rule `rotate_pages` enforces, so a bad angle is named before any file is opened.
    if degrees % 90 != 0 {
        return Err(ValueError::RotateMultiple(degrees));
    }
    Ok(Rotate { pages: page_list(pages)?.0, degrees })
}

fn move_spec(s: &str) -> Result<Move, ValueError> {
    let (pages, to) = s.split_once(':').ok_or(ValueError::MoveShape)?;
    let to = to.parse::<usize>().ok().filter(|to| *to > 0).ok_or_else(|| ValueError::PageAtLeastOne(to.to_owned()))?;
    Ok(Move { pages: page_list(pages)?.0, to: to - 1 })
}

fn key_value(s: &str) -> Result<KeyValue, ValueError> {
    let (key, value) = s.split_once('=').ok_or(ValueError::KeyValue)?;
    let value = serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::String(value.to_owned()));
    Ok(KeyValue { key: key.to_owned(), value })
}

fn image_out(s: &str) -> Result<ImageOut, ValueError> {
    use pdfcraft_engine::export::ImageFormat;
    let path = path(s)?;
    // The file is what its name says (#248): a `.png` used to get a netpbm PAM stream.
    let format = match path.extension().and_then(std::ffi::OsStr::to_str).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => Some(ImageFormat::Png),
        Some("jpg" | "jpeg") => Some(ImageFormat::Jpeg { quality: 90 }),
        Some("tif" | "tiff") => Some(ImageFormat::Tiff),
        Some("pam") => None,
        _ => return Err(ValueError::ImageName),
    };
    Ok(ImageOut { path, format })
}
