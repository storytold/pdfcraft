//! pdfcraft-cli — headless PdfCraft.
//!
//! Usage: `pdfcraft-cli --help`, which prints `USAGE` below.
//!
//! `run` and `mcp` drive the same tool table (`pdfcraft-automation`). In `run`, values parse as
//! JSON when they can (`pages=[1,3]`, `degrees=90`) and are strings otherwise. A script runs its
//! steps in one session, so `doc_open` returns id 1, the next document id 2, and so on. A step's
//! `"out"` (like `--out`) saves its image there. With `--root DIR`, every path a tool or step
//! names, `"out"` included, must be inside DIR, and relative paths resolve inside it.
//!
//! The MCP server never starts on its own: it runs only when this command is launched (normally
//! by an agent configured to use it), talks only over stdio, and exits when stdin closes.
//!
//! `check` is the robustness harness: every file is opened, inspected and fully rendered in a
//! *separate child process* with a wall-clock timeout, so hangs, panics and aborts in any
//! dependency are observed and reported instead of taking the harness down.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(feature = "mcp")]
mod automation_args;
mod batch;
mod scan_cmd;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pdfcraft_engine::export::ImageFormat;
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind, inspect};

/// The `mcp` lines of [`USAGE`]: only a build with the `mcp` feature has the command.
#[cfg(feature = "mcp")]
macro_rules! mcp_usage {
    () => {
        "\
pdfcraft-cli mcp    [--root DIR] [--compact] [--automation-read-root DIR] [--automation-write-root DIR]
                                                           MCP server on stdin/stdout (opt-in)
                                                           --compact lists a core set of tools plus tool_search and tool_call
                                                           --automation-read-root / --automation-write-root confine the files the
                                                           agent's tools read (open, combine, import, fonts, certificates…) and
                                                           write (save, export, extract, split, reports…) to DIR, links followed;
                                                           a root left out grants none of its access. --root, when also given,
                                                           applies as well
"
    };
}
#[cfg(not(feature = "mcp"))]
macro_rules! mcp_usage {
    () => {
        ""
    };
}

mod check;
mod commands;
mod options;
mod run;
mod ui;

use check::{audit, check, check_one};
use commands::{combine, edit, extract, info, render, split, text};
use options::{OptionSpec, flag, validate_options};
#[cfg(feature = "mcp")]
use run::mcp;
use run::{automation, print_output, run, tools};
use ui::ui;

