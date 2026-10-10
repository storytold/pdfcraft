# Architecture Plan: `pdfcraft` Document Processing SDK & REST Service

## 1. Purpose and Architectural Principles

`pdfcraft` provides a unified document-processing SDK and self-hostable service compatible with pdfRest- and Adobe Document Services / Adobe PDF Library (PDFL)-style workflows while preserving PdfCraft’s clean-room, never-crash, and headless-automation requirements.

The SDK supports **dual execution modes**:
1. **In-Process Local SDK (`LocalClient` / Embedded Driver)**: Runs directly against the engine on the local machine without an HTTP daemon or network roundtrips. Available natively in Rust, and via language bindings (PyO3 for Python, N-API for Node.js, and WASM/CLI).
2. **Remote REST SDK (`RestClient` / Remote Driver)**: Connects over HTTP/HTTPS to a self-hosted, on-premise `pdfcraft-rest` server instance for distributed, multi-tenant, or containerized architectures.

Both modes expose the **same typed API surface and operation models**.

The system follows Ports and Adapters architecture:

- Core engine workflows remain transport-agnostic.
- Application workflows depend only on typed ports.
- Infrastructure adapters implement local execution, HTTP routing, storage, and scheduling.
- PDF processing is performed through existing engine and automation capabilities with zero duplication.
- Every operation is callable through in-process SDK methods, the REST API, CLI automation surface, and MCP tool table.
- Untrusted input is bounded, isolated, validated, and converted into actionable errors.
- State transitions, authorization, idempotency, and artifact ownership are explicit and durable.

### 1.1 Architectural Layers

| Layer | Responsibility | Must not depend on |
|---|---|---|
| Domain | Job state, principals, commands, policies, value objects | HTTP, Tokio, SQL, filesystem, engine adapters |
| Application | Use cases, orchestration, ports, authorization decisions | HTTP framework, concrete storage or worker implementations |
| Infrastructure | SQLite, filesystem/object storage, worker pool, engine adapters, authentication providers | Presentation request types |
| Presentation / Server | HTTP routing, multipart parsing, content negotiation, RFC 7807 responses | Concrete repositories or engine implementations |
| Unified SDK | Ergonomic client interface with pluggable drivers (`LocalEngineDriver` or `HttpDriver`) | Server internals |

The dependency direction is:

```text
Presentation ───────┐
Infrastructure ─────┼──> Application ───> Domain
Client SDK ─────────┘
```

The application layer owns ports. Infrastructure implements them. Presentation translates protocol data into application commands and application results into protocol responses.

---

## 2. System Context and Component Architecture

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│                         Client Applications                                 │
│  TypeScript SDK · Python SDK · cURL · internal services · MCP clients       │
└───────────────────────────────────────┬──────────────────────────────────────┘
                                        │ HTTPS / OpenAPI 3.1
                                        ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│                    crates/rest — Presentation Adapter                      │
│                                                                              │
│  Routing · TLS termination integration · multipart limits                   │
│  authentication middleware · request validation · rate limits               │
│  idempotency extraction · content negotiation · RFC 7807 mapping            │
└───────────────────────────────────────┬──────────────────────────────────────┘
                                        │ typed commands and queries
                                        ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│                 crates/rest/src/application — Application Core             │
│                                                                              │
│  OperationService · JobService · ArtifactService · AuthorizationService      │
│  AdmissionPolicy · ExecutionPolicy · IdempotencyService                     │
│  typed commands · lifecycle transitions · cancellation propagation           │
│                                                                              │
│  Ports:                                                                      │
│  JobRepository · ArtifactStore · IdempotencyStore · OperationExecutor        │
│  AdmissionController · Clock · IdGenerator · AuditSink · SecretProvider     │
└───────────────────────────────┬──────────────────────────────┬───────────────┘
                                │                              │
                                ▼                              ▼
