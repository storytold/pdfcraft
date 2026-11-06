# ID Austria signing (desktop preview)

PdfCraft can prepare a PDF, open the original ID Austria / A-Trust authorization
pages, and complete a detached PDF signature from the returned CMS. This integration
is a preview pending personal authorization and native platform qualification.
A working anonymous connection or a synthetic signature test does not establish
that a production ID Austria signature succeeds.

## Requirements and data transfer

- An activated personal ID Austria with signing capability, an Internet connection,
  and the provider's normal personal authorization method.
- An installed Chromium, Google Chrome or Microsoft Edge browser. PdfCraft discovers
  common installation paths on Windows, macOS, Linux and FreeBSD. If necessary, set
  `PDFCRAFT_SIGNING_BROWSER` to its existing absolute executable path.
- A new output filename in a writable folder. The source PDF is never overwritten.

There is no PdfCraft account, backend, developer client ID, API key or provider
registration configured by the application. The provider still operates the remote
signing service and the user still needs their personal signing identity. This is
not offline signing.

**The entire prepared PDF is sent directly to A-Trust.** PdfCraft asks for explicit
consent before starting the browser. Opening a PDF, placing a box, previewing it,
and listing providers do not send it. Do not upload confidential documents unless
you accept the provider's handling of them.

## In the desktop app

1. Open the PDF and choose **Digitally sign**. Draw the visible box, or select an
   existing empty signature field. Invisible signing remains available.
2. Choose **Sign with ID Austria…** in the digital-ID dialog.
3. Choose the appearance, reason, location and graphic. **Placement** also offers
   keyboard-editable page, left/top position and size in displayed-page points.
   **Visible signature** switches a new box on or off. An existing Fill & Sign
   signature or your own imported image can be used. A drawing is the visible
   appearance; the provider certificate supplies the cryptographic identity.
4. Accept the PDF transfer and choose **Sign**, then a new output filename.
5. Complete authorization in the provider browser. Enter credentials only there.
6. PdfCraft verifies the CMS and saves a new signed copy. The Signatures panel
   shows the actual certificate identity and its validation results.

The certificate is not known before authorization, so the visible box uses the
neutral text “Digital signature” instead of guessing a name. The distinguished-name
appearance is unavailable for this provider. The signed appearance cannot be
changed after the certificate arrives without invalidating the signature.

Cancel closes the operation's isolated browser and saves nothing. A document edit,
reload or closure during authorization invalidates its prepared result. A failed
output write retains the verified result: retry saving or choose a different new
filename without authorizing again. Cancelling then discards that retained result.

## CLI and MCP

Remote signing is disabled unless the host opts in:

```sh
pdfcraft-cli run --remote-signing --root /path/to/work --script steps.json
pdfcraft-cli mcp --remote-signing --root /path/to/work
```

Use the public tool table in this order (the returned document and job IDs replace
these examples):

```json
{"tool":"sign_providers","args":{}}
{"tool":"sign_begin","args":{"doc":1,"provider":"id-austria","allow_document_upload":true,"out":"signed.pdf","page":1,"rect":[20,20,180,80],"graphic":{"text":"Approved"}}}
{"tool":"sign_status","args":{"job":"returned-job-id"}}
{"tool":"sign_finish","args":{"job":"returned-job-id"}}
```

These are individual tool calls, not a complete script. For a CLI script, add
`"wait": true` to `sign_begin`: that call waits for the personal authorization and
saves the checked copy before the CLI exits. Without it, use a persistent MCP host
for polling; a job cannot be resumed by a separate CLI invocation. `sign_status` reports
`awaiting_authorization`, `ready_to_save`, or `failed`; poll it while the human
approves. `sign_finish` only writes a ready, verified result. An optional `out`
retries into another new file. `sign_cancel` discards a job. Jobs are process-local,
expire after ten minutes while awaiting authorization, and are not restored after
restarting. At most four jobs and one job per document are retained. Every input,
graphic and output path honors `--root`, including rechecking the output after
potentially lengthy authorization. Prepared documents are limited to 100 MiB.
Before upload, signing refuses no-changes certifications, re-certification of an
already signed document, and read-only signature fields. FieldMDP field-lock policy
validation remains outside this preview; inspect existing signatures and restrictions
before using it on previously signed forms.

An agent must obtain the user's acceptance of the document transfer before setting
`allow_document_upload`; it cannot perform the personal approval. A CLI running in
a terminal still needs a desktop display for the visible provider browser.

## Protocol, privacy and verification

The integration follows the public [Security Layer core and bindings](https://sl.eid.egiz.gv.at/konzept/securitylayer/spezifikation/20140114-en/core/Core.en.html)
and [A-Trust's Security Layer documentation](https://docs.a-trust.at/apis/SecurityLayer/).
It posts `XMLRequest` to the [published HTTPS endpoint](https://technology.a-sit.at/faqs/),
requesting a detached CMS with the exact prepared PDF and its excluded byte range.
The binding's `DataURL` is a server-to-server callback and is not used as a local
callback. No relay service is needed.

The installed browser uses a fresh temporary private profile and inherited process
pipes. No debugging TCP port is opened. Only the initial Security Layer request is
intercepted; login requests and page DOM are not read or filled. PdfCraft reads the
final top-level XML resource only from the fixed provider origins. Cookies, browser
protocol messages, raw provider errors and response URLs are not logged or saved.
The isolated browser and its helpers are terminated when the operation ends.

The PDF engine checks the exact detached digest, signature value, signer certificate,
ESS SigningCertificateV2 binding, supported modern digest algorithms and certificate
validity. It rejects substituted documents and certificates, duplicate attributes,
multiple signers, oversized or malformed XML/CMS and stale session results. It
retains the document's existing incremental content and signature permissions.

The provider path uses `adbe.pkcs7.detached`. It does **not** claim PAdES Baseline,
qualified-signature status, a qualified timestamp, or trusted EU certificate-chain
validation merely because the provider is ID Austria. Those claims need their own
profile and trust evidence. Existing signature validation reports its actual result.

## Other European identities and platform scope

There is no universal “all European eIDs” PDF-signing endpoint. Electronic
identification and qualified signing are different capabilities. The shared
prepare/complete engine API is provider-independent and can support documented
signing adapters without changing PDF placement. Existing local PKCS#12 and supported
OS signing identities remain available. This change does not add CSC, PKCS#11, a
national login federation, or a new trust-list service.

The transport contains Unix and Windows backends for Windows, macOS, Linux and
FreeBSD. Native builds and personal authorization must be qualified on each target
before claiming tested support. Web/WASM keeps building but offers no ID Austria
browser operation in this iteration.

A sandboxed package must be able to launch the installed browser with inherited
pipes. The current adapter does not add a Flatpak host helper or broaden sandbox
permissions; use a native desktop package for this preview. A browser installed
only outside the application sandbox is not automatically reachable.