/// The full usage, printed by `--help` (also after a command, e.g. `render --help`).
const USAGE: &str = concat!(
    "\
pdfcraft-cli info   <file.pdf> [--password PW]            document summary as JSON
                                                          (includes any declared PDF/A or PDF/UA level)
pdfcraft-cli render <file.pdf> --page N [--dpi 96] --out x.png   (.png, .jpg, .tif or .pam)
pdfcraft-cli text   <file.pdf> [--page N]                  extracted text (pages separated by form feeds)
pdfcraft-cli edit   <in.pdf> --out out.pdf [--rotate 1,3:90] [--delete 2,4] [--move 5:1]
                      [--insert-blank 1] [--title T] [--author A] [--full]
                      (options run in order; page numbers refer to the document after the earlier options)
pdfcraft-cli combine <a.pdf> <b.pdf> … --out combined.pdf
pdfcraft-cli extract <in.pdf> --pages 1,3,5-8 --out out.pdf    (pages and ranges, in the order given)
pdfcraft-cli split   <in.pdf> (--every N | --before 3,7) [--out-dir DIR]
pdfcraft-cli check  <files or dirs…> [--timeout 20] [--dpi 36] [--json out.json]
pdfcraft-cli audit  <file.pdf> [--password PW]            find faux redactions (fails when found)
pdfcraft-cli batch <files or dirs...> --steps convert,ocr,optimize --out-dir DIR
                      [--recursive] [--root DIR] [--stop-on-error]
                      [--ppi 150] [--quality 60] [--dpi 300] [--language en] [--ocr-all-pages]
pdfcraft-cli scan   devices [--device TEXT | --details | --dump]
                      list scanners without opening them; --device opens matches, --details all; --dump every property the backend publishes (text)
pdfcraft-cli scan   check [--details | --device TEXT [--acquire [--out DIR]]] [--json]
                      what the scan stack sees; no paper moves unless --acquire (needs --device: one 100 dpi page; exit 1 on any problem)
pdfcraft-cli scan   --out DIR --yes [--dpi 200] [--color gray|rgb|bw]
                      [--source auto|feeder|glass] [--pages 1|all] [--device NAME]
                      [--brightness N] [--contrast N] [--threshold N]
                      [--rotation portrait|landscape|180|270]
                      [--format bmp|jpeg|png|tiff] [--compression none|jpeg|g4|…]
                      [--intent text|gray|color|…] [--page-size letter|a4|…]
                      [--page-length driver|size|auto|infer]  # size: the --page-size length; auto: scanner detection, else size; infer: approximate, from the image
                      [--deskew on|off] [--dpi-y N] [--orientation N] [--photometric N]
                      [--invert on|off] [--mirror on|off] [--lamp on|off] [--duplex]
                      [--left IN] [--top IN] [--width IN] [--height IN]
                      runs as one job: pages are saved to DIR as they arrive, DIR/scan-job.json records the job,
                      the result is one JSON line, and the exit status says how it ended:
                      0 completed, 10 needs attention (interrupted, finished short, or outcome unknown),
                      11 failed, 12 cancelled; 1 is a usage or file error before any job state.
                      Terminating the process mid-scan leaves the journal 'acquiring', read later as outcome unknown.
pdfcraft-cli scan   jobs [DIR...] [--json]
                      list the scan-job.json journals in each DIR (default: here and the folders directly below); opens no scanner
pdfcraft-cli scan   job DIR [--json]
                      one job in detail: state, pages kept and on disk, attempts, last error, next step; opens no scanner
pdfcraft-cli scan   --continue DIR (--append | --restart | --reverse) --yes [--i-checked-the-scanner]
                      go on with a job from its journal and request. --append keeps the kept pages and scans the
                      missing ones; --restart moves the kept page files to DIR/discarded-N and scans again; one is
                      required when pages are kept. A job that needs attention (outcome unknown) is refused until you
                      have looked at the scanner and add --i-checked-the-scanner: a repeated acquire moves the carriage
                      and pulls paper.
                      For a finished whole-feeder job: --append adds the next feeder load as more sheets; --reverse scans
                      the backs (flip the whole stack first): one job, every page saved at its place (the back of
                      sheet i is page 2i), continued with --append for each further part of the flipped stack
pdfcraft-cli tools                                       automation tools and their JSON Schemas
pdfcraft-cli run    <tool> [key=value …] [--root DIR] [--out image.png]
pdfcraft-cli run    --script steps.json [--root DIR] [--report]
                      [{\"tool\": \"doc_open\", \"args\": {…}}, …]; --report prints one JSON document for the run
",
    mcp_usage!(),
    "\
pdfcraft-cli ui     --control FILE <method> [key=value …] [--out shot.png]
                                                           drive a running app started with --control FILE
help and feedback: https://discord.gg/artcraft"
);

/// Entry point: dispatches the first argument to its command and turns the result into the exit status. A closed
/// stdout pipe counts as success; a `CliError::Exit` keeps the job's own exit code.
fn main() -> ExitCode {
    // `std::env::args` panics on an argument that is not UTF-8 ("called `Result::unwrap()` on an
    // `Err` value"), which a Latin-1 file name is, and it panics while collecting them: even
    // `--help` ended that way. Such a name cannot be passed on either, since the paths here are
    // `str`, so it is a usage error (photocraft#1108).
    let mut args: Vec<String> = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.into_string() {
            Ok(a) => args.push(a),
            Err(bad) => {
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "pdfcraft-cli: `{}` is not valid UTF-8, so it cannot be used as an argument",
                    bad.to_string_lossy()
                );
                return ExitCode::FAILURE;
            }
        }
    }
    // `<command> --help` too; only right after the command, so a later `-h` (a value) still reaches it.
    let command = match args.get(1).map(String::as_str) {
        Some("--help" | "-h") => Some("--help"),
        _ => args.first().map(String::as_str),
    };
    let result = match command {
        Some("--help" | "-h" | "help") => stdout_line(format_args!("{USAGE}")),
        None => Err(format!("usage:\n{USAGE}").into()),
        Some("info") => info(&args[1..]),
        Some("render") => render(&args[1..]),
        Some("text") => text(&args[1..]),
        Some("edit") => edit(&args[1..]),
        Some("combine") => combine(&args[1..]),
        Some("extract") => extract(&args[1..]),
        Some("split") => split(&args[1..]),
        Some("check") => check(&args[1..]),
        Some("check-one") => check_one(&args[1..]),
        Some("audit") => audit(&args[1..]),
        Some("scan") => scan(&args[1..]),
        Some("tools") => tools(),
        Some("run") => run(&args[1..]),
        Some("batch") => batch::run(&args[1..]),
        Some("ui") => ui(&args[1..]),
        #[cfg(feature = "mcp")]
        Some("mcp") => mcp(&args[1..]),
        #[cfg(not(feature = "mcp"))]
        Some("mcp") => Err("mcp: this build leaves out the MCP server (built without the `mcp` feature)".into()),
        Some("--version") => version(),
        Some(other) => Err(format!("unknown command '{other}'; see pdfcraft-cli --help").into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Stdout(e)) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(stderr, "pdfcraft-cli: {e}");
            if let CliError::Exit { code, json, .. } = &e {
                if let Some(json) = json {
                    let _ = writeln!(stderr, "{json}");
                }
                return ExitCode::from(*code);
            }
            ExitCode::FAILURE
        }
    }
}

