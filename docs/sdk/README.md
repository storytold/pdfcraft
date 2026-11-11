# PdfCraft Document Processing SDK & REST Service Guide

The **PdfCraft Document Processing SDK** provides a high-performance, `#![forbid(unsafe_code)]`, memory-safe document processing engine. It serves as an open-source alternative to the proprietary Adobe PDF Library (PDFL) for in-process execution, and pdfRest / Adobe PDF Services for containerized on-premise REST deployments.

> [!TIP]
> - For the complete function-by-function reference organized by domain with exhaustive parameters, return types, and code snippets, see the [**SDK API Reference Manual**](api-reference.md).
> - To understand how the SDK, REST service, and MCP server relate, see the [**Control Planes Architecture Guide**](control-planes.md).

---

## 1. Architectural Model & Layer Boundaries

### 1.1 PdfCraft Layers and Supported Boundaries

PdfCraft enforces a strict 7-layer architecture. The SDK and REST service operate entirely at the headless application and network layers, decoupled from the desktop/web graphical interface:

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│  Layer 7: Presentations & Remote Services                                   │
│  • pdfcraft-ui-egui (Desktop/Web UI — NOT exposed to SDK consumers)         │
│  • pdfcraft-rest (HTTP REST Daemon, Job Repository, Problem Details)        │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
┌──────────────────────────────────────▼──────────────────────────────────────┐
│  Layer 6: Application Façades & Orchestration                               │
│  • pdfcraft-sdk (Unified Client Contracts: LocalClient, RestClient)         │
│  • pdfcraft-automation (Headless Tool Registry, CLI, MCP Server)           │
│  • pdfcraft-engine (Session Orchestration, Undo Stack, CoW Revisions)       │
└──────────────────────────────────────┬──────────────────────────────────────┘
                                       │
┌──────────────────────────────────────▼──────────────────────────────────────┐
│  Layers 1–5: Domain Core & Low-Level Codecs                                 │
│  • L4–L5: pdfcraft-organize, pdfcraft-render, pdfcraft-annot, forms, sign   │
│  • L1–L3: pdfcraft-cos (Object Graph), crypt (AES/RC4), filters, fonts      │
└─────────────────────────────────────────────────────────────────────────────┘
```

| Layer | Component | Responsibility | Public to SDK Consumers? |
|---|---|---|---|
| **L7 (UI)** | `pdfcraft-ui-egui` | egui desktop/web presentation | **No** (Strictly isolated) |
| **L7 (Net)** | `pdfcraft-rest` | HTTP server, Bearer auth, job journal | REST clients only |
| **L6 (SDK)** | `pdfcraft-sdk` | Transport drivers (`LocalClient`, `RestClient`) | **Yes** (Primary API) |
| **L6 (Auto)**| `pdfcraft-automation`| Canonical headless tool catalog | Via CLI & MCP protocol |
| **L6 (Core)**| `pdfcraft-engine` | Document session & undo management | Via application adapters |
| **L1–L5** | `cos`, `render`, etc. | Pure PDF parsing, rendering, manipulation | Internal engine dependencies |

### 1.2 Dual-Transport Execution Model

PdfCraft exposes the **same operation vocabulary** across two transports. Operation semantics are shared, while result delivery, lifecycle, errors, authentication, and cancellation are transport-specific:

```text
                        ┌──────────────────────────────────────────────┐
                        │         Unified Operation Vocabulary         │
                        │ • call_tool (Universal 149-tool Bridge)      │
                        │ • merge, split, render_page                  │
                        │ • document (rotate, extract, delete, insert) │
                        │ • annot (highlight, stamp, comments)         │
                        │ • forms (fill, flatten, export, import)      │
                        │ • security (encrypt, decrypt, redact, sign)  │
                        │ • optimization (optimize, reduce_size)       │
                        └──────────────────────┬───────────────────────┘
                                               │
                ┌──────────────────────┴──────────────────────┐
                │                                             │
 ┌──────────────▼──────────────┐               ┌──────────────▼──────────────┐
 │     In-Process Driver       │               │     Remote REST Driver      │
 │       (LocalClient)         │               │        (RestClient)         │
 ├─────────────────────────────┤               ├─────────────────────────────┤
 │ • Zero network latency      │               │ • HTTP/1.1 Loopback or LAN  │
 │ • In-memory byte buffers    │               │ • Async Job Lifecycle       │
 │ • Native Rust / PyO3 / N-API│               │ • Bearer Token Auth         │
 │ • Ephemeral sandbox drop    │               │ • Bounded Worker Semaphore  │
 └──────────────┬──────────────┘               └──────────────┬──────────────┘
                │                                             │
                ▼                                             ▼
 ┌─────────────────────────────┐               ┌─────────────────────────────┐
 │  AutomationEngineAdapter    │               │     pdfcraft-rest Server    │
 └──────────────┬──────────────┘               ├─────────────────────────────┤
                │                              │ • Route Dispatcher          │
                │                              │ • BlockingExecutor (Pool)   │
                │                              │ • JobRepository (Journal)   │
                │                              └──────────────┬──────────────┘
                │                                             │
                └──────────────────────►◄─────────────────────┘
                                        │
                                        ▼
                        ┌──────────────────────────────┐
                        │ pdfcraft-engine / automation │
                        └──────────────────────────────┘
