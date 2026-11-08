use pdf_typographic_converter::{
    emitters::{export_docx, export_pptx, export_xlsx},
    extract, layout, ScriptType,
};

use std::env;
use std::fs;
use std::path::PathBuf;
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
    eprintln!("  -h, --help               Print help information");
}

fn main() {
    let start_time = Instant::now();
    let args: Vec<String> = env::args().collect();
    let program_name = args.first().map(String::as_str).unwrap_or("pdf-typographic-converter");

    if args.len() < 2 {
        print_usage(program_name);
        process::exit(1);
    }

    let mut pdf_path: Option<PathBuf> = None;
    let mut custom_pdfium_path: Option<PathBuf> = None;
    let mut output_dir = PathBuf::from(".");
    let mut output_format = OutputFormat::All;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_usage(program_name);
                process::exit(0);
            }
            "--format" => {
                i += 1;
                if i < args.len() {
                    match OutputFormat::from_str(&args[i]) {
                        Some(fmt) => output_format = fmt,
                        None => {
                            eprintln!("Error: Invalid format '{}'. Supported values are: docx, xlsx, pptx, all.", args[i]);
                            process::exit(1);
                        }
                    }
                } else {
                    eprintln!("Error: --format requires an argument (docx, xlsx, pptx, all).");
                    process::exit(1);
                }
            }
            arg if arg.starts_with("--format=") => {
                let fmt_str = &arg["--format=".len()..];
                match OutputFormat::from_str(fmt_str) {
                    Some(fmt) => output_format = fmt,
                    None => {
                        eprintln!("Error: Invalid format '{}'. Supported values are: docx, xlsx, pptx, all.", fmt_str);
                        process::exit(1);
                    }
                }
            }
            "--out" => {
                i += 1;
                if i < args.len() {
                    output_dir = PathBuf::from(&args[i]);
                } else {
                    eprintln!("Error: --out requires a directory path argument.");
                    process::exit(1);
                }
            }
            arg if arg.starts_with("--out=") => {
                let dir_str = &arg["--out=".len()..];
                output_dir = PathBuf::from(dir_str);
            }
            "--pdfium-path" => {
                i += 1;
                if i < args.len() {
                    custom_pdfium_path = Some(PathBuf::from(&args[i]));
                } else {
                    eprintln!("Error: --pdfium-path requires a path argument.");
                    process::exit(1);
                }
            }
            arg if arg.starts_with("--pdfium-path=") => {
                let path_str = &arg["--pdfium-path=".len()..];
                custom_pdfium_path = Some(PathBuf::from(path_str));
            }
            arg if arg.starts_with('-') => {
                eprintln!("Error: Unknown option '{}'.", arg);
                print_usage(program_name);
                process::exit(1);
            }
            arg => {
                if let Some(existing) = &pdf_path {
                    eprintln!(
                        "Error: Multiple file paths specified ('{}' and '{}').",
                        existing.display(),
                        arg
                    );
                    process::exit(1);
                } else {
                    pdf_path = Some(PathBuf::from(arg));
                }
            }
        }
        i += 1;
    }

    let input_file = match pdf_path {
        Some(path) => path,
        None => {
            eprintln!("Error: Missing input PDF file path.");
            print_usage(program_name);
            process::exit(1);
        }
    };

    if !input_file.exists() {
        eprintln!("Error: File not found: '{}'", input_file.display());
        process::exit(1);
    }

    // Ensure output directory exists
    if !output_dir.exists() {
        if let Err(err) = fs::create_dir_all(&output_dir) {
            eprintln!(
                "Error: Failed to create output directory '{}': {:#}",
                output_dir.display(),
                err
            );
            process::exit(1);
        }
    }

    let file_stem = input_file
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "document".to_string());

    println!("Converting: {}", input_file.display());

    // 1. Initialize PDFium
    let t_init = Instant::now();
    let pdfium = match extract::init_pdfium_with_path(custom_pdfium_path.as_deref()) {
        Ok(instance) => instance,
        Err(err) => {
            eprintln!("Error initializing PDFium: {:#}", err);
            process::exit(1);
        }
    };

    // 2. Extract glyphs grouped by page
    let page_glyphs = match extract::extract_pages_with_pdfium(&pdfium, &input_file) {
        Ok(pages) => pages,
        Err(err) => {
            eprintln!(
                "Error extracting glyphs from '{}': {:#}",
                input_file.display(),
                err
            );
            process::exit(1);
        }
    };
    let extract_duration = t_init.elapsed();
    println!("  -> Extracted raw glyphs in {:.2?}", extract_duration);

    // 3. Build document layout
    let t_layout = Instant::now();
    let layout = layout::build_layout(page_glyphs);
    let layout_duration = t_layout.elapsed();
    println!("  -> Built typographic layout in {:.2?}", layout_duration);

    // Calculate layout statistics
    let total_pages = layout.pages.len();
    let total_lines: usize = layout.pages.iter().map(|p| p.lines.len()).sum();
    let total_runs: usize = layout
        .pages
        .iter()
        .flat_map(|p| &p.lines)
        .map(|l| l.runs.len())
        .sum();

    let total_superscripts: usize = layout
        .pages
        .iter()
        .flat_map(|p| &p.lines)
        .flat_map(|l| &l.runs)
        .filter(|r| r.script == ScriptType::Superscript)
        .count();

    let total_subscripts: usize = layout
        .pages
        .iter()
        .flat_map(|p| &p.lines)
        .flat_map(|l| &l.runs)
        .filter(|r| r.script == ScriptType::Subscript)
        .count();

    // Save layout_debug.json
    let debug_path = output_dir.join(format!("{}_layout_debug.json", file_stem));
    // Also save standard layout_debug.json in current directory or output directory
    let default_debug_path = output_dir.join("layout_debug.json");
    if let Ok(json_str) = serde_json::to_string_pretty(&layout) {
        let _ = fs::write(&default_debug_path, &json_str);
        if debug_path != default_debug_path {
            let _ = fs::write(&debug_path, &json_str);
        }
    }

    // 4. Emit requested formats
    let t_emit = Instant::now();
    let mut generated_files = Vec::new();

    if output_format == OutputFormat::Docx || output_format == OutputFormat::All {
        let docx_path = output_dir.join(format!("{}.docx", file_stem));
        if let Err(err) = export_docx(&layout, &docx_path) {
            eprintln!("Error exporting DOCX: {:#}", err);
            process::exit(1);
        }
        generated_files.push(docx_path);
    }

    if output_format == OutputFormat::Xlsx || output_format == OutputFormat::All {
        let xlsx_path = output_dir.join(format!("{}.xlsx", file_stem));
        if let Err(err) = export_xlsx(&layout, &xlsx_path) {
            eprintln!("Error exporting XLSX: {:#}", err);
            process::exit(1);
        }
        generated_files.push(xlsx_path);
    }

    if output_format == OutputFormat::Pptx || output_format == OutputFormat::All {
        let pptx_path = output_dir.join(format!("{}.pptx", file_stem));
        if let Err(err) = export_pptx(&layout, &pptx_path) {
            eprintln!("Error exporting PPTX: {:#}", err);
            process::exit(1);
        }
        generated_files.push(pptx_path);
    }

    let emit_duration = t_emit.elapsed();
    let total_duration = start_time.elapsed();

    // 5. Print summary
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
    println!("  - {}", default_debug_path.display());
}