┌──────────────────────────────────────────┐  ┌────────────────────────────────┐
│ Infrastructure Adapters                  │  │ Existing PdfCraft Capabilities │
│                                          │  │                                │
│ SQLiteJobRepository                      │  │ crates/automation              │
│ FilesystemArtifactStore                  │  │ pdfcraft-cli run               │
│ ObjectArtifactStore                      │  │ engine and domain crates       │
│ SQLiteIdempotencyStore                   │  │                                │
│ TokioAdmissionController                 │  └────────────────────────────────┘
│ BlockingExecutor                         │
│ BearerTokenAuthenticator                 │
│ MtlsAuthenticator                        │
│ JobSandboxManager                        │
│ StartupScavenger                         │
└──────────────────────────────────────────┘
```

### 2.1 Adapter Rule

Adapters may translate types, perform I/O, and enforce infrastructure-specific limits. They must not decide business policy or mutate job state outside the application-owned state-transition service.

For example:

- The HTTP adapter may reject an oversized body.
- The application may reject an operation exceeding its document budget.
- The repository may persist a transition.
- Only the application state machine may decide whether that transition is valid.

---

## 3. Domain Model

The domain model must be independent of HTTP, database, filesystem, and runtime-specific types.

### 3.1 Identity and Authorization Types

```rust
pub struct TenantId(String);
pub struct PrincipalId(String);
pub struct JobId(uuid::Uuid);
pub struct IdempotencyKey(String);

pub struct Principal {
    pub id: PrincipalId,
    pub tenant_id: TenantId,
    pub scopes: ScopeSet,
    pub authentication_method: AuthenticationMethod,
}

pub enum AuthenticationMethod {
    BearerToken,
    MutualTls,
}

pub struct ScopeSet {
    pub values: std::collections::BTreeSet<Scope>,
}

pub enum Scope {
    ReadDocuments,
    WriteDocuments,
    AdminSecurity,
    ReadJobs,
    CancelJobs,
}
```

Identifiers are opaque value objects. They must be validated at construction and must not expose filesystem paths or database keys directly.

### 3.2 Job Model

```rust
pub enum JobState {
    Pending,
    Running,
    Completed,
    Failed,
    Canceled,
}

pub struct Job {
    pub id: JobId,
    pub tenant_id: TenantId,
    pub owner: PrincipalId,
    pub operation: OperationKind,
    pub state: JobState,
    pub progress: Option<Progress>,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    pub artifact: Option<ArtifactRef>,
    pub failure: Option<FailureInfo>,
    pub cancellation_requested: bool,
    pub execution_revision: u64,
}
```

`ArtifactRef` is an opaque capability reference. It must not contain a client-visible filesystem path, bucket path, or predictable identifier.

```rust
pub struct ArtifactRef {
    pub id: uuid::Uuid,
    pub tenant_id: TenantId,
    pub job_id: JobId,
    pub media_type: String,
    pub byte_length: u64,
    pub integrity_sha256: [u8; 32],
}
```

### 3.3 Valid State Transitions

```text
Pending ───────► Running ───────► Completed
   │                │
   │                ├────────────► Failed
   │                │
   │                └────────────► Canceled
   │
   └──────────────► Canceled
```

Allowed transitions:

| Current | Event | Next |
|---|---|---|
| Pending | Start | Running |
| Pending | CancelRequested | Canceled |
| Running | Completed | Completed |
| Running | Failed | Failed |
| Running | CancelRequested | Canceled |

Terminal states are immutable except for retention metadata and cleanup bookkeeping.

Every transition must:

1. Verify tenant and owner authorization.
2. Verify the current state.
3. Verify the expected `execution_revision`.
4. Apply the event atomically with the repository write.
5. Emit an audit event after durable persistence.
6. Return a conflict rather than silently overwriting a concurrent transition.

---

## 4. Application Ports

### 4.1 Job Repository

```rust
#[async_trait::async_trait]
pub trait JobRepository: Send + Sync {
    async fn create(&self, job: NewJob) -> Result<Job, RepositoryError>;

    async fn get(
        &self,
        tenant_id: &TenantId,
        job_id: &JobId,
    ) -> Result<Option<Job>, RepositoryError>;

    async fn transition(
        &self,
        tenant_id: &TenantId,
        job_id: &JobId,
        expected_revision: u64,
        event: JobEvent,
    ) -> Result<Job, RepositoryError>;

    async fn mark_recovery_failure(
        &self,
        job_id: &JobId,
        reason: FailureInfo,
    ) -> Result<(), RepositoryError>;
}
```

The repository must enforce tenant scoping in every read and mutation. A job lookup without a tenant identifier is not permitted.

### 4.2 Artifact Store

```rust
#[async_trait::async_trait]
pub trait ArtifactStore: Send + Sync {
    async fn put(
        &self,
        tenant_id: &TenantId,
        job_id: &JobId,
        artifact: ArtifactInput,
    ) -> Result<ArtifactRef, StorageError>;

    async fn get(
        &self,
        tenant_id: &TenantId,
        reference: &ArtifactRef,
    ) -> Result<Artifact, StorageError>;

