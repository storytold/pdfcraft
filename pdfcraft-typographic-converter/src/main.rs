use pdf_typographic_converter::{
    emitters::{export_docx, export_pptx, export_xlsx},
    extract, layout, ScriptType,
};

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Docx,
    Xlsx,
    Pptx,
    All,
}

impl OutputFormat {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "docx" => Some(Self::Docx),
            "xlsx" => Some(Self::Xlsx),
            "pptx" => Some(Self::Pptx),
            "all" => Some(Self::All),
            _ => None,
        }
    }
}

struct CliArgs {
    input_file: PathBuf,
    custom_pdfium_path: Option<PathBuf>,
    output_dir: PathBuf,
    output_format: OutputFormat,
    debug_json: bool,
}

fn print_usage(program_name: &str) {
    eprintln!("Usage: {} <INPUT_PDF> [OPTIONS]", program_name);
    eprintln!();
    eprintln!("Convert PDF documents to editable Microsoft Word (.docx), Excel (.xlsx), and PowerPoint (.pptx) formats.");
    eprintln!();
    eprintln!("Arguments:");
    eprintln!("  <INPUT_PDF>              Path to the input PDF file");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --format <FMT>           Output format: docx, xlsx, pptx, or all (default: all)");
    eprintln!("  --out <DIR>              Target output directory (default: .)");
    eprintln!("  --pdfium-path <PATH>     Explicit path to the PDFium dynamic library (pdfium.dll / libpdfium.so / libpdfium.dylib)");
    eprintln!("  --debug-json             Emit layout debug JSON file");
    eprintln!("  -h, --help               Print help information");
}

fn parse_format_value(fmt_str: &str) -> OutputFormat {
    OutputFormat::from_str(fmt_str).unwrap_or_else(|| {
        eprintln!("Error: Invalid format '{}'. Supported values are: docx, xlsx, pptx, all.", fmt_str);
        process::exit(1);
    })
}

fn take_next_arg<'a>(args: &'a [String], index: &mut usize, flag_name: &str) -> &'a str {
    *index += 1;
    args.get(*index).map(String::as_str).unwrap_or_else(|| {
        eprintln!("Error: {flag_name} requires an argument.");
        process::exit(1);
    })
}

fn parse_args(args: &[String], program_name: &str) -> CliArgs {
    if args.len() < 2 {
        print_usage(program_name);
        process::exit(1);
    }

    let mut pdf_path: Option<PathBuf> = None;
    let mut custom_pdfium_path: Option<PathBuf> = None;
    let mut output_dir = PathBuf::from(".");
    let mut output_format = OutputFormat::All;
    let mut debug_json = false;

    let mut i = 1;
    while i < args.len() {
        let arg = args[i].as_str();
        if matches!(arg, "-h" | "--help") {
            print_usage(program_name);
            process::exit(0);
        } else if arg == "--debug-json" {
            debug_json = true;
        } else if arg == "--format" {
            output_format = parse_format_value(take_next_arg(args, &mut i, "--format"));
        } else if let Some(fmt_str) = arg.strip_prefix("--format=") {
            output_format = parse_format_value(fmt_str);
        } else if arg == "--out" {
            output_dir = PathBuf::from(take_next_arg(args, &mut i, "--out"));
        } else if let Some(dir_str) = arg.strip_prefix("--out=") {
            output_dir = PathBuf::from(dir_str);
        } else if arg == "--pdfium-path" {
            custom_pdfium_path = Some(PathBuf::from(take_next_arg(args, &mut i, "--pdfium-path")));
        } else if let Some(path_str) = arg.strip_prefix("--pdfium-path=") {
            custom_pdfium_path = Some(PathBuf::from(path_str));
        } else if arg.starts_with('-') {
            eprintln!("Error: Unknown option '{}'.", arg);
            print_usage(program_name);
            process::exit(1);
        } else if let Some(existing) = &pdf_path {
            eprintln!("Error: Multiple file paths specified ('{}' and '{}').", existing.display(), arg);
            process::exit(1);
        } else {
            pdf_path = Some(PathBuf::from(arg));
        }
        i += 1;
    }

    let input_file = pdf_path.unwrap_or_else(|| {
        eprintln!("Error: Missing input PDF file path.");
        print_usage(program_name);
        process::exit(1);
    });

    CliArgs { input_file, custom_pdfium_path, output_dir, output_format, debug_json }
}

fn emit_format(
    enabled: bool,
    path: PathBuf,
    label: &str,
    layout: &layout::DocumentLayout,
    exporter: fn(&layout::DocumentLayout, &Path) -> anyhow::Result<()>,
    generated_files: &mut Vec<PathBuf>,
) {
    if !enabled {
        return;
    }
    if let Err(err) = exporter(layout, &path) {
        eprintln!("Error exporting {label}: {err:#}");
        process::exit(1);
    }
    generated_files.push(path);
}

