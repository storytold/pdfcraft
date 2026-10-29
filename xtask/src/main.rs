//! Project automation for PdfCraft, run via `cargo xtask <command>`.

use std::process::ExitCode;

mod assets;
mod corpus;
mod demo_pdf;
mod fuzz;
mod gates;
mod layers;
mod macos_integration;
mod mobile;
mod parity;
mod screenshots;
mod version;

type Command = fn(&[String]) -> anyhow::Result<()>;

/// Every subcommand: name, one-line summary, entry point.
const COMMANDS: &[(&str, &str, Command)] = &[
    ("version", "Print the workspace version, or `version set X.Y.Z[-pre]` to change it and refresh Cargo.lock", version_cmd),
    ("layers", "Enforce the crate dependency layering (plan/architecture.md §3)", gates::layers),
    ("wasm", "cargo check --target wasm32-unknown-unknown for every crate below L8", gates::wasm),
    ("deny", "Dependency licences, bans, sources and advisories (deny.toml; needs cargo-deny)", gates::deny),
    ("ci", "fmt --check, clippy -D warnings, test, layers, wasm, assets, deny, parity (stops at first failure)", gates::ci),
    ("assets", "Enforce the asset policy (AGENTS.md §1) against ATTRIBUTION.toml; --write regenerates ATTRIBUTION.md", assets::run),
    ("corpus", "Fetch the pinned pdf.js test corpus into corpus/ (git-ignored) and verify it; --update-pin re-records the pin", gates::corpus),
    ("check", "Robustness sweep over corpus/ with pdfcraft-cli; fails on crashes or regressions vs xtask/baselines", gates::check),
    ("fuzz", "Mutation fuzzing of open/render/edit/save in child processes; findings in fuzz-out/ (--time 300)", fuzz::run),
    ("parity", "Validate parity/acrobat-features.toml against the registry, tools and tests; report progress (--json, --partial)", parity::run),
    (
        "macos-integration",
        "Report macOS bundle, Quick Look, Spotlight and PDF integration diagnostics without mutating system state",
        macos_integration::run,
    ),
    ("text-oracle", "Compare text extraction with pdftotext over corpus/ (word F1; target median ≥ 0.97)", gates::text_oracle),
    (
        "render-oracle",
        "Compare page rendering with Poppler over corpus/ (pixel disagreement rate; --dir, --dpi, --limit, --tolerance, --hide-annotations)",
        gates::render_oracle,
    ),
    ("screenshots", "Regenerate the README screenshots in docs/images/ and their ATTRIBUTION entries", screenshots::run),
    ("models", "Fetch the OCR models (ATTRIBUTION.toml kind = \"model\") into assets/models/, verified by SHA-256", assets::models),
    ("demo-pdf", "Build dist/demo/pdfcraft-showcase.pdf (needs Google Chrome or Chromium)", demo_pdf::run),
    ("mobile-baseline", "Run mobile performance baseline (open time, tile render p50/p95, peak RSS)", mobile::baseline),
    ("mobile-bindings", "Generate Swift and Kotlin bindings for pdfcraft-ffi (--check diffs against api/*.api)", mobile::bindings),
    ("mobile-xcframework", "Build and package XCFramework for iOS device and simulator", mobile::xcframework),
    ("mobile-android", "Build and package Android .so native libraries (16KB aligned) into pdfcraft-kit", mobile::android),
    ("mobile-size", "Report binary footprint and per-feature cost of mobile libraries against budget B10", mobile::size),
    ("mobile-viewer-size", "Build viewer-only libpdfcraft_ffi.so for aarch64-linux-android and assert size <= ceiling (+2%)", mobile::viewer_size),
    ("mobile-push-corpus", "Push test corpus PDFs to Android device via adb", mobile::push_corpus),
    ("mobile-tile-check", "Compare tiles rendered on emulator against host render with 1 LSB tolerance", mobile::tile_check),
    ("mobile-soak", "Run Android emulator soak test and parse dumpsys meminfo/gfxinfo", mobile::soak),
];

fn version_cmd(args: &[String]) -> anyhow::Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    version::run(&gates::root(), &args).map_err(anyhow::Error::msg)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(name) = args.first() else {
        print_help();
        return ExitCode::FAILURE;
    };
    if matches!(name.as_str(), "help" | "-h" | "--help") {
        print_help();
        return ExitCode::SUCCESS;
    }
    let Some((_, _, command)) = COMMANDS.iter().find(|(n, _, _)| n == name) else {
        eprintln!("xtask: unknown command `{name}`\n");
        print_help();
        return ExitCode::FAILURE;
    };
    match command(&args[1..]) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("xtask {name}: error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    eprintln!("Usage: cargo xtask <command> [args]\n\nCommands:");
    for (name, about, _) in COMMANDS {
        eprintln!("  {name:<12} {about}");
    }
    eprintln!("  {:<12} Show this list", "help");
}
