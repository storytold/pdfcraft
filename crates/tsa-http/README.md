# pdfcraft-tsa-http

Layer L7: the opt-in network transport for RFC 3161 time servers. `pdfcraft-sign` builds and checks
timestamp requests and tokens but never opens a socket (`TimestampAuthority`); this crate is the one
place that sends such a request, and the desktop app (`pdfcraft-ui-egui`) and the automation tools
(`pdfcraft-automation`) reach a time server only through it (`xtask/src/layers.rs` allows both edges).

- **Off unless a server is named.** There is no default server. Preferences ▸ Signatures ▸ Time
  server starts at None; `sign_document` sends nothing without `timestamp_server`. `SUGGESTED` lists
  well-known free servers (DigiCert, Sectigo, GlobalSign, Entrust) that a person may pick.
- **What is sent:** an RFC 3161 `TimeStampReq` by HTTP POST (`application/timestamp-query`, RFC 3161
  §3.4): a SHA-256 fingerprint of the signature value (for a document timestamp, of the file), a random
  nonce and a request for the server's certificate. Never the document.
- **What is accepted:** `HttpAuthority` checks the answer against the request (status, CMS signature,
  imprint, nonce) before handing it on, so a bad answer counts as the server's failure. Answers are
  capped at 4 MiB, redirects are not followed, and one request may take `DEFAULT_TIMEOUT` (10 s).
- **Never fails a signing:** `sign_with_fallback` signs without the token when the server can't be
  reached or answers wrongly, and returns why, so the caller can say the signature uses this
  computer's clock. An error from before the server was asked (a wrong password, say) stays an error.
- **Addresses:** `check_url` accepts `http://` and `https://` addresses with a server name or IP
  address, an optional port and a path, and refuses credentials, fragments, spaces and non-ASCII
  characters. It is total on any input, including settings read back from disk.
- **Trust:** time servers usually speak plain `http://`; the token is signed and checked here and at
  validation, so the transport adds nothing to its trust. `https://` uses `ureq` over `rustls` with the
  operating system's root certificates (`rustls-native-certs`), as the desktop app's update check
  does. No root certificate list is bundled.
- **Browser build:** no sockets; a request fails with a clear error, and the app doesn't offer the
  setting there.

The `test-server` feature adds `test_server`, a loopback HTTP server that answers with a closure, for
this crate's tests and the app's and tools' end-to-end tests. Shipped builds never enable it.