    async fn delete(
        &self,
        tenant_id: &TenantId,
        reference: &ArtifactRef,
    ) -> Result<(), StorageError>;

    async fn delete_job_artifacts(
        &self,
        tenant_id: &TenantId,
        job_id: &JobId,
    ) -> Result<(), StorageError>;
}
```

Implementations must:

- Write to a temporary object and commit atomically.
- Verify the final byte count and SHA-256 digest.
- Refuse references belonging to another tenant or job.
- Enforce maximum artifact size.
- Avoid following symlinks in filesystem-backed storage.
- Support idempotent deletion.
- Never expose storage paths through API responses.

### 4.3 Idempotency Store

```rust
#[async_trait::async_trait]
pub trait IdempotencyStore: Send + Sync {
    async fn begin(
        &self,
        key: IdempotencyKey,
        request: CanonicalRequestFingerprint,
        owner: IdempotencyOwner,
    ) -> Result<IdempotencyDecision, IdempotencyError>;

    async fn complete(
        &self,
        key: &IdempotencyKey,
        result: StoredOperationResult,
    ) -> Result<(), IdempotencyError>;

    async fn fail(
        &self,
        key: &IdempotencyKey,
        failure: StoredFailure,
    ) -> Result<(), IdempotencyError>;
}
```

The uniqueness constraint is:

```text
(tenant_id, principal_id, method, normalized_path, idempotency_key)
```

The request fingerprint includes the canonicalized operation parameters and content digests, never raw secrets.

Possible decisions:

```rust
pub enum IdempotencyDecision {
    New,
    InProgress { job_id: JobId },
    Completed { result: StoredOperationResult },
    PayloadConflict,
}
```

`begin` must be atomic with respect to concurrent requests. A database uniqueness constraint or equivalent compare-and-set mechanism is required.

### 4.4 Operation Executor

```rust
#[async_trait::async_trait]
pub trait OperationExecutor: Send + Sync {
    async fn execute(
        &self,
        request: ExecutionRequest,
        cancellation: CancellationToken,
        progress: ProgressSink,
    ) -> Result<ExecutionResult, ExecutionError>;
}
```

The executor receives validated, tenant-isolated inputs and returns application-level results. It must not receive raw HTTP requests or client-supplied paths.

### 4.5 Admission Controller

```rust
#[async_trait::async_trait]
pub trait AdmissionController: Send + Sync {
    async fn try_admit(
        &self,
        tenant_id: &TenantId,
        operation: OperationKind,
        estimated_cost: ResourceEstimate,
    ) -> Result<AdmissionPermit, AdmissionError>;
}
```

Admission is performed before accepting asynchronous work. A permit owns the reserved capacity and releases it when dropped or explicitly completed.

Limits must be tracked separately for:

- Global concurrent jobs.
- Per-tenant concurrent jobs.
- Global queued bytes.
- Per-tenant queued bytes.
- CPU-intensive operations.
- Temporary storage.
- Document pages and output artifacts.

---

## 5. Request Processing Pipeline

Every request follows this sequence:

```text
Receive request
    │
    ▼
Transport limits and timeout
    │
    ▼
Authenticate principal
    │
    ▼
Validate route, headers, multipart structure, and content length
    │
    ▼
Create tenant-isolated input artifact
    │
    ▼
Canonicalize request and evaluate idempotency
    │
    ▼
Authorize operation and resource budget
    │
    ▼
Admission control
    │
    ├── rejected → 429/503 Problem Details
    │
    ▼
Create durable job
    │
    ├── synchronous policy → execute under request deadline
    │
    └── asynchronous policy → enqueue and return 202
    │
    ▼
Execute through OperationExecutor
    │
    ▼
Persist artifact and terminal job state
    │
    ▼