// Keep stdout errors typed: a closed pipe is normal, but file and command errors still fail.
#[derive(Debug)]
enum CliError {
    Message(String),
    Stdout(std::io::Error),
    /// A scan job that did not complete: its exit status by `ExitClass` (see `scan --help` text), the sentence
    /// for stderr, and the failure's identity as one line of JSON when there is one. The result JSON is
    /// already on stdout.
    Exit {
        code: u8,
        message: String,
        json: Option<String>,
    },
}

impl From<String> for CliError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for CliError {
    fn from(message: &str) -> Self {
        Self::Message(message.to_string())
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(message) | Self::Exit { message, .. } => f.write_str(message),
            Self::Stdout(error) => write!(f, "stdout: {error}"),
        }
    }
}

fn stdout_line(line: std::fmt::Arguments<'_>) -> Result<(), CliError> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}").and_then(|()| stdout.flush()).map_err(CliError::Stdout)
}

fn version() -> Result<(), CliError> {
    stdout_line(format_args!("pdfcraft-cli {}", pdfcraft_engine::build_info::version()))?;
    stdout_line(format_args!("Discord: {}  (help and feedback)", pdfcraft_engine::links::DISCORD))?;
    stdout_line(format_args!("Web:     {}", pdfcraft_engine::links::APP_PAGE))?;
    stdout_line(format_args!("Source:  {}", pdfcraft_engine::links::GITHUB))?;
    Ok(())
}
const SCAN_OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--out", takes_value: true },
    OptionSpec { name: "--dpi", takes_value: true },
    OptionSpec { name: "--color", takes_value: true },
    OptionSpec { name: "--source", takes_value: true },
    OptionSpec { name: "--pages", takes_value: true },
    OptionSpec { name: "--device", takes_value: true },
    OptionSpec { name: "--brightness", takes_value: true },
    OptionSpec { name: "--contrast", takes_value: true },
    OptionSpec { name: "--threshold", takes_value: true },
    OptionSpec { name: "--rotation", takes_value: true },
    OptionSpec { name: "--format", takes_value: true },
    OptionSpec { name: "--compression", takes_value: true },
    OptionSpec { name: "--intent", takes_value: true },
    OptionSpec { name: "--page-size", takes_value: true },
    OptionSpec { name: "--page-length", takes_value: true },
    OptionSpec { name: "--deskew", takes_value: true },
    OptionSpec { name: "--dpi-y", takes_value: true },
    OptionSpec { name: "--orientation", takes_value: true },
    OptionSpec { name: "--photometric", takes_value: true },
    OptionSpec { name: "--invert", takes_value: true },
    OptionSpec { name: "--mirror", takes_value: true },
    OptionSpec { name: "--lamp", takes_value: true },
    OptionSpec { name: "--left", takes_value: true },
    OptionSpec { name: "--top", takes_value: true },
    OptionSpec { name: "--width", takes_value: true },
    OptionSpec { name: "--height", takes_value: true },
    OptionSpec { name: "--continue", takes_value: true },
    OptionSpec { name: "--append", takes_value: false },
    OptionSpec { name: "--restart", takes_value: false },
    OptionSpec { name: "--i-checked-the-scanner", takes_value: false },
    OptionSpec { name: "--yes", takes_value: false },
    OptionSpec { name: "--duplex", takes_value: false },
    OptionSpec { name: "--reverse", takes_value: false },
    OptionSpec { name: "--dump", takes_value: false },
    OptionSpec { name: "--details", takes_value: false },
    OptionSpec { name: "--acquire", takes_value: false },
    OptionSpec { name: "--json", takes_value: false },
];

