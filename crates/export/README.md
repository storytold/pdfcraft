# pdfcraft-export

Layer L4 writers for Word (DOCX), HTML, Rich Text Format (RTF), and PowerPoint (PPTX).
The engine supplies each page's text blocks, images, dimensions, and background.
The writers infer paragraphs, headings, and tables from that content.

RTF preserves text size, bold, italic, color, detected tables, and page breaks.
Hard line endings inside text are written as `\line` controls, preserving their paragraph or table cell.
LF and CR each produce one hard break, and a CRLF pair produces one break.
The text encoder retains repeated, leading, and trailing line endings in the strings it receives, including direct encoder input.
PDF paragraph inference trims boundary CR/LF before Office export; preserving those boundary endings remains an extraction limitation.
Internal PDF line endings retained by inference reach the encoder and are covered by the automation regression.
RTF syntax characters and Unicode text keep their existing escaping.

RTF leaves images out, and layout and reading-order fidelity remain limited by the inferred blocks.
This hard-break handling applies to RTF; the other writers retain their existing behavior.
