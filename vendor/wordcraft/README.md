# Embedded WordCraft document conversion

The local library crates and bundled fonts come from storytold/wordcraft commit `bf2be3cad86d6c6a72d604676ab6ff3f0aa9478d`. The workspace uses PDFCraft's craft-libs revision. Update the pin with PDFCraft's root manifest and preserve the upstream notices and font licenses.

Desktop conversion uses WordCraft's byte-based open and PDF save APIs with their default layout and export settings. WordCraft recognizes DOC/DOT, DOCX/DOCM/DOTX/DOTM, RTF, ODT/OTT/FODT, Word 2003 XML and Flat OPC XML, HTML/HTM/XHTML, MHT/MHTML, MD/Markdown, TeX/LaTeX/LTX, plain text and its JSON document model (including .wcraft.json). PDFCraft retains its direct PDF, image and plain-text handlers. Office documents open as new PDFs. The desktop worker can request a password for an encrypted Word file.

The converter consumes supplied bytes. Standalone HTML and Markdown should use embedded resources for portable pictures.