/// `scan devices` lists scanners without opening any (id, name, port, connection, `details:
/// "skipped"`). `--device TEXT` opens only the devices whose id, name or port contains TEXT and
/// returns their capabilities (`details: "loaded"`); `--details` opens all of them. A device that
/// will not open stays listed with `details: "error"` and the reason.
/// `scan devices --dump` prints every property the backend publishes, as text. Anything else
/// pulls paper, and only with `--yes`. A pile larger than the feeder is one job: `scan --continue DIR
/// --append --yes` adds the next feeder load. Manual duplex (a feeder without a duplex unit) turns
/// the same job: `scan --continue DIR --reverse --yes` with the whole stack flipped, then `--append`
/// for each further part of it; every page is written to its place in the document as it arrives.
/// Scanning is Windows/WIA only in this version: elsewhere a scan ends with one clear line (the
/// device commands get the same sentence from the unsupported backend).
/// This is a run-time answer; the code builds on every platform.
fn ensure_scanning_supported() -> Result<(), CliError> {
    if pdfcraft_platform::scan::supported() {
        return Ok(());
    }
    Err(format!("scan: {}", pdfcraft_platform::scan::ScanError::Unsupported).into())
}

/// `scan check`: report what the scan stack sees, without scanning. The device list is cheap (no
/// device is opened); `--details` opens every device read-only and `--device TEXT` opens exactly one.
/// `--acquire` (which needs `--device`) is the only part that moves the scanner: one 100 dpi page,
/// reported as size, format and byte count, and saved only into `--out`. The report goes to stdout
/// (`--json` for the machine form); exit status 1 means a device would not open or reported an error.
fn scan_check(args: &[String]) -> Result<(), CliError> {
    let mut options = pdfcraft_automation::ScanCheckOptions::default();
    let mut json = false;
    let mut index = 1;
    while let Some(arg) = args.get(index) {
        index += 1;
        match arg.as_str() {
            "--details" => options.details = true,
            "--acquire" => options.acquire = true,
            "--json" => json = true,
            "--device" | "--out" => {
                let value = args.get(index).filter(|value| !value.starts_with("--")).ok_or_else(|| format!("scan check: {arg} needs a value"))?;
                index += 1;
                if arg == "--device" {
                    options.device = Some(value.clone());
                } else {
                    options.out = Some(PathBuf::from(value));
                }
            }
            other => return Err(format!("scan check takes only --details, --device TEXT, --acquire, --out DIR, and --json (got {other})").into()),
        }
    }
    let mut notice = |line: &str| eprintln!("pdfcraft-cli: {line}");
    let check = pdfcraft_automation::scan_check(&options, &mut notice).map_err(|error| format!("scan check: {error}"))?;
    if json {
        let line = serde_json::to_string(&check.report).map_err(|error| format!("scan check: {error}"))?;
        stdout_line(format_args!("{line}"))?;
    } else {
        stdout_line(format_args!("{}", pdfcraft_automation::render_check_text(&check.report).trim_end()))?;
    }
    if check.ok {
        Ok(())
    } else {
        let count = check.report["problems"].as_array().map_or(0, Vec::len);
        Err(format!("scan check: {count} problem(s) found").into())
    }
}

