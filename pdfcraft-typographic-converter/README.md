# pdf-typographic-converter

A high-fidelity Rust binary and library that converts PDF documents into structured, editable Microsoft Word (`.docx`), Excel (`.xlsx`), and PowerPoint (`.pptx`) formats using Google PDFium and a typographic clustering engine.

## Features

- **Multi-Format Emitters**:
  - **Microsoft Word (`.docx`)**: Emits styled paragraphs and text runs with font names, sizes (in half-points), bold, italic, superscript/subscript vertical alignments, tabs, and page breaks using `docx-rs`.
  - **Microsoft Excel (`.xlsx`)**: Maps lines to rows and horizontal offsets to columns, with full font formatting (size, bold, italic, font scripts) using `rust_xlsxwriter`.
  - **Microsoft PowerPoint (`.pptx`)**: Generates valid OpenXML `.pptx` presentations using `zip::ZipWriter` with exact shape textboxes positioned in EMUs (flipped to top-down coordinates), character-level font properties, and superscript/subscript baseline shifts.
- **Typographic Layout & Clustering Engine**:
  - Clustered lines based on vertical baseline proximity (`|origin_y_a - origin_y_b| <= 2.0 pt`).
  - Median font size computation for line reference.
  - Script classification (`Normal`, `Superscript`, `Subscript`) according to font-size ratio and vertical baseline displacement.
  - Run segmentation on font family, size, weight, italic, and script changes.
  - Word space detection (`0.22 * font_size <= gap < 1.5 * font_size`) and tab stop detection (`gap >= 1.5 * font_size`).
  - Tracking delta calculation and enclosing bounding boxes (`(min_x, min_y, max_x, max_y)`).
  - Structured output models: `DocumentLayout`, `PageLayout`, `TextLine`, `TextRun`.
- **Character & Glyph Metrics**:
  - `char_code`: Unicode character
  - `font_name`: Extracted font name (with fallbacks to parent text object and descriptors)
  - `font_size`: Scaled/effective rendered font size
  - `is_bold`: Bold typeface detection via font weight, descriptor flags, and font family heuristics
  - `is_italic`: Italic/oblique detection via descriptor flags, italic angle, and font heuristics
  - `bounds`: Tuple of `(left, top, right, bottom)` coordinates in PDF space
  - `origin_x`, `origin_y`: Baseline origin coordinates
- **Robust Error Handling**:
  - Gracefully processes empty pages and pages with missing or partial font descriptors without crashing or panicking.
  - Formatted JSON output via `serde` and `serde_json`.
- **Flexible PDFium Initialization**:
  - Automatically checks local directories for `pdfium.dll` (Windows) / `libpdfium.so` (Linux) / `libpdfium.dylib` (macOS).
  - Supports `--pdfium-path <PATH>` CLI parameter and the `PDFIUM_PATH` environment variable.
  - Falls back to system PDFium if available in PATH.

## Installation & Build

```bash
cargo build --release
```

## CLI Usage

```bash
# Convert to all formats (.docx, .xlsx, .pptx) in current directory
cargo run -- <input_pdf>

# Specify output format (docx, xlsx, pptx, or all)
cargo run -- <input_pdf> --format docx
cargo run -- <input_pdf> --format xlsx
cargo run -- <input_pdf> --format pptx

# Specify custom output directory
cargo run -- <input_pdf> --out ./output_directory

# Specify custom PDFium library path
cargo run -- <input_pdf> --pdfium-path /path/to/pdfium.dll
```

## Example CLI Output

```text
Converting: sample.pdf
  -> Extracted raw glyphs in 18.01ms
  -> Built typographic layout in 209.50µs

=== Conversion Summary ===
Total Pages:          2
Total Lines:          5
Total Text Runs:      9
Detected Superscripts:1
Detected Subscripts:  1
Emitter Elapsed Time: 15.79ms
Total Elapsed Time:   36.92ms

Generated Files:
  - ./sample.docx
  - ./sample.xlsx
  - ./sample.pptx
  - ./layout_debug.json
```

## Running Tests

```bash
cargo test
```
