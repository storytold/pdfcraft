# pdfcraft-rest

Production-ready, RFC 7807-compliant HTTP REST microservice and client library for PdfCraft document processing operations, offering an open-source, self-hosted alternative to pdfRest containers and Adobe PDF Services.

> [!NOTE]
> **Part of PdfCraft Enterprise Extensions ([PR #919](https://github.com/storytold/pdfcraft/pull/919), RFC [#875](https://github.com/storytold/pdfcraft/issues/875))**
> See the [Main README](../../README.md#⚡-fork-overview-enterprise-sdk--rest-api) for the full ecosystem overview and interactive diagram.

---

## 🏛️ Microservice Architecture

```text
  ┌─────────────────────────────────────────────────────────────┐
  │                 HTTP Client / Web Service                   │
  │           (curl, Python requests, TS Axios, Go)             │
  └──────────────────────────────┬──────────────────────────────┘
                                 │ HTTP/1.1 (Bearer Auth, JSON/Streams)
                                 ▼
  ┌─────────────────────────────────────────────────────────────┐
  │         pdfcraft-rest Server (Tokio & Axum L7)              │
  │  • OpenAPI 3.1 Route Dispatcher                             │
  │  • RFC 7807 Problem Details Handler                         │
  │  • Bounded Worker Semaphore (Concurrency Governance)        │
  │  • JobRepository (Async Job Queue & Retention)              │
  └──────────────────────────────┬──────────────────────────────┘
                                 │
                                 ▼
  ┌─────────────────────────────────────────────────────────────┐
  │     pdfcraft-sdk (OperationExecutor Port & Adapters)        │
  └──────────────────────────────┬──────────────────────────────┘
                                 │
                                 ▼
  ┌─────────────────────────────────────────────────────────────┐
  │             pdfcraft Engine Core (L6)                       │
  └─────────────────────────────────────────────────────────────┘
```

---

## 🛡️ Security Hardening & Astra Audit Summary (Score: 8/10)

Developed with iterative adversarial reviews by Staff Technical Architect `gpt-6-astra` (OpenAI Codex):
- **Server-Local Path Rejection (8/10):** The REST service strictly rejects server-local paths (`SourcePayload::Path` -> HTTP 400 Bad Request). All payloads must arrive as binary streams or in-memory Base64, completely preventing arbitrary filesystem reads.
- **Fail-Closed Delivery (8/10):** Zero-byte or partially materialized split outputs are never returned; failed jobs cleanly abort and purge temporary files.
- **Strict Boundary Ceilings (7/10):** Aggregated response payload ceiling (100 MiB limit) and streaming 1-byte lookahead probe prevent memory-exhaustion DoS attacks.
- **Path Traversal Protection (8/10):** Multipart and split filename extractors reject directory traversal (`..`), slashes, backslashes, and null bytes.

---

## 🚀 Running the Server

```bash
# Start daemon on port 8080 with a Bearer authentication token
cargo run -p pdfcraft-rest -- --port 8080 --auth-token "your-secret-token"
```

Or programmatically in Rust:
```rust
use pdfcraft_rest::server::{RestServer, ServerConfig};
use pdfcraft_sdk::LocalPdfClient;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(LocalPdfClient::new()?);
    let config = ServerConfig {
        bind_addr: "127.0.0.1:8080".parse()?,
        bearer_token: Some("your-secret-token".to_string()),
        max_concurrent_workers: 8,
        storage_dir: "./job-storage".into(),
        job_retention_secs: 3600,
    };

    let server = RestServer::new(config, client)?;
    server.run().await?;
    Ok(())
}
```

---

## 📡 REST API Endpoints (OpenAPI 3.1)

| Route | Method | Mode | Description |
| :--- | :--- | :--- | :--- |
| `/v1/merged-pdf` | `POST` | Synchronous | Merges two or more document streams into a single PDF |
| `/v1/split-pdf` | `POST` | Synchronous | Splits a document into multiple page ranges |
| `/v1/page-preview` | `POST` | Synchronous | Renders a high-DPI PNG raster of a given page |
| `/v1/jobs` | `POST` | Asynchronous | Submits a long-running batch job to the queue |
| `/v1/jobs/{id}` | `GET` | Polling | Returns status (`queued`, `running`, `completed`, `failed`) |
| `/v1/jobs/{id}/result`| `GET` | Download | Streams the completed binary output file |

All error responses strictly comply with **RFC 7807 Problem Details** (`application/problem+json`).

---

## 📚 Related Documentation

- **[Multi-Language SDK Overview](../../sdks/README.md)**: Python and TypeScript SDKs.
- **[Main Project README](../../README.md#⚡-fork-overview-enterprise-sdk--rest-api)**: Executive overview and architecture comparison.
- **[Rust SDK Crate](../sdk/README.md)**: `pdfcraft-sdk` in-process client library.
- **[OpenAPI 3.1 Specification](../../sdks/openapi.json)**: Machine-readable API schema.
- **[Canonical SDK Developer Guide](../../docs/sdk/README.md)**: Deployment topologies and architectural blueprints.
- **[Domain-by-Domain API Reference](../../docs/sdk/api-reference.md)**: Full API parameters, schemas, and signatures.
- **[Control Planes Blueprint](../../docs/sdk/control-planes.md)**: Architectural comparison of MCP, SDK, and REST.