```

1. **In-Process Mode (`LocalClient`)**:
   - Executes directly within the host process via Rust, Python (PyO3 native extension), or TypeScript (Node.js N-API native addon).
   - Zero daemon lifecycle, zero port binding, zero network serialization.
   - Ideal for CLI tools, desktop plugins, and high-throughput serverless compute pipelines.

2. **On-Premise REST Mode (`RestClient`)**:
   - Connects over HTTP/HTTPS to an on-premise `pdfcraft-rest` daemon.
   - Bounded concurrency with worker semaphores, crash-resilient disk journaling, and RFC 7807 problem details.
   - Ideal for microservice fleets, web applications, and multi-service environments.

---

## 2. Divisions of Functions & Resource Limits

### 2.1 Document Merge (`merge`)
- **Capability**: Combines multiple independent PDF documents into a single document.
- **Input Sources**: Path references (`PathBuf`, file path strings) or in-memory byte buffers (`Vec<u8>`, `bytes`, `Buffer`).
- **Options (`MergeOptions`)**:
  - `output_filename`: Optional virtual filename for output metadata.
  - `pages`: Specific page range selectors (e.g., `["1-3", "5"]`).

### 2.2 Document Split & Extraction (`split`)
- **Capability**: Partitions a source document into multiple PDF documents.
- **Split Modes (`SplitMode`)**:
  - `EveryNPages(n)`: Splits into consecutive chunks of $N$ pages each.
  - `BeforePages(vec![p1, p2])`: Inserts split boundaries before designated page numbers.
  - `AtBookmarks(level)`: Splits at outline bookmarks of the specified level.

### 2.3 High-DPI Page Rendering (`render_page`)
- **Capability**: Renders a designated page to a raster image at an exact physical target resolution.
- **DPI Calculation**: Decoupled from monitor or OS PPI (#739); dimensions are computed strictly via:
  $$\text{pixels} = \frac{\text{points} \times \text{dpi}}{72.0}$$
- **Options (`RenderOptions`)**:
  - `dpi`: Target resolution (default `150.0`, screen `72.0`, print `300.0`, archival `600.0`).
  - `image_format`: Output format (`png` or `jpeg`).

### 2.4 Resource Limits & Safe Spilling Contract

PdfCraft guarantees resource containment through explicit quotas:

| Dimension | Default Value | Configurable? | Enforcement Behavior |
|---|---|---|---|
| **Memory Spill Threshold** | 10 MB | Yes (`max_memory_bytes`) | Outputs $\le$ 10 MB stay in RAM; outputs $>$ 10 MB automatically spill to disk. |
| **Max Input Payload** | 128 MB | Yes (`max_input_bytes`) | Fast rejection with `QUOTA_EXCEEDED` before parsing. |
| **Max Render Resolution** | 600.0 DPI | Yes (`max_render_dpi`) | Rejection with `INVALID_ARGUMENT` if exceeded. |
| **Max Worker Concurrency** | CPU core count | Yes (`max_workers`) | Bounded semaphore queue; rejects with `WORKER_POOL_SATURATED` (429) if full. |
| **Panic Containment** | Enabled | Permanent | `catch_unwind` turns hostile engine panics into actionable `ENGINE_PANIC_CONTAINED` errors. |

### 2.5 Unified Error Taxonomy & Transport Mapping

The application defines stable, transport-neutral error categories. The REST transport serializes them as RFC 7807 Problem Details; local clients expose them as language-native typed exceptions carrying the same error code:

| Error Code | HTTP Status | Rust SDK | Python SDK | TypeScript SDK |
|---|---|---|---|---|
| `INVALID_ARGUMENT` | 400 Bad Request | `SdkError::InvalidArgument` | `InvalidArgumentError` | `InvalidArgumentError` |
| `UNAUTHORIZED` | 401 Unauthorized | `SdkError::Unauthorized` | `UnauthorizedError` | `UnauthorizedError` |
| `NOT_FOUND` | 404 Not Found | `SdkError::NotFound` | `NotFoundError` | `NotFoundError` |
| `CONFLICT` | 409 Conflict | `SdkError::Conflict` | `ConflictError` | `ConflictError` |
| `QUOTA_EXCEEDED` | 413 Payload Too Large | `SdkError::QuotaExceeded` | `QuotaExceededError` | `QuotaExceededError` |
| `WORKER_POOL_SATURATED`| 429 Too Many Requests | `SdkError::PoolSaturated` | `PoolSaturatedError` | `PoolSaturatedError` |
| `ENGINE_ERROR` | 500 Internal Error | `SdkError::EngineError` | `PdfCraftError` | `PdfCraftError` |
| `ENGINE_PANIC_CONTAINED`| 500 Internal Error | `SdkError::EnginePanic` | `PdfCraftError` | `PdfCraftError` |

---

## 3. Language Guide: Rust Developers

### Crate Dependencies (`Cargo.toml`)
```toml
[dependencies]
pdfcraft-sdk = { version = "0.5.0", path = "crates/sdk" }
# Optional: for self-hosting the REST daemon or connecting remotely
pdfcraft-rest = { version = "0.5.0", path = "crates/rest" }
```

### 3.1 In-Process Execution (`LocalClient`)
```rust
use std::fs;
use pdfcraft_sdk::{
    DocumentResult, DocumentSource, LocalClient, MergeOptions, PdfCraftClient, RenderOptions,
    ResourceLimits, SdkError,
};

