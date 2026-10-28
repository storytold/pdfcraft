# Enhancement Proposal: Robust Math & Symbol Font Mapping (#3519)

## Context & User Problem
In technical, mathematical, and academic documents (such as the Boolean logic and circuit assignment shown in the user's screenshot), formulas such as $f = (A+B+C)(A+\bar{B}+C)$ frequently fail to render operators (`+`, `-`, parentheses, Greek letters) correctly. Upstream issue #3519 documents this exact behavior:
> *"PDF craft cannot show maths symbols +, -, 1st image is how it should look and 2nd is how its looking in PDF craft (+) brackets not visible, symbols like alpha are also turned into small a"*

## Root Cause
When PDF files encode mathematical symbols using Symbol fonts, Type 3 font dictionaries, or custom embedded subset fonts without an explicit `/ToUnicode` mapping stream:
1. The renderer and text extraction pipeline default to standard WinAnsi / Latin-1 encoding tables.
2. Under Latin-1, character codes used by standard math fonts (e.g. Adobe Symbol font or Computer Modern Math) map to non-printable control characters, accented letters, or missing glyph boxes.
3. In `crates/fonts/src/std14_metrics.rs`, Symbol and ZapfDingbats metrics exist, but the encoding lookup fails to fallback to the Symbol Encoding table when `/Encoding` is missing or specified as a standard name.

## Architectural Solution & Variation
1. **Symbol Encoding Table in `crates/fonts`:**
   Add a standard Symbol encoding translation table (mapping character codes `0x20`–`0xFF` to Unicode codepoints `U+0391`–`U+03C9` for Greek, `U+2200`–`U+22FF` for mathematical operators like `\forall`, `\exists`, `\times`, `+`, `-`, and brackets).
2. **Fallback Detection in `pdfcraft-render` / `pdfcraft-content`:**
   If a font's `/BaseFont` contains `Symbol`, `Math`, `CMSY`, `CMMI`, or `/FontDescriptor /Flags` indicates symbolic font (bit 3), map character codes via the Symbol translation table before falling back to Latin-1.
3. **Clean-Room License Compliance:**
   Use the open Unicode consortium mapping table (`SYMBOL.TXT`, public domain) already referenced in standard font repositories, fully documented in `ATTRIBUTION.toml`.
