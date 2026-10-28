# pdfcraft-invoice

L4 invoice draft arithmetic, independent of PDFs, the filesystem and UI toolkits.
Exposed to frontends as `pdfcraft_engine::invoice` and through the read-only
`invoice_calculate` CLI/MCP tool. See [invoice drafts](../../docs/invoice-drafts.md).

`Draft` and `Decimal` accept strict serde inputs, reject unknown fields, and use
checked integer arithmetic. `calculate(&Draft)` returns field issues and optional
EUR totals. Draft strings, line count and numeric precision are bounded; the tool
also bounds the JSON tree before copying/deserializing it. Dates use chrono.

This initial subset supports DE parties, EUR, positive quantities, non-negative
unit prices and positive standard-rated VAT. It does not implement exemptions,
reverse charge, credit notes, adjustments, XML, PDF generation or conformance
validation. No official Schematron, XSD, code lists or invoice implementation
have been copied into the crate. It adds no new third-party package to the lockfile.

Run `cargo test -p pdfcraft-invoice -j 2` and
`cargo test -p pdfcraft-automation -j 2 invoice::`.
