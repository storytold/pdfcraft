# PdfCraft Multi-Language SDKs & Enterprise Services

This directory contains client libraries, native bindings, and API specifications for integrating **PdfCraft** programmatically into enterprise document pipelines, microservices, and foreign-language application runtimes.

> [!NOTE]
> **Fork Context:** Upstream [`storytold/pdfcraft`](https://github.com/storytold/pdfcraft) is designed as a standalone desktop PDF reader/editor GUI and interactive CLI. This fork introduces the headless, programmable SDK and REST microservice layer originally designed for upstream issue `#872` (RFC `#875`, PR `#919`, immutable tag `archive/upstream-pr-919`).

---

## 🌟 What This Fork Adds

| Component | Upstream `storytold/pdfcraft` | This Fork (`richfrem/pdfcraft`) |
| :--- | :--- | :--- |
| **Rust SDK (`L7`)** | None | `pdfcraft-sdk`: strongly-typed document builder, page ops, tools adapter |
| **HTTP REST Service (`L7`)** | None | `pdfcraft-rest`: Tokio/Axum microservice, async jobs, bearer auth, RFC 7807 |
| **Python Bindings** | None | `pdfcraft`: native PyO3 wheels + async REST client |
| **TypeScript / Node.js** | None | `@pdfcraft/sdk`: native N-API addon + typed REST client |
| **OpenAPI 3.1 Spec** | None | `sdks/openapi.json`: full HTTP contract with auto drift verification |
| **Contract Drift CI** | None | `sdks/check_contract_drift.py` ensuring zero drift across all 4 ecosystems |
| **Architecture Guides** | Desktop/UI docs | Control planes, token economics, determinism, and runnable examples |

---

## 📦 Language Packages

| Package | Language | Runtime / Bindings | Directory | Description |
| :--- | :--- | :--- | :--- | :--- |
| `pdfcraft-sdk` | Rust | In-process native crate (`L7`) | [`crates/sdk`](../crates/sdk) | Zero-overhead programmatic Rust API over the automation engine |
| `pdfcraft-rest` | Rust | HTTP REST daemon & client (`L7`) | [`crates/rest`](../crates/rest) | Tokio & Axum REST microservice with job queue and RFC 7807 errors |
| `pdfcraft` | Python | PyO3 native extension + HTTP REST | [`sdks/python`](python) | Dual-mode Python package: native C-extension speed or REST client |
| `@pdfcraft/sdk` | TypeScript / Node.js | N-API native addon + typed HTTP REST | [`sdks/typescript`](typescript) | Dual-mode Node.js client: native bindings or typed HTTP REST client |

---

## 🏛️ Dual Control Plane Architecture

The fork provides two interoperable control planes depending on your deployment requirements:

```
┌────────────────────────────────────────────────────────┐
│            Foreign Language Applications               │
│         (Python, TypeScript/Node, Go, etc.)            │
└───────────────┬────────────────────────┬───────────────┘
                │                        │
       In-Process Native           HTTP/1.1 REST
      (PyO3 / Node N-API)        (Bearer Auth + JSON)
                │                        │
                ▼                        ▼
      ┌──────────────────┐     ┌──────────────────┐
      │   pdfcraft-sdk   │     │  pdfcraft-rest   │
      │   (Crate L7)     │     │   (Crate L7)     │
      └─────────┬────────┘     └─────────┬────────┘
                │                        │
                └───────────┬────────────┘
                            ▼
               ┌────────────────────────┐
               │  pdfcraft-automation   │
               │   (Engine Core L7)     │
               └────────────┬───────────┘
                            ▼
               ┌────────────────────────┐
               │    pdfcraft-engine     │
               │       (Crate L6)       │
               └────────────────────────┘
```

1. **In-Process Native Control Plane (`pdfcraft-sdk`):**
   - Direct linking in the same OS process.
   - Lowest latency, zero serialization overhead, zero network round-trips.
   - Ideal for Rust backend services and high-performance Python/Node batch jobs.

2. **Network / Microservice Control Plane (`pdfcraft-rest`):**
   - Decoupled HTTP/1.1 REST service.
   - Fault-isolated: process crashes or heavy memory allocations do not destabilize the host application.
   - Built-in asynchronous job queue (`/v1/jobs`) for long-running batch operations with progress polling.
   - Standardized error reporting with RFC 7807 Problem Details.

---

## 🚀 Quickstart Guides

### 1. Rust SDK (`crates/sdk`)

In `Cargo.toml`:
```toml
[dependencies]
pdfcraft-sdk = { path = "path/to/pdfcraft/crates/sdk" }
```

In your code:
```rust
use pdfcraft_sdk::{DocumentSource, LocalPdfClient, PdfClient};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = LocalPdfClient::new()?;

    // Merge multiple PDF sources
    let merged = client.merge(vec![
        DocumentSource::from_path("cover.pdf")?,
        DocumentSource::from_path("content.pdf")?,
    ])?;

    std::fs::write("output.pdf", merged.into_bytes()?)?;
    Ok(())
}
```

### 2. Self-Hosted REST Microservice (`crates/rest`)

Start the daemon:
```bash
cargo run -p pdfcraft-rest -- --port 8080 --auth-token "secret-key"
```

Perform a synchronous document merge via curl:
```bash
curl -X POST http://localhost:8080/v1/merged-pdf \
  -H "Authorization: Bearer secret-key" \
  -H "Content-Type: application/json" \
  -d '{"sources": [{"path": "cover.pdf"}, {"path": "content.pdf"}]}' \
  --output combined.pdf
```

Submit an asynchronous background job:
```bash
# Submit job
curl -X POST http://localhost:8080/v1/jobs \
  -H "Authorization: Bearer secret-key" \
  -H "Content-Type: application/json" \
  -d '{"action": "split", "source": {"path": "large.pdf"}, "ranges": ["1-5", "6-10"]}'

# Poll status: GET http://localhost:8080/v1/jobs/{job_id}
# Download result: GET http://localhost:8080/v1/jobs/{job_id}/result
```

### 3. Python SDK (`sdks/python`)

Install locally in editable mode:
```bash
cd sdks/python
pip install -e .
```

Usage:
```python
from pdfcraft import PdfClient, DocumentSource

# Using the HTTP REST client
client = PdfClient(base_url="http://localhost:8080", token="secret-key")
doc = client.merge([
    DocumentSource.from_path("doc1.pdf"),
    DocumentSource.from_path("doc2.pdf"),
])
doc.save("merged.pdf")
```

### 4. TypeScript / Node.js SDK (`sdks/typescript`)

Install dependencies and build:
```bash
cd sdks/typescript
npm install
npm run build
```

Usage:
```typescript
import { PdfCraftClient } from "@pdfcraft/sdk";

const client = new PdfCraftClient({
  baseUrl: "http://localhost:8080",
  token: "secret-key",
});

const merged = await client.mergeDocuments(["invoice1.pdf", "invoice2.pdf"]);
await merged.saveToFile("invoices.pdf");
```

---

## 🔍 OpenAPI Specification & Contract Drift Checker

- **OpenAPI 3.1 Spec:** [`sdks/openapi.json`](openapi.json) defines all schemas and routes.
- **Contract Drift Verification:** To verify that OpenAPI schemas, Rust models, Python dataclasses, and TypeScript interfaces remain 100% identical, run:
  ```bash
  python3 sdks/check_contract_drift.py
  ```
  This script is a hard gate in CI.

---

## 🧪 Verification & Testing Commands

To verify that all SDK and REST components are functioning correctly:

```bash
# 1. Architectural layering test (ensures L7 compliance)
cargo run -p xtask -- layers

# 2. Run all unit and integration tests across SDK & REST crates
cargo test -p pdfcraft-sdk -p pdfcraft-rest

# 3. Verify cross-language schema alignment
python3 sdks/check_contract_drift.py

# 4. Run clippy gates on the SDK crates
cargo clippy -p pdfcraft-sdk -p pdfcraft-rest --all-targets -- -D warnings
```

---

## 📖 In-Depth Guides

- [**Canonical SDK & REST Architecture Guide**](../docs/sdk/README.md): Concurrency, memory bounds, error hierarchy, and token economics.
- [**SDK API Reference Manual**](../docs/sdk/api-reference.md): Detailed parameter tables, status codes, and multi-language signatures.
- [**Control Planes & Threading Model**](../docs/sdk/control-planes.md): Architectural diagram and communication channels.
- [**Runnable CLI Code Examples**](../docs/sdk/examples/): Standalone executable scripts for Rust, Python, and TypeScript.
- [**Protected Fork Extensions Policy**](../AGENTS.md#6-document-processing-sdk-rest-api--multi-language-bindings-protected-fork-extensions): Rules for syncing upstream changes while protecting fork code.