/// `scan ...`: routes `check`, `jobs`, `job`, `devices` and `--continue` to their handlers; otherwise parses the
/// scan options into `ScanSettings` and runs a fresh job. Options are all validated before the output folder
/// is created, so a bad option leaves nothing behind.
fn scan(args: &[String]) -> Result<(), CliError> {
    validate_options("scan", args, SCAN_OPTIONS)?;
    if args.first().map(String::as_str) == Some("check") {
        return scan_check(args);
    }
    match args.first().map(String::as_str) {
        Some("jobs") => return scan_cmd::jobs(args),
        Some("job") => return scan_cmd::job(args),
        _ => {}
    }
    if args.iter().any(|arg| arg == "--continue") {
        return scan_continue(args);
    }
    // By position, like `check`, `jobs` and `job`: `scan --out devices --yes` names a folder, it is not the subcommand.
    if args.first().map(String::as_str) == Some("devices") {
        let dump = args.iter().any(|arg| arg == "--dump");
        let details = args.iter().any(|arg| arg == "--details");
        let filter = option_value(args, "--device")?;
        let mut unexpected = 0;
        let mut index = 0;
        while let Some(arg) = args.get(index) {
            index += if arg == "--device" { 2 } else { 1 };
            if !matches!(arg.as_str(), "devices" | "--dump" | "--details" | "--device") {
                unexpected += 1;
            }
        }
        if unexpected != 0 || (dump && (details || filter.is_some())) {
            return Err("scan devices takes only the optional --device TEXT, --details, or --dump".into());
        }
        return if dump { dump_scanners() } else { list_scanners(filter, details) };
    }
    let out = option_value(args, "--out")?.ok_or("scan: --out DIR is required (scan devices lists scanners without feeding paper)")?;
    if !args.iter().any(|arg| arg == "--yes") {
        return Err("scan: --yes is required, because this pulls paper".into());
    }
    if args.iter().any(|arg| arg == "--reverse") {
        return Err(format!(
            "scan: the backs of a scan are part of the same job: flip the whole stack and run `pdfcraft-cli scan --continue {out} --reverse --yes`"
        )
        .into());
    }
    let duplex = args.iter().any(|arg| arg == "--duplex");
    let dpi = match option_value(args, "--dpi")? {
        Some(text) => text.parse::<u32>().map_err(|_| "scan: --dpi is a whole number")?,
        None => 200,
    };
    let color_text = option_value(args, "--color")?.unwrap_or("gray");
    let color = pdfcraft_platform::scan::Color::parse(color_text).ok_or_else(|| format!("scan: --color is gray, rgb, or bw (got {color_text})"))?;
    let source_text = option_value(args, "--source")?.unwrap_or("auto");
    let source =
        pdfcraft_platform::scan::Source::parse(source_text).ok_or_else(|| format!("scan: --source is auto, feeder, or glass (got {source_text})"))?;
    let folder = PathBuf::from(out);
    let pages = match option_value(args, "--pages")?.unwrap_or("1") {
        "all" => 0,
        text => text.parse::<u32>().map_err(|_| "scan: --pages is a whole number, or all")?,
    };
    let rotation = match option_value(args, "--rotation")? {
        None => None,
        Some(text) => Some(
            pdfcraft_platform::scan::Rotation::parse(text)
                .ok_or_else(|| format!("scan: --rotation is portrait, landscape, 180, or 270 (got {text})"))?,
        ),
    };
    let intent = match option_value(args, "--intent")? {
        None => None,
        Some(text) => Some(
            pdfcraft_platform::scan::Intent::parse(text)
                .ok_or_else(|| format!("scan: --intent is none, color, gray, text, preview, quality, or size (got {text})"))?,
        ),
    };
    let page_size = match option_value(args, "--page-size")? {
        None => None,
        Some(text) => Some(
            pdfcraft_platform::scan::PageSize::parse(text)
                .ok_or_else(|| format!("scan: --page-size is a4, letter, legal, auto, custom, or business-card (got {text})"))?,
        ),
    };
    let page_length = match option_value(args, "--page-length")? {
        None => pdfcraft_platform::scan::PageLength::Driver,
        Some(text) => pdfcraft_platform::scan::PageLength::parse(text).ok_or_else(|| {
            format!(
                "scan: --page-length is driver (the scanner's own length, the default), size (the --page-size length as extents), auto (the scanner's detection when it has one, else the --page-size length) or infer (auto, then a guess from the image: approximate) (got {text})"
            )
        })?,
    };
    let auto_deskew = optional_on_off(args, "--deskew")?;
    let dpi_y = match option_value(args, "--dpi-y")? {
        Some(text) => Some(text.parse::<u32>().map_err(|_| "scan: --dpi-y is a whole number")?),
        None => None,
    };
    let settings = pdfcraft_platform::scan::ScanSettings {
        device: option_value(args, "--device")?.unwrap_or("").to_string(),
        source,
        dpi,
        dpi_y,
        color,
        pages,
        brightness: optional_i32(args, "--brightness")?,
        contrast: optional_i32(args, "--contrast")?,
        threshold: optional_i32(args, "--threshold")?,
        rotation,
        region: region_from_args(args)?,
        format: option_value(args, "--format")?.map(str::to_string),
        compression: option_value(args, "--compression")?.map(str::to_string),
        intent,
        page_size,
        page_length,
        auto_deskew,
        orientation: optional_i32(args, "--orientation")?,
        photometric: optional_i32(args, "--photometric")?,
        invert: optional_on_off(args, "--invert")?,
        mirror: optional_on_off(args, "--mirror")?,
        lamp: optional_on_off(args, "--lamp")?,
        duplex,
        cancel: None,
        progress: None,
    };
    settings.checked().map_err(|error| format!("scan: {error}"))?;
    ensure_scanning_supported()?;
    refuse_a_folder_that_has_a_job(&folder)?;
    // Only now: an option that did not parse must not leave an empty output folder behind.
    std::fs::create_dir_all(&folder).map_err(|error| format!("scan: {}: {error}", folder.display()))?;
    scan_cmd::run(scan_cmd::Run { folder, settings, resume: None, duplex: None })
}

