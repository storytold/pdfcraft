# pdfcraft-ai

The optional AI assistant: summarize a document, answer a question about it, or translate it, through a provider the user configures.

- **Layer:** L4. No UI. No network code unless the `http` feature is on (only the desktop app and the CLI turn it on).
- **Off by default.** PdfCraft ships no provider, endpoint, model or key. Nothing is sent anywhere until a caller hands `run` a `Provider` and a `Transport`. The desktop app does so only after the user turns the assistant on in Preferences; the CLI only when `PDFCRAFT_AI_ENDPOINT` and `PDFCRAFT_AI_MODEL` are set.
- **Status:** the three text tasks work end to end. See "What is missing" below.

## API

```rust
let provider = Provider::new(Api::OpenAi, "http://localhost:11434/v1", "llama3").with_key(None);
let answer = run(&provider, &HttpTransport, &Task::Ask { question: "Who signed it?".into() }, &pages)?;
answer.text                      // the model's words, plain text
answer.coverage                  // pages sent, pages with text, whether text was left out
```

`request` and `answer` are the two halves of `run` (build the HTTP request; read the reply), for callers that send it themselves. `Transport` is one method, `post(&Request) -> Result<Response>`.

## Providers

| `Api` | Request | Key |
|---|---|---|
| `OpenAi` | `POST {endpoint}/chat/completions` (the format Ollama, llama.cpp, LM Studio, vLLM and most gateways speak) | optional, `Authorization: Bearer` |
| `Anthropic` | `POST {endpoint}/v1/messages`, `anthropic-version: 2023-06-01` | required, `x-api-key` |

The model name is always the user's. No sampling parameters are sent, so each provider's defaults apply.

## Safety rules this crate keeps

- **A key never travels in the clear to another machine.** With a key set, the endpoint must be `https://`, or `http://` on this computer (`localhost`, `127.0.0.0/8`, `[::1]`). Plain `http` to another host is allowed only without a key.
- **The key is never printed.** `Provider`'s `Debug` hides it, `Request` has no `Debug`, and errors carry the provider's message, never a header.
- **No redirects.** The request body is the user's document; `HttpTransport` refuses to follow a redirect to somewhere else.
- **The document is data.** Its text is fenced between `<document>` lines, a closing marker inside it is defused, and the system prompt tells the model not to follow instructions found there. This lowers the risk of prompt injection; it cannot remove it, which is why the answer is only ever shown as text and never acted on.
- **Nothing is left out silently.** One request carries at most `Provider::max_input_chars` characters of document text (60,000 by default): whole pages in order, then the start of the next. `Coverage` reports how many pages went and whether text was cut, the prompt tells the model, and the UI and tools tell the user.
- **Untrusted input everywhere.** Settings, questions, document text and the provider's reply are all length-capped and validated; replies are read up to 8 MiB and answers kept up to 200,000 characters. TLS uses the operating system's trusted roots.

## What is missing

- Only the first `max_input_chars` of a long document is sent: there is no chunking, map-reduce summary or retrieval, so questions about later pages of a long document can't be answered.
- Page citations are requested in the prompt, not verified against the document, and are not clickable.
- No streaming (the answer appears when it is complete) and no cancelling a request in flight.
- One question at a time: no conversation history.
- Translate returns text; it does not produce a translated PDF.
- Scanned pages need Recognize text first: images are not sent.
- The desktop app keeps no key between sessions (it is typed each time, or read from an environment variable); there is no OS keychain integration.
- Not available in the web build (no transport there).
- No test runs against a real provider; the tests use a stand-in transport and a local socket.