fn main() -> Result<(), SdkError> {
    let client = LocalClient::default().with_limits(ResourceLimits {
        max_memory_bytes: 64 * 1024 * 1024, // 64 MB before disk spill
        ..ResourceLimits::default()
    });

    // 1. Merge documents from disk or memory buffers
    let doc1 = DocumentSource::from_path("cover.pdf");
    let doc2 = DocumentSource::from_bytes(b"%PDF-1.4 sample content".to_vec(), None);
    let merged = client.merge(&[doc1, doc2], MergeOptions::default())?;
    
    println!("Merged: {} bytes (MIME: {})", merged.byte_len(), merged.mime_type());
    match merged {
        DocumentResult::Memory { data, .. } => fs::write("final.pdf", data)?,
        DocumentResult::File { path, .. } => {
            fs::copy(path, "final.pdf")?;
        }
    }

    // 2. High-DPI page rendering
    let render = client.render_page(
        &DocumentSource::from_path("final.pdf"),
        1, // Page 1 (1-based)
        RenderOptions { dpi: 300.0, ..Default::default() },
    )?;
    println!("Rendered 300 DPI page image: {} bytes", render.byte_len());

    Ok(())
}
```

### 3.2 Production REST Daemon Deployment
```rust
use std::sync::Arc;
use pdfcraft_rest::executor::{BlockingExecutor, ExecutorConfig};
use pdfcraft_rest::routes::RestService;
use pdfcraft_rest::server::{HttpServer, ServerConfig};
use pdfcraft_rest::storage::DurableFileJobRepository;
use pdfcraft_sdk::engine_adapter::AutomationEngineAdapter;
use pdfcraft_sdk::local::OperationExecutor;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine: Arc<dyn OperationExecutor> = Arc::new(AutomationEngineAdapter::default());
    let executor = Arc::new(BlockingExecutor::new(engine, ExecutorConfig::default()));
    
    // Durable disk journal with crash recovery
    let repo = Arc::new(DurableFileJobRepository::new("./data/job-journal")?);

    let service = Arc::new(RestService::new(
        executor,
        repo,
        Some("production-secret-token".into()),
    ));

    let config = ServerConfig {
        bind_addr: "127.0.0.1:8080".parse()?,
        allow_public_bind: false,
    };

    let server = HttpServer::start(service, config)?;
    println!("PdfCraft REST Daemon listening on http://127.0.0.1:8080");
    
    // Keep running until shutdown signal
    std::thread::park();
    server.stop();
    Ok(())
}
```

---

## 4. Language Guide: Python Developers

### Package Installation
```bash
# Production install
pip install pdfcraft

