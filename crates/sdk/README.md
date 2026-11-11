# pdfcraft-sdk

High-performance, in-process Rust SDK and C-ABI/FFI bindings for PDF document processing operations, providing an open-source, memory-safe (`#![forbid(unsafe_code)]`) alternative to Adobe PDF Library (APDFL) and Datalogics APDFL.

> [!NOTE]
> **Part of PdfCraft Enterprise Extensions ([PR #919](https://github.com/storytold/pdfcraft/pull/919), RFC [#875](https://github.com/storytold/pdfcraft/issues/875))**
> See the [Main README](../../README.md#⚡-fork-overview-enterprise-sdk--rest-api) for the full ecosystem overview and interactive diagram.

---

## 🏛️ Architecture & Ports-and-Adapters Boundary

`pdfcraft-sdk` is an **L7** crate adhering to strict Hexagonal Architecture (Ports and Adapters):

```text
  ┌─────────────────────────────────────────────────────────────┐
  │                 Application / Caller Code                   │
  │            (Rust backend, PyO3, Node N-API)                 │
  └──────────────────────────────┬──────────────────────────────┘
                                 │
                                 ▼
  ┌─────────────────────────────────────────────────────────────┐
  │         pdfcraft_sdk::ports::PdfCraftClient (Port)          │
  │                  LocalPdfClient (Adapter)                   │
  └──────────────────────────────┬──────────────────────────────┘
                                 │
                                 ▼
  ┌─────────────────────────────────────────────────────────────┐
  │      pdfcraft_sdk::ports::OperationExecutor (Port)          │
  │             AutomationEngineAdapter (Adapter)               │
  └──────────────────────────────┬──────────────────────────────┘
                                 │
                                 ▼
  ┌─────────────────────────────────────────────────────────────┐
  │           pdfcraft-automation & pdfcraft-engine             │
  │                    (L7 / L6 Engine)                         │
  └─────────────────────────────────────────────────────────────┘
```

- **Hexagonal Decoupling:** The `OperationExecutor` port decouples operations from the underlying engine, enabling zero-engine mock test harnesses and independent lifecycle verification.
- **Panic Containment:** Every untrusted document parse and tool dispatch sits behind `std::panic::catch_unwind` guards to ensure foreign inputs never bring down the host process.
- **Resource Limits & Spillover:** Automatic disk spillover for large files (> 10 MB) prevents unbounded heap consumption.
- **Safe by Default:** `#![forbid(unsafe_code)]` with zero `unsafe` blocks.

---

## 🛡️ Astra Staff Architect Audit Hardening (Score: 8/10)

- **Security & Containment (8/10):** Local paths validated strictly against traversal (`..`), null bytes, and sandbox escapes.
- **Resource Governance (7/10):** Bounded streaming copy with 1-byte lookahead probe; aggregate input quotas covering auxiliary files; bookmark pagination cap (5,000 nodes total).
- **Correctness & Failure Semantics (8/10):** Fail-closed split delivery; strictly advancing pagination offsets (`next > offset`).
- **Separation of Concerns (8/10):** OperationExecutor port decoupled from AutomationEngineAdapter.
- **Test Quality (8/10):** Comprehensive unit, integration, and mutation tests covering 76+ deterministic scenarios.

---

## 🚀 Quickstart

Add `pdfcraft-sdk` to your `Cargo.toml`:

```toml
[dependencies]
pdfcraft-sdk = { path = "crates/sdk" }
```

### Document Merge & Page Preview
```rust
use pdfcraft_sdk::{DocumentSource, LocalPdfClient, PdfClient};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = LocalPdfClient::new()?;

    // Merge multiple PDF sources
    let merged = client.merge(vec![
        DocumentSource::from_path("cover.pdf")?,
        DocumentSource::from_path("content.pdf")?,
    ])?;
    std::fs::write("book.pdf", merged.into_bytes()?)?;

    // Render high-DPI page preview
    let preview = client.render_page_preview(
        DocumentSource::from_path("book.pdf")?,
        1,   // Page 1
        1024 // Max dimension (px)
    )?;
    std::fs::write("page_1.png", preview.png_bytes())?;

    Ok(())
}
```

---

## 📚 Related Documentation

- **[Multi-Language SDK Overview](../../sdks/README.md)**: Python & TypeScript packages.
- **[Main Project README](../../README.md#⚡-fork-overview-enterprise-sdk--rest-api)**: Executive overview and architecture comparison.
- **[REST Service Crate](../rest/README.md)**: Self-hostable HTTP microservice powered by `pdfcraft-sdk`.
- **[Canonical SDK Developer Guide](../../docs/sdk/README.md)**: Concurrency models, safety barriers, and deployment patterns.
- **[Domain-by-Domain API Reference](../../docs/sdk/api-reference.md)**: Full API parameters, schemas, and signatures.
- **[Control Planes Blueprint](../../docs/sdk/control-planes.md)**: Architectural comparison of MCP, SDK, and REST.