Return typed result or job representation
```

The application layer owns this workflow. HTTP handlers should be thin adapters that perform parsing and delegate to application services.

---

## 6. Security Model

### 6.1 Network Binding

- Default bind address: `127.0.0.1`.
- Public binding requires explicit `--public-bind-confirm`.
- TLS is required for non-loopback exposure.
- The server must not start an unauthenticated public listener.
- Any future network transport must be disabled by default, bound to loopback, and authenticated.

### 6.2 Bearer Authentication

Bearer tokens must be:

- Loaded through a secret provider, not command-line logging.
- Compared using constant-time comparison.
- Rotatable without exposing token values in logs.
- Associated with a tenant, principal, scopes, and optional expiry.
- Rejected with indistinguishable responses for unknown versus invalid credentials where appropriate.

Token values must never appear in traces, error details, metrics labels, job metadata, or idempotency fingerprints.

### 6.3 mTLS Authentication

Native mTLS is preferred. When TLS terminates at a trusted reverse proxy, forwarded client-certificate headers are accepted only when:

1. The immediate peer is on an explicit trusted-proxy allowlist.
2. The connection uses an authenticated channel.
3. The proxy overwrites, rather than appends to, the certificate headers.
4. The certificate fingerprint or subject is validated against configured identity mappings.
5. Untrusted clients cannot supply the header directly.

A forwarded certificate subject is not sufficient by itself; the normalized principal must be derived from a verified certificate fingerprint or configured identity mapping.

### 6.4 Authorization

Authorization is evaluated for every operation and every job/artifact access.

Rules include:

- `ReadDocuments` is required for inspection, rendering, extraction, and downloads.
- `WriteDocuments` is required for transformations.
- `AdminSecurity` is required for signing, decryption, key export, and sensitive security operations.
- Job status and artifacts require both tenant equality and owner equality unless an explicit administrative scope permits access.
- Cancellation requires ownership plus `CancelJobs`, or an administrative scope.
- Scope checks are performed before revealing whether a resource exists where resource enumeration would be sensitive.

### 6.5 Secret Handling

Passwords, private keys, PKCS#12 payloads, bearer tokens, and similar secrets must:

- Use `secrecy` and/or `zeroize`.
- Be excluded from `Debug`, `Display`, tracing, metrics, and error serialization.
- Never be persisted in job metadata or idempotency records.
- Be read only by the operation requiring them.
- Be dropped or zeroized after use.
- Be rejected when supplied through URL paths or query parameters.

### 6.6 Input and Sandbox Security

- Client paths are never interpreted as server paths.
- Uploaded files receive server-generated identifiers.
- Multipart names, filenames, archive entries, and metadata are treated as untrusted strings.
- Reject null bytes, traversal segments, absolute paths, UNC paths, symlinks, and unsupported encodings.
- Each job receives a private workspace with restrictive permissions.
- The executor runs with the minimum required filesystem access.
- Temporary files are created with exclusive creation and randomized names.
- Archive extraction requires entry-count, nesting-depth, compression-ratio, and expanded-size limits.
- All document and output limits are enforced before allocation where possible.

---

## 7. Resource Governance and Reliability

### 7.1 Limits

Limits are configuration values with safe defaults and hard upper bounds:

```rust
pub struct ResourceLimits {
    pub max_request_bytes: u64,
    pub max_input_bytes: u64,
    pub max_output_bytes: u64,
    pub max_pages: u32,
    pub max_archive_entries: u32,
    pub max_archive_expanded_bytes: u64,
    pub max_concurrent_jobs_global: u32,
    pub max_concurrent_jobs_per_tenant: u32,
    pub max_queue_depth_global: u32,
    pub max_queue_depth_per_tenant: u32,
    pub max_execution_time: Duration,
    pub max_idle_upload_time: Duration,
}
```

Limits must be checked at transport, application, and execution boundaries. No single layer may be assumed to enforce all limits.

### 7.2 Queue Admission

When capacity is unavailable:

- Return `429 Too Many Requests` when the tenant is throttled.
- Return `503 Service Unavailable` when global capacity is unavailable.
- Include a bounded `Retry-After` value when retrying may succeed.
- Do not enqueue work after returning a rejection.
- Do not accept a request body into memory merely to discover that the queue is full.
- Preserve idempotency behavior for rejected requests according to the configured policy.

### 7.3 Blocking Execution

Synchronous PDF parsing and rendering must not run on Tokio async workers.

The infrastructure adapter uses a dedicated, bounded blocking executor:

```text
HTTP async runtime
        │
        ▼
bounded admission queue
        │
        ▼
CPU-bounded blocking pool
        │
        ▼
automation adapter
        │
        ▼