# Or local development checkout
pip install ./sdks/python
```

### 4.1 In-Process Execution (`LocalClient`)

> [!NOTE]
> **Native Extension Prerequisite**: `LocalClient` executes directly against the compiled in-process native engine (`pdfcraft._native`). When running from a source checkout, compile the native extension via `maturin develop` inside `sdks/python` or `cargo build -p pdfcraft-pyo3 --release`. If `pdfcraft._native` is unavailable, `LocalClient()` raises `ImportError`. For environments without a native Rust toolchain, use `RestClient` (Section 4.2) pointing to a running `pdfcraft-rest` daemon instead.

```python
from pathlib import Path
from pdfcraft.local import LocalClient
from pdfcraft.types import DocumentResult, DocumentSource, MergeOptions, RenderOptions

client = LocalClient()

def save_result(result: DocumentResult, destination: str) -> None:
    if result.data is not None:
        Path(destination).write_bytes(result.data)
    elif result.path is not None:
        Path(destination).write_bytes(result.path.read_bytes())
    else:
        raise RuntimeError("DocumentResult has no data or path")

# 1. Merge documents
merged = client.merge([
    DocumentSource.from_path("section1.pdf"),
    DocumentSource.from_path("section2.pdf"),
], options=MergeOptions(output_filename="complete.pdf"))
save_result(merged, "complete.pdf")

# 2. Split document
split = client.split("complete.pdf", every_n_pages=2)
print(f"Split generated {len(split.files)} files")

# 3. High-DPI Page Rendering
preview = client.render_page("complete.pdf", page=1, options=RenderOptions(dpi=300.0))
save_result(preview, "page1_300dpi.png")

# 4. Universal Tool Bridge (Any of the 149 engine tools)
tool_res = client.call_tool("form_fields", {"doc": 1})
print(f"Extracted fields natively: {tool_res}")
```

### 4.2 Remote REST Client (`RestClient`)
```python
from pdfcraft.rest import RestClient
from pdfcraft.errors import InvalidArgumentError

client = RestClient(base_url="http://127.0.0.1:8080", auth_token="production-secret-token")

# 1. Synchronous Merge
try:
    result = client.merge(["docA.pdf", "docB.pdf"])
    result.save_to_path("merged.pdf")
except InvalidArgumentError as e:
    print(f"Validation failed: {e}")

# 2. Asynchronous Job with Polling
job = client.merge_async(["large_doc1.pdf", "large_doc2.pdf"])
result = job.wait_for_completion(timeout_secs=60)
result.save_to_path("async_merged.pdf")
```

---

## 5. Language Guide: TypeScript / Node.js Developers

### Package Installation
```bash
# Production install
npm install @pdfcraft/sdk

