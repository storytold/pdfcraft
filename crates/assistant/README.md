# Optional document assistant

This native prototype adds read-only summaries and questions about the open PDF.
It is off until the user enables it. It neither edits PDFs nor installs models or CLIs.

## Try the desktop interface

Build from this PR using the normal PdfCraft prerequisites:

```sh
cargo run -p pdfcraft -- path/to/a-synthetic-document.pdf
```

Choose **View > Side panels > AI assistant**, then **Activar asistente**.
Choose **Español** or **English** under **Answer language**. This controls answers
and summaries regardless of the question's language. Quotes retain the source language.
The prototype's new settings and diagnostic labels are primarily Spanish; complete
interface catalog translations are a follow-up in craft-i18n.

Open **Cambiar** to choose a provider:

- **Ollama**: use an existing local server, normally `http://127.0.0.1:11434`.
  **Buscar modelos** lists existing models; none are downloaded. Select a text model. Known Ollama cloud entries are excluded. A loopback-compatible
  server may itself forward requests; its operator remains responsible for its routing.
- **OpenAI / compatible API**: enter an HTTPS endpoint, model and, if required,
  your own API key. A loopback-compatible server can also be used over HTTP.
  Hosted OpenAI requires a separate API key; a Codex sign-in does not authenticate it.
- **Claude Code**: install and sign in to the official CLI separately. Select its
  executable if discovery did not find it. The app uses its default model.
- **Codex**: install and sign in to the official CLI with ChatGPT separately.
  Select its executable if necessary. The app uses its default model.

For either CLI or a remote API, review the destination and explicitly authorize
external document processing for the current session. Then choose **Summarize document**
or type a question and choose **Ask**. Spanish labels appear when Spanish is selected.
Use a synthetic document for initial testing. Existing subscription quotas and account
terms apply. The API option can incur separate charges. No automatic paid fallback exists.

Answer language and CLI/local-provider settings survive a restart; API keys, external
consent and the enabled state do not. Remote URLs are not persisted. Keys are held in memory for the session.
Changing provider clears its key and consent. Closing the panel or changing the document
cancels a running answer. CLI requests have a 120-second upper bound.

## Trust and scope

Only extracted page text is sent, with a bounded context and an explicit list of omitted
or partial pages. Existing OCR layers are identified; optional in-memory OCR uses the
app's recognizer. A quotation is accepted only if it matches source page text (with
normalization for extraction artifacts); a wrong page can be corrected only within sent
pages. This checks the quotation, not the interpretation or completeness of the answer.
Unsupported statements are labelled and unmatched quotations are rejected.

Copy-restricted documents and documents with unapplied redactions are refused rather
than sending concealed content. HTTP proxies and redirects are disabled. Remote HTTP is
refused. TLS uses operating-system trust roots, matching the app's existing HTTP stack.
CLI runs use an empty temporary directory, tools disabled, no session persistence and
API credential environment variables removed. CLI configuration/authentication remains
subject to the installed program; this is not an OS security sandbox for an arbitrary executable.

No streaming UI, conversation history, autonomous editing, multi-document workflow or
complete automation-tool registration is included. Translation/OCR, hosted API execution,
Windows/Linux and wasm have not received the live macOS demo's end-to-end validation.
The native UI is excluded from wasm. The standalone `ask` example offers an Ollama
exercise without the GUI; it is not an MCP tool.

## Review and tests

```sh
cargo test -p pdfcraft-assistant
cargo test -p pdfcraft-ui-egui assistant_ui::tests
cargo check -p pdfcraft-ui-egui
cargo clippy -p pdfcraft-assistant -p pdfcraft-ui-egui --all-targets -- -D warnings
cargo fmt --check
cargo xtask layers
cargo xtask assets
```

Tests use synthetic PDFs, fake CLIs and loopback servers, with no paid inference.
See the PR description for actual commands/results and remaining gates.

Related work: #3112, #182, #5744 and #6054. This is an alternative partial proposal for
review, not a claim that #3112's agentic editing scope is complete.