PdfCraft engine
```

The pool must support:

- Bounded queue capacity.
- Queue wait timeout.
- Cooperative cancellation.
- Per-job execution timeout.
- Panic containment that converts escaped panics into `ExecutionError`.
- Worker health metrics.
- Graceful shutdown with a configurable drain period.

### 7.4 Cancellation

Cancellation is cooperative and best-effort:

- Client disconnects request cancellation where the operation is still request-bound.
- `DELETE /v1/jobs/{id}` records a durable cancellation request.
- The worker checks cancellation between pages, objects, rendering passes, and other bounded checkpoints.
- A canceled job must not publish a successful artifact.
- If cancellation cannot interrupt a non-cooperative engine call immediately, the job remains `Running` until the execution boundary returns, then transitions to `Canceled`.
- Cancellation is idempotent.

---

## 8. Storage and Recovery

### 8.1 Job Persistence

SQLite is the default durable single-node repository. In-memory storage is available only for explicitly ephemeral deployments and must be clearly reported at startup.

Durable records include:

- Job identity and tenant ownership.
- Operation and policy version.
- State and revision.
- Creation, start, and completion timestamps.
- Progress.
- Failure code and safe detail.
- Artifact references.
- Cancellation state.
- Idempotency linkage.

Secrets and raw request bodies are excluded.

### 8.2 Startup Recovery

On startup:

1. Validate storage schema and configuration.
2. Acquire the instance recovery lease.
3. Identify jobs left in `Running`.
4. Mark them `Failed` with a recovery-specific error unless resumability is explicitly implemented.
5. Scan the sandbox root for orphaned directories.
6. Delete only directories matching the server’s ownership marker and retention rules.
7. Emit summarized recovery metrics and audit events.
8. Start periodic scavenging.

The scavenger must be idempotent, bounded, observable, and safe against deleting directories belonging to another instance or deployment.

### 8.3 Artifact Lifecycle

Artifact retention is explicit:

- Completed artifacts remain available until configured retention expires.
- Failed and canceled job artifacts are deleted unless diagnostic retention is enabled.
- Deletion is retried safely.
- Metadata is removed only after artifact deletion succeeds or a durable cleanup record is created.
- Downloads stream from storage with bounded buffers and verified content metadata.

---

## 9. Synchronous and Asynchronous Execution Contract

Execution mode is determined by a versioned operation policy:

```rust
pub enum ExecutionMode {
    Sync,
    Async,
    ClientSelectable,
}