fn emit_outputs(layout: &layout::DocumentLayout, cli: &CliArgs, file_stem: &str) -> Vec<PathBuf> {
    let mut generated_files = Vec::new();

    if cli.debug_json {
        let debug_path = cli.output_dir.join(format!("{file_stem}_layout_debug.json"));
        if let Ok(json_str) = serde_json::to_string_pretty(layout) {
            let _ = fs::write(&debug_path, &json_str);
            generated_files.push(debug_path);
        }
    }

    emit_format(
        matches!(cli.output_format, OutputFormat::Docx | OutputFormat::All),
        cli.output_dir.join(format!("{file_stem}.docx")),
        "DOCX",
        layout,
        export_docx,
        &mut generated_files,
    );
    emit_format(
        matches!(cli.output_format, OutputFormat::Xlsx | OutputFormat::All),
        cli.output_dir.join(format!("{file_stem}.xlsx")),
        "XLSX",
        layout,
        export_xlsx,
        &mut generated_files,
    );
    emit_format(
        matches!(cli.output_format, OutputFormat::Pptx | OutputFormat::All),
        cli.output_dir.join(format!("{file_stem}.pptx")),
        "PPTX",
        layout,
        export_pptx,
        &mut generated_files,
    );

    generated_files
}

fn main() {
    let start_time = Instant::now();
    let args: Vec<String> = env::args().collect();
    let program_name = args.first().map(String::as_str).unwrap_or("pdf-typographic-converter");
    let cli = parse_args(&args, program_name);

    if !cli.input_file.exists() {
        eprintln!("Error: File not found: '{}'", cli.input_file.display());
        process::exit(1);
    }

    if !cli.output_dir.exists() {
        if let Err(err) = fs::create_dir_all(&cli.output_dir) {
            eprintln!("Error: Failed to create output directory '{}': {:#}", cli.output_dir.display(), err);
            process::exit(1);
        }
    }

    let file_stem = cli.input_file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "document".to_string());

    println!("Converting: {}", cli.input_file.display());

    let t_init = Instant::now();
    let pdfium = match extract::init_pdfium_with_path(cli.custom_pdfium_path.as_deref()) {
        Ok(instance) => instance,
        Err(err) => {
            eprintln!("Error initializing PDFium: {:#}", err);
            process::exit(1);
        }
    };

    let page_data = match extract::extract_pages_full_with_pdfium(&pdfium, &cli.input_file) {
        Ok(pages) => pages,
        Err(err) => {
            eprintln!("Error extracting page data from '{}': {:#}", cli.input_file.display(), err);
            process::exit(1);
        }
    };
    let extract_duration = t_init.elapsed();
    println!("  -> Extracted raw page data in {:.2?}", extract_duration);

    let total_skipped_images: usize = page_data.iter().map(|p| p.skipped_images).sum();
    if total_skipped_images > 0 {
        eprintln!("Warning: skipped {total_skipped_images} image(s) exceeding the 128 MiB per-page image extraction budget.");
    }
    let t_layout = Instant::now();
    let layout = layout::build_layout(page_data);
    let layout_duration = t_layout.elapsed();
    println!("  -> Built typographic layout in {:.2?}", layout_duration);

    let total_pages = layout.pages.len();
    let total_lines: usize = layout.pages.iter().map(|p| p.lines.len()).sum();
    let total_runs: usize = layout.pages.iter().flat_map(|p| &p.lines).map(|l| l.runs.len()).sum();
    let total_superscripts: usize =
        layout.pages.iter().flat_map(|p| &p.lines).flat_map(|l| &l.runs).filter(|r| r.script == ScriptType::Superscript).count();
    let total_subscripts: usize =
        layout.pages.iter().flat_map(|p| &p.lines).flat_map(|l| &l.runs).filter(|r| r.script == ScriptType::Subscript).count();

    let t_emit = Instant::now();
    let generated_files = emit_outputs(&layout, &cli, &file_stem);
    let emit_duration = t_emit.elapsed();
    let total_duration = start_time.elapsed();

    println!();
    println!("=== Conversion Summary ===");
    println!("Total Pages:          {}", total_pages);
    println!("Total Lines:          {}", total_lines);
    println!("Total Text Runs:      {}", total_runs);
    println!("Detected Superscripts:{}", total_superscripts);
    println!("Detected Subscripts:  {}", total_subscripts);
    println!("Emitter Elapsed Time: {:.2?}", emit_duration);
    println!("Total Elapsed Time:   {:.2?}", total_duration);
    println!();
    println!("Generated Files:");
    for f in &generated_files {
        println!("  - {}", f.display());
    }
}
