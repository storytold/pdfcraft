# Invoice draft foundation

> **Last reviewed:** 2026-10-23 · **Last updated:** 2026-10-23 · **Change:** major (initial draft arithmetic API and implementation roadmap)

PdfCraft can calculate a small, explicitly supported invoice draft through the
engine API and `invoice_calculate`. This prepares the shared data and arithmetic
for a future invoice editor and exporters. It does **not** yet create an
electronic invoice or validate EN 16931, XRechnung or ZUGFeRD.

## Headless use

Use the ordinary CLI tool entry point or MCP with this tool and argument:

```sh
pdfcraft-cli run --script invoice-steps.json
```

```json
[{"tool": "invoice_calculate", "args": {
  "draft": {
    "number": "RE-2026-001",
    "issue_date": "2026-10-23",
    "currency": "EUR",
    "seller": { "name": "Example Seller", "country_code": "DE" },
    "buyer": { "name": "Example Buyer", "country_code": "DE" },
    "lines": [
      { "description": "Service", "quantity": "2.5", "unit_price": "12.3456", "vat_rate": "19" }
    ]
  }
}}]
```

The result contains `scope: "draft_arithmetic_only"`,
`standards_validation: "not_performed"`, `issues` and `totals`. This example's net,
VAT and payable amounts are respectively `"30.86"`, `"5.86"` and `"36.72"`.
The tool never opens a PDF, reads a file, writes output itself or changes history.
CLI script execution and MCP use the same tool definition.

The engine re-exports `pdfcraft_invoice` as `pdfcraft_engine::invoice`;
`invoice::calculate(&Draft)` uses the same implementation as the tool.

## Contract and limits

- Numeric inputs are strings of ASCII digits with an optional dot. At most twelve
  integral and six fractional digits are accepted. JSON numbers, signs,
  scientific notation, spaces and decimal commas are refused. Output money is
  always a string with two fractional digits, avoiding client floating-point loss.
- Inputs currently support only EUR and seller/buyer country DE. Party names are
  minimal draft identifiers, not complete legal invoice addresses.
- Quantity must be positive; unit price may be zero. VAT must be above zero and
  at most 100, with no more than two significant fractional places. This checks
  arithmetic support, not whether a rate is legally applicable to a transaction.
- Each quantity × unit-price product is rounded half-up to cents. Rounded line
  nets are summed by VAT rate. VAT is calculated and rounded half-up once per
  group. Payable is net plus the sum of those tax groups. No floating point is used.
- Between 1 and 1000 lines are supported. Text fields are capped at 1024 UTF-8
  bytes and cannot contain control characters. Dates must be actual calendar
  dates in `YYYY-MM-DD` form, in years 0001–9999.
- Unknown fields at every model level are refused. Do not remove unsupported
  discounts, exemptions or credits from a real invoice merely to make it fit.
- The tool bounds the JSON tree before deserialization: 1000 entries per array,
  10,000 value nodes, eight levels, 32 fields per object, 64 bytes per field name,
  and 1024 bytes per string. These limits apply to unknown fields too. JSON
  string limits may therefore reject malformed input before a field report exists.
- Syntax/type errors are tool errors. Supported-shaped drafts with semantic
  errors return field paths, local diagnostic codes and `totals: null`.
  Paths such as `lines[0].quantity` are **zero-based JSON indexes**, not PDF page
  numbers. No standard business-rule identifiers are claimed.

The API is an initial draft contract, not a complete invoice semantic model.
Tax treatment, addresses, identifiers, currencies, allowances, credits, payment
details and service periods need explicit extensions before standards export.

## Implementation sequence

1. Extend the semantic model and exact calculations with supported business cases,
   preserving explicit refusal for unsupported cases and unknown imported fields.
2. Implement standalone CII/XRechnung output and validate with a pinned independent
   KoSIT scenario. Decide the native/web product validation strategy.
3. Generate the visual PDF and embedded XML from one reconciled snapshot. Add
   PDF/A-3, associated-file and invoice XMP packaging; independently check the
   resulting hybrid using veraPDF and an invoice validator.
4. Add the native invoice editor, local drafts, preview and safe editing workflow.
   Existing PDF edits or sanitizing must not silently leave stale/remove invoice XML.
5. Extend to UBL, Peppol BIS files and other profiles. Actual Peppol transmission,
   arbitrary-PDF AI/OCR ingestion and accounting/archive services are separate work.

Each stage needs engine/headless coverage and honest parity evidence before being
presented as shipped. No full-standard conformance status follows from this API.

## Provenance and intellectual-property boundaries

This change contains original Rust model, bounded decimal parsing and arithmetic,
using existing serde/chrono dependencies. No PDF24 code, visual layout, screenshot,
logo, font, invoice library, official validation artifacts or example documents are
included. The public PDF24 tool was a functional reference only.

The future format work must track specification versions, source provenance,
copyright notices, dependency licenses and any declared patent terms separately.
Public access and open-source licensing alone do not establish patent freedom to
operate. This PR makes no patent-clearance claim. A jurisdiction/claim-specific
assessment remains separate from software verification; identified risks must be
resolved before the affected implementation is adopted.

In particular, current CEN validation artifacts identify EUPL-1.2, outside the
repository's asset allowlist. Do not assume that an Apache-licensed validation
engine clears the licenses of bundled rules. No such artifacts are introduced.
The European Commission's standard-access page also describes CEN derivative-use
notices and the 2017-to-2026 transition; review the applicable terms before
implementing or distributing the complete standard-derived model or formats.

Sources reviewed on 2026-10-23:

- [European Commission: standard access and derivative use](https://ec.europa.eu/digital-building-blocks/sites/spaces/DIGITAL/pages/467108971/Obtaining%2Ba%2Bcopy%2Bof%2Bthe%2BEuropean%2Bstandard%2Bon%2BeInvoicing)
- [KoSIT: production XRechnung bundles and preview versions](https://xeinkauf.de/xrechnung/versionen-und-bundles/)
- [FeRD: ZUGFeRD 2.5.2 information](https://www.ferd-net.de/publikationen-produkte/publikationen/detailseite/zugferd-252-deutsch)
- [CEN validation artifacts and license](https://github.com/ConnectingEurope/eInvoicing-EN16931)
- [Swiss IPI: scope of freedom-to-operate searches](https://www.ige.ch/en/ip-search/searches/freedom-to-operate-search)

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-23 | major | Add bounded draft arithmetic and document the remaining e-invoicing work and provenance boundaries. |
