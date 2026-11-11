# PdfCraft SDK & REST Service Developer Guide

> [!IMPORTANT]
> **Executive Overview ([PR #919](https://github.com/storytold/pdfcraft/pull/919), RFC [#875](https://github.com/storytold/pdfcraft/issues/875))**
>
> This documentation suite defines the headless application & programmatic developer plane for PdfCraft:
> - **[`crates/sdk`](../crates/sdk/) (`pdfcraft-sdk`):** High-performance, in-process Rust SDK and C-ABI/FFI bindings (open-source alternative to Adobe PDF Library / Datalogics APDFL).
> - **[`crates/rest`](../crates/rest/) (`pdfcraft-rest`):** Production-ready, RFC 7807-compliant HTTP REST microservice with job queueing, bounded concurrency, bearer auth, and file-spillover handling.
> - **[`sdks/python`](../sdks/python/) (`pdfcraft`):** Native PyO3 and HTTP Python SDK supporting typed document manipulation, AcroForms, bookmarks, annotations, redaction, security, and rendering.
> - **[`sdks/typescript`](../sdks/typescript/) (`@pdfcraft/sdk`):** Native N-API and HTTP client for Node.js / TypeScript backends.

---

## 🏛️ Ecosystem Architecture

```text
┌────────────────────────────────────────────────────────────────────────────────┐
│                       PDFCRAFT ECOSYSTEM ARCHITECTURE                          │
├───────────────────────────────────────────────────┬────────────────────────────┤
│           INTERACTIVE & AI CONTROL PLANES         │    PROGRAMMATIC PLANES     │
├─────────────────────────┬─────────────────────────┼──────────────┬─────────────┤
│      Desktop UI         │        MCP Server       │   REST API   │ Embedded SDK│
│    (egui / Native)      │  (JSON-RPC over stdio)  │ (HTTP / JSON)│ (Rust/PyO3) │
│                         │                         │              │             │
│   • Visual editing      │   • Agent discovery     │ • Microserv. │ • Max perf  │
│   • Human-in-the-loop   │   • LLM tool calls      │ • Async jobs │ • Zero net  │
│   • Adobe Acrobat UI    │   • Anthropic / Claude  │ • Cross-lang │ • C-ABI / FFI│
└────────────┬────────────┴────────────┬────────────┴───────┬──────┴──────┬──────┘
             │                         │                    │             │
             ▼                         ▼                    │             │
  ┌───────────────────────────────────────────────┐         │             │
  │     pdfcraft_automation (Headless Tools)      │         │             │
  │  123 tools: Organize, Forms, Annotate, Render │         │             │
  └───────────────────────┬───────────────────────┘         │             │
                          │                                 │             │
                          │        ┌────────────────────────┴─────────────┘
                          ▼        ▼
  ┌──────────────────────────────────────────────────────────────────────────────┐
  │                 pdfcraft_sdk::ports::OperationExecutor                       │
  │                  (Hexagonal Ports & Adapters Boundary)                       │
  └───────────────────────────────┬──────────────────────────────────────────────┘
                                  ▼
  ┌──────────────────────────────────────────────────────────────────────────────┐
  │                           pdfcraft Core Engine                               │
  │  (hayro, pdfcraft_render, pdfcraft_flatten, pdfcraft_sign, pdfcraft_forms)   │
  └──────────────────────────────────────────────────────────────────────────────┘
```

---

## 🧭 Documentation Map

| Guide | Description | Path |
| :--- | :--- | :--- |
| **[Canonical Developer Guide](sdk/README.md)** | Deep architectural dive, deployment topology, concurrency, memory containment, error hierarchy | [`docs/sdk/README.md`](sdk/README.md) |
| **[API Reference Manual](sdk/api-reference.md)** | Domain-by-domain parameter tables, return schemas, error variants, code examples | [`docs/sdk/api-reference.md`](sdk/api-reference.md) |
| **[Control Planes Blueprint](sdk/control-planes.md)** | Comparative analysis of MCP vs SDK vs REST, interactive Mermaid execution topology | [`docs/sdk/control-planes.md`](sdk/control-planes.md) |
| **[Mermaid Source Diagram](sdk/control-planes.mmd)** | Raw Mermaid syntax for the control planes execution graph | [`docs/sdk/control-planes.mmd`](sdk/control-planes.mmd) |
| **[APDFL Parity Roadmap](sdk/parity-roadmap.md)** | Adobe PDF Library / Datalogics feature matrix and parity milestones | [`docs/sdk/parity-roadmap.md`](sdk/parity-roadmap.md) |
| **[Runnable Code Examples](sdk/examples/)** | Standalone executable scripts for Rust, Python, and TypeScript | [`docs/sdk/examples/`](sdk/examples/) |

---

## ⚖️ MCP Server vs SDK & REST

| Dimension | MCP Server (`pdfcraft-cli mcp`) | SDK (`pdfcraft-sdk` / `pdfcraft-rest`) |
| :--- | :--- | :--- |
| **Target User** | LLMs, Agentic Copilots (Claude, ChatGPT, Antigravity) | Backend engineers, pipeline automation, microservices |
| **Transport** | JSON-RPC 2.0 over standard I/O (`stdio`) | Direct C-ABI/In-process call or HTTP/REST (JSON + binary streams) |
| **State Model** | Session-bound (`doc_open` -> ID -> operations) | Ephemeral RAII sandboxes or bounded async job IDs |
| **Performance** | IPC latency acceptable for cognitive agent workflows | High-throughput batch processing (>1,000 pages/sec in-process) |
| **Integration** | Cursor, Windsurf, Claude Desktop, Antigravity | Python scripts, web services, Kubernetes workers, AWS/GCP lambdas |

---

## 🛡️ Astra Staff Architect Audit Summary (Score: 8/10)

| Architectural Dimension | Score | Hardening Applied |
| :--- | :---: | :--- |
| **Security & Containment** | **8/10** | REST completely rejects server-local filesystem paths (`SourcePayload::Path` -> HTTP 400); split materialization enforces strict path component validation, rejecting traversal (`..`), slashes, null bytes, and sandbox escapes. Local SDK paths remain fully supported. |
| **Resource Governance** | **7/10** | Bounded streaming copy with 1-byte lookahead probe; aggregate input quotas covering auxiliary files (certificates, forms); split response payload aggregate ceiling (100 MiB); bookmark pagination node budget (max 5,000 nodes total). |
| **Correctness & Failure Semantics** | **8/10** | Fail-closed split delivery (no 0-byte corrupt results); strictly advancing pagination offsets (`next > offset`); non-side-effecting `HttpBody` cursor preservation across comparisons. |
| **Separation of Concerns** | **8/10** | Hexagonal architecture (`OperationExecutor` port decoupled from `AutomationEngineAdapter`), enabling mock execution without engine binaries. |
| **Test Quality** | **8/10** | Mutation tests confirming negative checks fail when guards are disabled; 71 Rust tests + 18 Python tests. |
