//! pdfcraft-cli — headless PdfCraft.
//!
//! Usage: `pdfcraft-cli --help`; `pdfcraft-cli <command> --help` for a command.
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
//!
//! The command line follows RFC-0012: arguments are parsed by clap before any work, a usage error
//! (unknown or repeated flag, bad value, conflicting flags) exits 2 on stderr, a command that
//! fails exits 1 with `error: <message>`, and results go to stdout.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod check;
mod cli;
mod commands;
mod error;
mod run;
mod ui;

use std::ffi::OsString;
use std::io::Write;

use clap::{CommandFactory, FromArgMatches};

pub use cli::{Cli, Cmd};
pub use error::{CliError, UsageError, ValueError};

/// Where a command's results and diagnostics go.
pub(crate) struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

/// One line on stdout, flushed. A write error is typed so a closed pipe can be told apart.
pub(crate) fn stdout_line(io: &mut Io, line: std::fmt::Arguments<'_>) -> Result<(), CliError> {
    writeln!(io.out, "{line}").and_then(|()| io.out.flush()).map_err(CliError::Stdout)
}

/// The clap command: the definition `--help`, completions and man pages come from.
pub fn command() -> clap::Command {
    let version = pdfcraft_engine::build_info::version();
    let long = format!(
        "{version}\nDiscord: {}  (help and feedback)\nWeb:     {}\nSource:  {}",
        pdfcraft_engine::links::DISCORD,
        pdfcraft_engine::links::APP_PAGE,
        pdfcraft_engine::links::GITHUB
    );
    Cli::command().version(version).long_version(long)
}

/// Parse `args` (the program name first), ready to run. `Err` is what clap prints: a usage error,
/// or help or the version, which exit 0.
pub fn parse<I, T>(args: I) -> Result<Parsed, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let mut cmd = command();
    let matches = cmd.try_get_matches_from_mut(args)?;
    let cli = Cli::from_arg_matches(&matches)?;
    // What the parser cannot see: flags that are fine alone and wrong together.
    let (name, checked) = match &cli.command {
        Cmd::Batch(args) => ("batch", args.check()),
        Cmd::Run(args) => ("run", args.check()),
        _ => ("", Ok(())),
    };
    if let Err(e) = checked {
        let mut fallback = clap::Command::new(name);
        return Err(e.into_clap(cmd.find_subcommand_mut(name).unwrap_or(&mut fallback)));
    }
    Ok(Parsed { cli, matches })
}

/// A parsed command line. `edit` needs the matches too, for the order of its options.
#[derive(Debug)]
pub struct Parsed {
    pub cli: Cli,
    matches: clap::ArgMatches,
}

/// Run the command line `args` (the program name first), writing results to `out` and diagnostics
/// to `err`; returns the exit status: 0 success, 1 the command failed, 2 usage error.
pub fn run<I, T>(args: I, out: &mut dyn Write, err: &mut dyn Write) -> u8
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let parsed = match parse(args) {
        Ok(parsed) => parsed,
        // Help and `--version` come back as errors too: stdout, exit 0, and the command never runs.
        // Writing them can fail like any other output (a full disk), and then it is an error.
        Err(e) if !e.use_stderr() => {
            let written = write!(out, "{}", e.render()).and_then(|()| out.flush());
            return finish(err, written.map_err(CliError::Stdout), u8::try_from(e.exit_code()).unwrap_or(0));
        }
        Err(e) => {
            let _ = write!(err, "{}", e.render());
            return u8::try_from(e.exit_code()).unwrap_or(2);
        }
    };
    let mut io = Io { out, err };
    let result = execute(&parsed, &mut io);
    finish(io.err, result, 0)
}

/// The exit status of a finished command: `ok` when it succeeded, 0 when only the reader of our
/// stdout went away, else 1 with `error: <message>` on `err`.
fn finish(err: &mut dyn Write, result: Result<(), CliError>, ok: u8) -> u8 {
    match result {
        Ok(()) => ok,
        Err(e) if e.is_closed_pipe() => 0,
        Err(e) => {
            let _ = writeln!(err, "error: {e}");
            1
        }
    }
}

fn execute(parsed: &Parsed, io: &mut Io) -> Result<(), CliError> {
    match &parsed.cli.command {
        Cmd::Info(args) => commands::info(args, io),
        Cmd::Render(args) => commands::render(args, io),
        Cmd::Text(args) => commands::text(args, io),
        Cmd::Edit(args) => {
            let matches = parsed.matches.subcommand_matches("edit");
            let steps = matches.map(|m| args.steps(m)).unwrap_or_default();
            commands::edit(args, steps)
        }
        Cmd::Combine(args) => commands::combine(args),
        Cmd::Extract(args) => commands::extract(args),
        Cmd::Split(args) => commands::split(args, io),
        Cmd::Check(args) => check::check(args, io),
        Cmd::CheckOne(args) => check::check_one(args, io),
        Cmd::Audit(args) => commands::audit(args, io),
        Cmd::Batch(args) => run::batch(args, io),
        Cmd::Tools => run::tools(io),
        Cmd::Run(args) => run::run(args, io),
        Cmd::Ui(args) => ui::ui(args, io),
        #[cfg(feature = "mcp")]
        Cmd::Mcp(args) => run::mcp(args, io),
        #[cfg(not(feature = "mcp"))]
        Cmd::Mcp(_) => Err(CliError::McpLeftOut),
    }
}

#[cfg(test)]
mod tests;