/// One job per output folder. A fresh scan does not start in a folder that already holds a journal (it
/// would overwrite the record of what happened there) or page files (the pages would collide). More sheets
/// and the backs of a job go through `scan --continue`.
fn refuse_a_folder_that_has_a_job(folder: &Path) -> Result<(), CliError> {
    use pdfcraft_automation::scan_jobs;
    if folder.join(scan_jobs::JOB_FILE).is_file() {
        let record = scan_jobs::read_record(folder).map_err(|error| format!("scan: {error}"))?;
        {
            return Err(format!(
                "scan: {} already holds a scan job ({}). One job per folder: look at it with `pdfcraft-cli scan job {}`, go on with `scan --continue {}`, or scan into another folder",
                folder.display(),
                record.summary_line(),
                folder.display(),
                folder.display()
            )
            .into());
        }
    }
    // A folder that does not exist yet holds nothing; one that cannot be read is an error, not an empty folder
    // (a page file in it would collide with the first page written, after paper was pulled).
    let files = match pdfcraft_automation::scan_jobs::page_numbers(folder) {
        Ok(files) => files,
        Err(_) if !folder.exists() => Vec::new(),
        Err(error) => return Err(format!("scan: cannot read {}: {error}", folder.display()).into()),
    };
    if !files.is_empty() {
        return Err(format!("scan: {} already holds {} page file(s); scan into an empty folder", folder.display(), files.len()).into());
    }
    Ok(())
}

/// `scan --continue DIR`: the settings come from the job file, so no scan option may be given beside it.
fn scan_continue(args: &[String]) -> Result<(), CliError> {
    const ALLOWED: &[&str] = &["--continue", "--append", "--restart", "--reverse", "--yes", "--i-checked-the-scanner"];
    let dir = option_value(args, "--continue")?.ok_or("scan: --continue needs DIR")?;
    if let Some(other) = args.iter().filter(|arg| arg.starts_with("--")).find(|arg| !ALLOWED.contains(&arg.as_str())) {
        return Err(format!(
            "scan --continue takes only --append, --restart or --reverse, --yes and --i-checked-the-scanner; the settings come from the job file (got {other})"
        )
        .into());
    }
    if !args.iter().any(|arg| arg == "--yes") {
        return Err("scan: --yes is required, because this pulls paper".into());
    }
    let flags = scan_cmd::ContinueFlags {
        append: args.iter().any(|arg| arg == "--append"),
        restart: args.iter().any(|arg| arg == "--restart"),
        checked: args.iter().any(|arg| arg == "--i-checked-the-scanner"),
        reverse: args.iter().any(|arg| arg == "--reverse"),
    };
    scan_cmd::continue_job(Path::new(dir), &flags)
}

/// One saved page as the JSON row printed in the result: file path, format, pixel size, dpi, byte count,
/// whether it was thresholded, and the page-length report.
fn page_json(path: &Path, page: &pdfcraft_platform::scan::Page) -> serde_json::Value {
    serde_json::json!({
        "path": path.display().to_string(),
        "format": page.format,
        "width": page.width,
        "height": page.height,
        "dpi": page.dpi,
        "bytes": page.bytes.len(),
        "thresholded": page.thresholded,
        "length": pdfcraft_automation::scan_length_json(&page.length),
    })
}

/// Print the scanner listing (optionally only devices matching `filter`, with `details` read from each opened
/// device) as one line of JSON.
fn list_scanners(filter: Option<&str>, details: bool) -> Result<(), CliError> {
    let listing = pdfcraft_automation::list_scan_devices(filter, details).map_err(|error| format!("scan: {error}"))?;
    let line = serde_json::to_string(&listing).map_err(|error| format!("scan: {error}"))?;
    stdout_line(format_args!("{line}"))
}

/// Print the backend's plain-text diagnostic dump of every scanner, without trailing whitespace.
fn dump_scanners() -> Result<(), CliError> {
    // Text, rendered by each backend (for WIA, every published property).
    let text = pdfcraft_platform::scan::diagnostic_dump("").map_err(|error| format!("scan: {error}"))?;
    stdout_line(format_args!("{}", text.trim_end()))
}