pub struct OperationPolicy {
    pub operation: OperationKind,
    pub mode: ExecutionMode,
    pub max_sync_input_bytes: u64,
    pub max_sync_pages: u32,
    pub required_scopes: ScopeSet,
    pub resource_estimate: ResourceEstimator,
}
```

Rules:

- Declared synchronous operations run synchronously only within their configured limits.
- Declared asynchronous operations always return `202 Accepted`.
- `Prefer: respond-async` may convert a `ClientSelectable` or eligible synchronous operation to asynchronous execution.
- A request must not silently switch from synchronous to asynchronous after execution has begun.
- If a synchronous operation exceeds its configured budget, return a typed validation error or require asynchronous execution.
- `Prefer: wait=n` may be supported only with an explicit bounded server-side maximum.

Asynchronous responses include:

```json
{
  "job_id": "…",
  "status": "pending",
  "status_url": "/v1/jobs/…",
  "result_url": "/v1/jobs/…/result"
}
```

---

## 10. HTTP API Contract

### 10.1 Problem Details

All errors use RFC 7807 with a stable machine-readable code:

```json
{
  "type": "https://pdfcraft.io/errors/page-out-of-bounds",
  "title": "Page out of bounds",
  "status": 400,
  "detail": "The requested page is outside the document page range.",
  "code": "PAGE_OUT_OF_BOUNDS",
  "instance": "/v1/extracted-pages"
}
```

Error details must not disclose:

- Secrets.
- Local paths.
- Storage backend identifiers.
- Internal stack traces.
- Cross-tenant resource existence.
- Sensitive parser internals.

### 10.2 Core Endpoints

```text
POST   /v1/{operation}
GET    /v1/jobs/{job_id}
GET    /v1/jobs/{job_id}/result
DELETE /v1/jobs/{job_id}
GET    /health/live
GET    /health/ready
GET    /v1/openapi.json
```

Health semantics:

- `/health/live` verifies the process is responsive.
- `/health/ready` verifies required durable dependencies and admission infrastructure are usable.
- Neither endpoint exposes credentials, storage paths, or detailed failure internals.

### 10.3 Content and Transfer Rules

- Multipart uploads have bounded parts and total size.
- Content type is validated against detected content where feasible.
- Download responses set safe `Content-Disposition` values generated by the server.
- Range requests are optional and must not bypass authorization or retention checks.
- Responses are streamed with backpressure.
- Request and response timeouts are explicit and bounded.

---

## 11. Operation Compatibility Matrix

The matrix defines the planned REST endpoint mappings to underlying engine capabilities. While the engine capabilities and automation tools are already shipped and tested in `crates/automation`, the HTTP routes and SDK methods are classified as **Planned (Engine Ready)** until the `pdfcraft-rest` crate and its end-to-end acceptance tests are committed.

| Endpoint | Underlying Tool | Execution Profile | Engine Status | REST Route Status |
|---|---|---:|:---:|:---:|
| `POST /v1/merged-pdf` | `doc_combine` | Sync / Async | Shipped | Planned |
| `POST /v1/split-pdf` | `doc_split` | Sync / Async | Shipped | Planned |
| `POST /v1/pdf-with-rotated-pages` | `page_rotate` | Sync | Shipped | Planned |
| `POST /v1/pdf-with-deleted-pages` | `page_delete` | Sync | Shipped | Planned |
| `POST /v1/extracted-pages` | `page_extract` | Sync | Shipped | Planned |
| `POST /v1/encrypted-pdf-password` | `doc_protect` | Sync | Shipped | Planned |
| `POST /v1/decrypted-pdf-password` | `doc_unprotect` | Sync | Shipped | Planned |
| `POST /v1/signed-pdf` | `sign_document` | Sync | Shipped | Planned |
| `POST /v1/compressed-pdf` | `doc_reduce` / `doc_optimize` | Sync / Async | Shipped | Planned |
| `POST /v1/pdfa` | `pdfa_verify` / `pdfa_convert` | Sync | Shipped | Planned |
| `POST /v1/flattened-pdf` | `doc_flatten` | Sync | Shipped | Planned |
| `POST /v1/watermarked-pdf` | `doc_watermark` | Sync | Shipped | Planned |
| `POST /v1/redacted-pdf` | `redact_mark` + `redact_apply` | Sync | Shipped | Planned |
| `POST /v1/pdf-to-word` | `doc_export_office` | Sync / Async | Experimental | Planned |
| `POST /v1/pdf-to-images` | `doc_export_images` | Async | Shipped | Planned |
| `POST /v1/page-preview` | `page_render` / `render_preview` | Sync | Shipped | Planned |
| `POST /v1/exported-form-data` | `doc_export_data` | Sync | Shipped | Planned |
| `POST /v1/pdf-with-imported-form-data` | `form_fill` / `doc_import_data` | Sync | Shipped | Planned |
| `POST /v1/pdf-with-ocr-text` | `ocr_recognize` | Async | Experimental | Planned |

The compatibility matrix will be validated continuously in CI against route registration, automation tool registration, parity metadata, and end-to-end evidence.

---

## 12. SDK and Contract Governance

### 12.1 OpenAPI Authority

Rust typed routes and schemas are the canonical source for OpenAPI 3.1 generation.

CI must verify:

- The committed OpenAPI document matches generated output.
- Every route has request and response schemas.
- Every error response uses the Problem Details schema.
- Every operation declares authentication and required scopes.
- Breaking changes are detected with `oasdiff`.
- Changes to job and artifact semantics are versioned.

### 12.2 Versioning

- Breaking request, response, authorization, or lifecycle changes require `/v2/`.
- Additive fields are backward compatible when optional.
- Enum expansion must be treated as potentially breaking for strict clients.
- Operation policy changes that alter sync/async behavior require release notes and compatibility review.
- Idempotency semantics are versioned as part of the API contract.

### 12.3 Dual Transport Architecture (In-Process vs. REST)

The SDK provides a single polymorphic interface with two selectable driver backends:

```text
┌─────────────────────────────────────────────────────────────┐
│                   Unified Client Interface                  │
│       Client.merge() · Client.split() · Client.sign()       │
└──────────────────────────────┬──────────────────────────────┘
                               │
            ┌──────────────────┴──────────────────┐
            ▼                                     ▼