# Or local development checkout
npm install ./sdks/typescript
```

### 5.1 In-Process Execution (`PdfCraftLocalClient`)

> [!NOTE]
> **Native Addon Prerequisite**: `PdfCraftLocalClient` requires the compiled Node.js native addon (`pdfcraft_napi`). If the native addon is not available in the environment, client methods reject with `PdfCraftError` carrying error code `NATIVE_UNAVAILABLE` (HTTP 501). For environments without native compilation toolchains, use `PdfCraftRestClient` (Section 5.2) connecting to an on-premise or containerized `pdfcraft-rest` daemon instead.

```typescript
import { writeFile } from "node:fs/promises";
import { PdfCraftLocalClient } from "@pdfcraft/sdk";
import type { DocumentResult } from "@pdfcraft/sdk";

const client = new PdfCraftLocalClient();

async function saveResult(result: DocumentResult, path: string) {
  if (result.type !== "memory") throw new Error("Expected an in-memory result");
  await writeFile(path, result.data);
}

async function run() {
  // 1. Merge files
  const merged = await client.merge(["chapter1.pdf", "chapter2.pdf"]);
  await saveResult(merged, "book.pdf");

  // 2. High-DPI page preview
  const preview = await client.renderPage("book.pdf", 1, { dpi: 300 });
  await saveResult(preview, "preview.png");
}

run().catch(console.error);
```

### 5.2 Remote REST Client (`PdfCraftRestClient`)
```typescript
import { writeFile } from "node:fs/promises";
import { PdfCraftRestClient, InvalidArgumentError } from "@pdfcraft/sdk";
import type { DocumentResult } from "@pdfcraft/sdk";

const client = new PdfCraftRestClient({
  baseUrl: "http://127.0.0.1:8080",
  authToken: "production-secret-token",
});

async function saveResult(result: DocumentResult, path: string) {
  if (result.type !== "memory") throw new Error("Expected an in-memory result");
  await writeFile(path, result.data);
}

async function run() {
  try {
    // 1. Synchronous execution
    const merged = await client.merge(["doc1.pdf", "doc2.pdf"]);
    await saveResult(merged, "out.pdf");

    // 2. Asynchronous background execution (asyncJob = true)
    const asyncResult = await client.merge(
      ["large1.pdf", "large2.pdf"],
      undefined,
      true
    );
    await saveResult(asyncResult, "async_out.pdf");
  } catch (err) {
    if (err instanceof InvalidArgumentError) {
      console.error("Validation rejected:", err.message);
    }
  }
}

run().catch(console.error);
```

---

## 6. Where Documents Belong in the Repository

| Document Type | Location | Tracked in Git? | Description |
|---|---|---|---|
| **Public User Guide** | [`docs/sdk/README.md`](README.md) | **Yes** | Canonical developer documentation across Rust, Python, and TypeScript. |
| **API Reference Manual**| [`docs/sdk/api-reference.md`](api-reference.md) | **Yes** | Exhaustive domain-by-domain function reference with full signatures and examples. |
| **Runnable Examples** | [`docs/sdk/examples/`](examples/) | **Yes** | Standalone executable CLI examples for all 5 lifecycle stages. |
| **Package READMEs** | `crates/sdk/README.md`<br>`crates/rest/README.md`<br>`sdks/python/README.md`<br>`sdks/typescript/README.md` | **Yes** | Quickstarts for crates.io, PyPI, and npm registry packages. |
| **API Specification** | [`sdks/openapi.json`](../sdks/openapi.json) | **Yes** | Canonical OpenAPI 3.1.0 schema for the REST service. |
| **Architecture Specification** | `docs/plans/pdfcraft-rest-sdk-plan.md` | **No** (Gitignored) | Comprehensive technical architecture and design document. |
| **Implementation Tasks** | `docs/plans/tasks.md` | **No** (Gitignored) | Engineering task breakdown across Phases 1–5. |
| **Local Test Fixtures** | `/sample-docs/` | **No** (Gitignored) | Local test files (never committed or published per `AGENTS.md` §1 asset policy). |
| **Local Test Scripts** | `/scripts/` | **No** (Gitignored) | Local test harnesses for offline fixture verification. |