/// Read `--left/--top/--width/--height` (inches) into a scan region. `None` when none of the four is given;
/// giving any one of them requires all four.
fn region_from_args(args: &[String]) -> Result<Option<pdfcraft_platform::scan::Region>, CliError> {
    let left = option_value(args, "--left")?;
    let top = option_value(args, "--top")?;
    let width = option_value(args, "--width")?;
    let height = option_value(args, "--height")?;
    if left.is_none() && top.is_none() && width.is_none() && height.is_none() {
        return Ok(None);
    }
    let inches = |name: &str, text: Option<&str>| -> Result<f64, CliError> {
        let text = text.ok_or_else(|| format!("scan: {name} is required with a region"))?;
        text.parse::<f64>().map_err(|_| format!("scan: {name} is a number of inches").into())
    };
    pdfcraft_platform::scan::region_from_inches(
        inches("--left", left)?,
        inches("--top", top)?,
        inches("--width", width)?,
        inches("--height", height)?,
    )
    .map(Some)
    .map_err(|error| format!("scan: {error}").into())
}

/// The whole-number value of option `name`, or `None` when the option is absent.
fn optional_i32(args: &[String], name: &str) -> Result<Option<i32>, CliError> {
    match option_value(args, name)? {
        None => Ok(None),
        Some(text) => text.parse().map(Some).map_err(|_| format!("scan: {name} is a whole number").into()),
    }
}

/// The on/off value of option `name` (`on`/`true`/`1`/`yes`, `off`/`false`/`0`/`no`), or `None` when absent.
fn optional_on_off(args: &[String], name: &str) -> Result<Option<bool>, CliError> {
    match option_value(args, name)? {
        None => Ok(None),
        Some("on" | "true" | "1" | "yes") => Ok(Some(true)),
        Some("off" | "false" | "0" | "no") => Ok(Some(false)),
        Some(other) => Err(format!("scan: {name} is on or off (got {other})").into()),
    }
}