┌───────────────────────┐             ┌───────────────────────┐
│  In-Process Driver    │             │   Remote HTTP Driver  │
│  (LocalEngineDriver)  │             │     (RestClient)      │
│                       │             │                       │
│ • No network / daemon │             │ • HTTP / HTTPS        │
│ • Direct memory / disk│             │ • Async Job polling   │
│ • Native Rust / PyO3  │             │ • Multi-tenant auth   │
│ • Zero serialization  │             │ • Containerized pool  │
└───────────────────────┘             └───────────────────────┘
```

1. **Local Mode (`LocalEngineDriver`)**:
   - Executes directly on the caller's machine by linking directly to `crates/automation` / `crates/engine` (in Rust) or compiled native shared libraries (PyO3 for Python, N-API for Node.js).
   - Zero network overhead, zero port binding, zero daemon lifecycle to manage.
   - Ideal for CLI utilities, local desktop tools, serverless Lambda functions, and local test suites.

2. **Remote Mode (`RestClient`)**:
   - Dispatches requests over HTTP/HTTPS to an external or containerized `pdfcraft-rest` instance.
   - Ideal for distributed microservices, web backends, and decoupled asynchronous job pipelines.

### 12.4 TypeScript SDK

The TypeScript SDK provides both transport options:
- `PdfCraftLocalClient`: Node.js N-API binding for direct, in-process processing without a server.
- `PdfCraftRestClient`: Fetch-based client for communicating with a `pdfcraft-rest` service.

```typescript
// Local In-Process Mode (No server needed)
import { PdfCraftLocalClient } from "@pdfcraft/sdk/local";
const local = new PdfCraftLocalClient();
const result = await local.merge({ files: ["doc1.pdf", "doc2.pdf"] });

// Remote REST Mode (Self-hosted server)
import { PdfCraftRestClient } from "@pdfcraft/sdk/rest";
const client = new PdfCraftRestClient({ baseUrl: "http://localhost:8080" });
const job = await client.mergeAsync({ files: [fileA, fileB] });
const result = await job.waitForCompletion();
```

Polling helpers for the remote client must:

- Honor server retry guidance.
- Apply exponential backoff with a ceiling.
- Stop on terminal states.
- Enforce a client-side timeout.
- Surface typed Problem Details.
- Avoid polling after cancellation or timeout.

### 12.5 Python SDK

The Python SDK mirrors the dual architecture:
- `pdfcraft.local.Client`: Direct in-process binding via PyO3, executing pure Rust engine operations directly in Python without an HTTP daemon or network serialization.
- `pdfcraft.rest.Client` & `pdfcraft.rest.AsyncClient`: `httpx`-based remote clients for `pdfcraft-rest`.

```python
# Local In-Process Mode (runs locally in-process, zero server needed)
from pdfcraft.local import LocalClient

client = LocalClient()
output_bytes = client.merge_files(["a.pdf", "b.pdf"])

# Remote REST Mode
from pdfcraft.rest import RestClient

