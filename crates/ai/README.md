# pdfcraft-ai

The sans-IO core of the AI assistant (M13): what is sent to a model, how its streamed answer is
read, what an answer can cite, and the personal PDF library searched on this machine.

- **Layer:** L4, standalone: no dependency on any other PdfCraft crate. `engine` builds the context
  from open documents, `platform` supplies the network transport, `ui-egui` and `automation` are
  the frontends.
- **No I/O:** nothing here opens a connection, reads a file or a clock. The [`Transport`] trait is
  the seam: without one (the web build, a headless run without the network flag) nothing is sent.
  The crate builds for `wasm32-unknown-unknown`.
- **Licence:** MIT OR Apache-2.0. Written from Anthropic's public API documentation (Messages,
  streaming, search results, citations, web search, prompt caching, vision, effort, errors;
  `anthropic-version: 2026-10-21`), the WHATWG HTML "server-sent events" parsing rules, UAX #29
  (word and sentence boundaries, through `unicode-segmentation`), UAX #15 (NFKC, through
  `unicode-normalization`) and the BM25 ranking function (Robertson and Zaragoza, "The
  Probabilistic Relevance Framework: BM25 and Beyond", 2009). No SDK or other client was read.
- **Design:** the ADR "AI assistant" (`plan/adr/`, local-only).

## API

```rust
// Requests (claude.rs)
user_turn(passages: &[Passage], image: Option<&Image>, question) -> Result<Message, RequestError>
request(&Options, &Secret, history: &[Message], turn: &Message) -> Result<HttpRequest, RequestError>
sent_sources(messages: &[Message]) -> Vec<Option<SourceId>>   // what a citation's index names
error_event(status, body) -> Event                            // an HTTP error response

// Answers
Decoder::new(sent_sources) ; push(bytes, &mut Vec<Event>) ; finish(&mut Vec<Event>)
Decoder::message() -> Message      // the assistant message, blocks as received, to replay
Decoder::usage() -> Usage ; Decoder::stop_reason()

pub enum Event { Started, Text { block, text }, Citation { block, source }, WebSearch { query },
                 WebResults { results, error }, Usage { usage }, Stop { reason },
                 ProviderError { kind, message }, Skipped { what } }

// Citations (source.rs)
pub enum Source { ThisDocument { page, quote }, Library { sha256, page, quote }, Web { url, title, quote } }
SourceId::{ThisDocument { page }, Library { sha256, page }}   // "pdfcraft-doc:i3", "pdfcraft-lib:<sha256>:i3"
Sha256::of(bytes) ; DocRef { sha256, size }

// Library (library/)
Library::add(NewDocument) -> Result<Added, LibraryError> ; remove(&Sha256) ; entries()
Library::search(query, k, exclude: Option<&Sha256>) -> Vec<Hit>
Library::to_json() ; Library::from_json(bytes) -> Result<(Library, Vec<String> /*warnings*/), LibraryError>

// Models and cost (model.rs)
MODELS, default_model(), find(id), Model::cost_nanodollars(&Usage), format_dollars(n)

// Transport (transport.rs)
trait Transport { fn post_stream(&self, &HttpRequest, on_bytes) -> Result<u16, TransportError> }
check_url(url)      // https anywhere, http only to localhost / 127.0.0.0/8 / ::1; no user info
Secret              // Debug prints a mask; no Display or Serialize
HttpRequest::preview() -> RequestPreview   // secret headers masked
```

## What is sent

One request per question: the fixed system prompt, the conversation so far exactly as it was
sent and received, then the new turn.
- **Passages** become `search_result` blocks, with citations on. That covers the selection and a
  window of its page, and the library passages retrieved. Each passage is split into one text block
  per sentence, so a citation names the sentences it quotes.
- **The image**, if there is one, is base64.
- **The question** comes last.

Cache breakpoints sit after the system prompt and at the end of the new turn, so the next
question reads the conversation from the cache. Breakpoints left in a replayed message are
removed, so a request never carries more than two.

**Settings:**
- `thinking` is adaptive, with display omitted. Opus 5.5 can't turn thinking off.
- `output_config.effort` is always sent; Opus 5.5 defaults to `medium`.
- `fallbacks: "default"` re-runs a refused request on another model, with its beta header. Haiku
  5.5 doesn't support it, so the model table leaves it out there.
- Web search (`web_search_20260209`, called directly) is added only when asked for.

## Never panic

- Stream lines and event data are capped at 1 MiB each.
- One answer keeps at most 4 MiB of text, 4,096 blocks and 500,000 events. After that it stops with
  `StopReason::TooLong`.
- Malformed JSON, deltas for blocks that never started and citations of passages that weren't
  sent each become an `Event::Skipped`. Unknown event types are ignored, as the API asks clients
  to do. A stream that ends early stops with `StopReason::Truncated`.
- Bodies are decoded in 64 KiB pieces, so one huge chunk from the transport can't queue more
  events than the cap.
- A web search domain filter is sent whole or refused: more than 64 domains, a domain over 253
  bytes, or both an allow list and a block list is a `RequestError`.
- Citations are checked against what was sent: the index must name a passage of ours, and the
  echoed `source` must match it. Web citations must be `http(s)` addresses with a host.
- Library limits: 10,000 documents, 20,000 pages per document, 256 KiB of text per page, 500,000
  passages and 256 MiB of text in all, 512 MiB for a saved file. A saved library in the wrong
  format or version is an error. A damaged but readable one keeps what is valid, with warnings.

## Library

- Passages are about 1,000 characters, cut on sentence boundaries. Each starts with the last
  sentence of the one before, unless that sentence is longer than 500 characters.
- Tokens are NFKC, lowercased and split on Unicode word boundaries. Chinese, Japanese and Korean
  become each character plus each overlapping pair of characters, so a one-character query
  matches too.
- Ranking is BM25 (k1 = 1.2, b = 0.75), at most one passage per page. While other documents also
  match, at most half the results come from one document.
- The index is rebuilt from the passages when a library is loaded, and isn't saved.
- **Loading.** A saved library is read entry by entry: a damaged entry or passage is dropped
  alone, with a warning. A loaded library is never over the caps. An entry that lost passages
  is indexed in full when its file is added again.
- **A later read of the same bytes.** If it reaches further (an open copy read with its
  password, or with the owner's), it replaces the entry's status: needs password, then not
  permitted, then no text, then indexed.
- **Search exclusions.** `search` leaves out any number of documents. For the document being
  read, `engine::Session::library_exclusions` names its own bytes and any copy last seen at its
  path.
- Prices in `model.rs` are display estimates, dated in `PRICES_AS_OF`. The provider's bill is the
  authority.

## Tests

`cargo test -p pdfcraft-ai`:
- **Property tests.** A stream split anywhere decodes the same, and arbitrary bytes never panic,
  for the SSE decoder and the Claude decoder alike. Library search and the saved form are total.
- **A golden request.** The exact JSON body and its masked headers.
- **Hand-written SSE fixtures.** A cited answer with thinking, and a web search with an error
  result and a `pause_turn`. Also errors, truncation and the answer cap.
- **Ranking** on a small synthetic corpus, and damaged saved libraries.