/// The argument after `name`, or `None` when the option is absent. A missing value, or one that starts with
/// `--`, is an error.
fn option_value<'a>(args: &'a [String], name: &str) -> Result<Option<&'a str>, CliError> {
    let Some(index) = args.iter().position(|arg| arg == name) else {
        return Ok(None);
    };
    match args.get(index + 1).map(String::as_str) {
        Some(value) if !value.starts_with("--") => Ok(Some(value)),
        _ => Err(format!("scan: {name} needs a value").into()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn scanning_reports_one_line_where_there_is_no_backend() {
        match super::ensure_scanning_supported() {
            Ok(()) => assert!(pdfcraft_platform::scan::supported(), "accepted a scan with no backend registered"),
            Err(refusal) => assert_eq!(refusal.to_string(), "scan: scanning is only supported on Windows (WIA) in this version"),
        }
    }

    #[test]
    fn scan_refuses_to_pull_paper_without_confirmation() {
        let missing = super::scan(&["--out".into(), "pages".into()]).unwrap_err();
        assert!(missing.to_string().contains("--yes"), "{missing}");
        let color = super::scan(&["--out".into(), "pages".into(), "--yes".into(), "--color".into(), "purple".into()]).unwrap_err();
        assert!(color.to_string().contains("color"), "{color}");
        let turn = super::scan(&["--out".into(), "pages".into(), "--yes".into(), "--rotation".into(), "sideways".into()]).unwrap_err();
        assert!(turn.to_string().contains("rotation"), "{turn}");
        let region = super::scan(&[
            "--out".into(),
            "pages".into(),
            "--yes".into(),
            "--left".into(),
            "0".into(),
            "--top".into(),
            "0".into(),
            "--width".into(),
            "0".into(),
            "--height".into(),
            "1".into(),
        ])
        .unwrap_err();
        assert!(region.to_string().contains("width"), "{region}");
    }

    #[test]
    fn scan_refuses_unknown_source_intent_page_size_and_deskew() {
        let source = super::scan(&["--out".into(), "pages".into(), "--yes".into(), "--source".into(), "tray".into()]).unwrap_err();
        assert!(source.to_string().contains("source"), "{source}");
        let intent = super::scan(&["--out".into(), "pages".into(), "--yes".into(), "--intent".into(), "photo".into()]).unwrap_err();
        assert!(intent.to_string().contains("intent"), "{intent}");
        let page = super::scan(&["--out".into(), "pages".into(), "--yes".into(), "--page-size".into(), "tabloid".into()]).unwrap_err();
        assert!(page.to_string().contains("page-size"), "{page}");
        let deskew = super::scan(&["--out".into(), "pages".into(), "--yes".into(), "--deskew".into(), "maybe".into()]).unwrap_err();
        assert!(deskew.to_string().contains("deskew"), "{deskew}");
        let dpi_y = super::scan(&["--out".into(), "pages".into(), "--yes".into(), "--dpi-y".into(), "x".into()]).unwrap_err();
        assert!(dpi_y.to_string().contains("dpi-y"), "{dpi_y}");
        let length = super::scan(&["--out".into(), "pages".into(), "--yes".into(), "--page-length".into(), "exact".into()]).unwrap_err();
        let text = length.to_string();
        assert!(text.contains("page-length") && text.contains("approximate"), "{text}");
    }

    #[test]
    fn a_scan_with_an_invalid_option_does_not_create_the_output_folder() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-scan-no-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let out = dir.to_string_lossy().into_owned();
        for bad in [["--rotation", "sideways"], ["--intent", "photo"], ["--page-size", "tabloid"], ["--dpi-y", "x"]] {
            let args = ["--out", out.as_str(), "--yes", bad[0], bad[1]].map(String::from);
            assert!(super::scan(&args).is_err());
            assert!(!dir.exists(), "{} left behind by {bad:?}", dir.display());
        }
    }

    #[test]
    fn a_saved_page_reports_how_its_length_was_obtained() {
        use pdfcraft_platform::scan::{LengthRecord, LengthSource, Page};
        let page = Page {
            bytes: vec![0; 4],
            format: "bmp",
            width: 1700,
            height: 2146,
            dpi: 200,
            dpi_y: 200,
            requested_dpi: 200,
            thresholded: false,
            length: LengthRecord::with_source(2146, 200, LengthSource::Inferred),
        };
        let json = super::page_json(std::path::Path::new("page-001.bmp"), &page);
        assert_eq!(json["length"]["source"], "inferred");
        assert_eq!(json["length"]["approximate"], true);
        assert_eq!(json["length"]["length_inches"], 10.73);
    }

    #[test]
    fn scan_devices_takes_only_its_own_flags() {
        for args in
            [vec!["devices", "--yes"], vec!["devices", "stray"], vec!["devices", "--dump", "--details"], vec!["devices", "--dump", "--device", "x"]]
        {
            let args: Vec<String> = args.into_iter().map(String::from).collect();
            let error = super::scan(&args).unwrap_err();
            assert!(error.to_string().contains("scan devices") || error.to_string().contains("unknown option"), "{args:?}: {error}");
        }
        let missing = super::scan(&["devices".into(), "--device".into()]).unwrap_err();
        assert!(missing.to_string().contains("--device"), "{missing}");
        let empty = super::scan(&["devices".into(), "--device".into(), " ".into()]).unwrap_err();
        assert!(empty.to_string().contains("empty"), "{empty}");
    }

    #[test]
    fn scan_reverse_is_a_continuation_of_the_job() {
        let duplex = super::scan(&["--out".into(), "pages".into(), "--yes".into(), "--reverse".into(), "--duplex".into()]).unwrap_err();
        assert!(duplex.to_string().contains("--reverse"), "{duplex}");
        let dir = std::env::temp_dir().join(format!("pdfcraft-scan-reverse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(std::fs::create_dir_all(&dir).is_ok());
        let fresh = super::scan(&["--out".into(), dir.to_string_lossy().into_owned(), "--yes".into(), "--reverse".into()]).unwrap_err();
        assert!(fresh.to_string().contains("--continue") && fresh.to_string().contains("--reverse"), "{fresh}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fresh_scan_needs_an_empty_or_missing_folder_it_can_read() {
        let base = std::env::temp_dir().join(format!("pdfcraft-cli-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        assert!(super::refuse_a_folder_that_has_a_job(&base).is_ok(), "a folder that does not exist yet");
        std::fs::create_dir_all(&base).expect("folder");
        assert!(super::refuse_a_folder_that_has_a_job(&base).is_ok(), "an empty folder");
        std::fs::write(base.join("page-001.png"), b"x").expect("page");
        assert!(super::refuse_a_folder_that_has_a_job(&base).unwrap_err().to_string().contains("1 page file(s)"));
        // A path that cannot be listed (here a file) is an error, not an empty folder.
        let error = super::refuse_a_folder_that_has_a_job(&base.join("page-001.png")).unwrap_err().to_string();
        assert!(error.contains("cannot read"), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }
}