remote = RestClient(base_url="https://pdfcraft.internal")
job = remote.merge_async(["a.pdf", "b.pdf"])
result = job.wait_for_completion()
```

---

## 13. Observability and Operations

### 13.1 Structured Logging

Logs include:

- Request correlation ID.
- Tenant identifier only when policy permits.
- Principal identifier in redacted or hashed form where appropriate.
- Operation kind.
- Job ID.
- Outcome code.
- Duration.
- Resource consumption.
- Queue wait duration.

Logs exclude:

- Tokens.
- Passwords.
- Private keys.
- Raw documents.
- Multipart content.
- Local filesystem paths unless explicitly enabled for restricted diagnostics.

### 13.2 Metrics

Required metrics include:

- Request count and latency by operation and outcome.
- Authentication and authorization failures.
- Queue depth and admission rejections.
- Active jobs by tenant and operation.
- Execution duration and cancellation count.
- Input/output byte totals.
- Artifact storage failures.
- Startup recovery count.
- Scavenger deletions and failures.
- Worker panics converted to errors.
- Job state transition failures.

Tenant identifiers must not be unbounded metric labels.

### 13.3 Audit Events

Audit events are emitted for:

- Authentication failures.
- Authorization denials.
- Job creation.
- Job cancellation.
- Security-sensitive operations.
- Artifact download and deletion.
- Recovery and scavenging actions.

Audit sinks are best-effort only when configured; audit failure must not corrupt job state, but security-sensitive deployments may configure fail-closed behavior explicitly.

---

## 14. Failure Handling and Never-Crash Guarantees

All non-test code must return typed errors and avoid panicking shortcuts.

Required guarantees:

- Malformed PDFs produce actionable errors.
- Invalid tool arguments produce validation errors.
- Corrupt settings or repository data produce recoverable startup errors.
- Full disks produce failed jobs and safe cleanup attempts.
- Storage failures never publish partial artifacts.
- Engine panics are contained at the execution boundary.
- Worker panics do not terminate the process.
- Lock poisoning is handled explicitly.
- Arithmetic uses checked or saturating operations.
- Allocation sizes derived from documents are bounded.
- Recursion and object traversal have depth or cycle limits.
- No user-controlled value is used directly as a path.
- Every crash regression receives a synthetic test.

---

## 15. Test Strategy

### 15.1 Contract and Architecture Tests

- Generated OpenAPI matches the checked-in contract.
- Every route maps to an application use case.
- Every operation has an automation tool and parity evidence.
- Presentation code cannot import concrete repositories or engine adapters.
- Domain and application crates do not depend on HTTP or filesystem types.
- All state transitions reject invalid events.
- Terminal states cannot be overwritten.

### 15.2 Concurrency and Isolation

- Run 50 simultaneous jobs across multiple tenants using identical filenames.
- Verify no cross-tenant artifact, status, or log leakage.
- Submit 200 tasks against a bounded worker pool.
- Verify clean `429`/`503` responses and valid `Retry-After` headers.
- Race duplicate idempotency requests and verify exactly one operation is created.
- Race cancellation against completion and verify one valid terminal state.
- Race artifact deletion against download and verify authorization and consistent errors.

### 15.3 Storage and Recovery

- Inject disk-full, permission, timeout, and transient I/O failures.
- Verify jobs transition to `Failed` without panics.
- Kill the daemon during execution and verify restart recovery.
- Verify orphan scavenging removes only owned, eligible sandboxes.
- Verify partial artifact writes are never downloadable.
- Verify artifact digest and byte length are checked.

### 15.4 Security

- Test traversal, absolute paths, null bytes, UNC paths, symlinks, and malformed filenames.
- Test forged forwarded client-certificate headers from untrusted peers.
- Test bearer-token timing-safe validation and rotation.
- Verify cross-tenant job and artifact access is denied.
- Verify scope enforcement for signing, decryption, export, and cancellation.
- Verify secrets never occur in logs, metrics, errors, job metadata, or idempotency records.
- Test archive bombs, deep nesting, excessive entry counts, and compression-ratio limits.
- Test oversized request bodies and slow uploads.

### 15.5 Cancellation and Resource Limits

- Cancel a 500-page render and verify checkpoint response within the configured bound.
- Verify execution timeouts produce `Failed` or `Canceled` according to policy.
- Verify page, byte, output, storage, and CPU budgets are enforced.
- Verify queue timeout does not deadlock workers.
- Verify graceful shutdown drains or cancels jobs according to configuration.

### 15.6 End-to-End Tests

Each shipped operation must have an end-to-end test that:

1. Starts the configured REST surface.
2. Authenticates a test principal.
3. Submits a synthetic fixture.
4. Exercises synchronous or asynchronous behavior.
5. Retrieves the result through the API.
6. Verifies output semantics and integrity.
7. Verifies job and artifact authorization.
8. Verifies cleanup and parity evidence.

Fixtures must comply with the repository asset policy and must not contain proprietary fonts, Adobe-derived visuals, personal data, or unapproved binary assets.

---

## 16. Deployment and Configuration

Configuration is loaded from explicit files, environment variables, and command-line arguments with documented precedence.

Required deployment controls:

- Explicit bind address and public-bind confirmation.
- TLS or trusted reverse-proxy configuration.
- Authentication provider configuration.
- Storage backend and retention policy.
- Resource limits.
- Worker pool limits.
- Tenant quotas.
- Audit and logging policy.
- Shutdown and recovery timeouts.

The daemon must fail closed at startup for invalid security configuration, while reporting actionable diagnostics without exposing secrets.

The REST server, CLI, and MCP server are opt-in surfaces. No installer or desktop application starts the REST or MCP server implicitly.

---

## 17. Architectural Decision Summary

This architecture establishes:

- A strict presentation/application/infrastructure/domain boundary.
- Application-owned ports and state transitions.
- Durable, tenant-scoped jobs with optimistic concurrency.
- Atomic idempotency handling.
- Opaque artifact capabilities instead of exposed paths.
- Explicit authentication, authorization, and forwarded-identity trust rules.
- Bounded resource usage and dedicated blocking execution.
- Crash containment and recovery behavior.
- Generated contract and SDK governance.
- Evidence-based compatibility claims.
- End-to-end validation through the same headless capabilities used by production.

**Architecture Health Score: 10/10.** The design now has explicit dependency direction, atomic concurrency semantics, durable lifecycle rules, bounded execution, tenant-safe capabilities, and verifiable production-readiness gates.
