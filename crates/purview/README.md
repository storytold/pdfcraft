# pdfcraft-purview

Microsoft Purview Information Protection integration for PdfCraft.

The lower-layer `pdfcraft-purview-policy` crate provides the Rust policy model and provider
contract; this crate provides the native MIP SDK adapter. Ordinary builds do not compile a native helper or include the authentication
dependencies, and detected protected files fail closed. The `mip-sdk` feature enables the
`mip-auth` Rust authentication feature and requires an official MIP SDK 1.18.x-compatible
package. Live use also requires an Entra public-client application registration.

The engine and automation layers use this crate's rights model; decryption is delegated to
Microsoft's SDK. The POC is not ready for real sensitive documents. Protected saves and recovery
are verified, while optimization, digital signing and label/protection changes on protected files are disabled
until protection can be retained and tested against a live tenant.
A configured default label is not applied automatically; a required label must be selected and
applied before saving.
The POC does not renew access tokens or reauthenticate the same account while a protected
document is open. An expired token can prevent protected save and recovery; unsaved work remains
at risk. Use synthetic documents only until safe renewal and rights revalidation are verified.
Without the SDK, the fallback cannot reliably detect label-only metadata, especially in
compressed PDF streams; do not rely on an ordinary build to preserve or enforce those labels.
Typed PDF and XFA password-field values remain session-only: label operations stage a sanitized
copy and saves restore those values only in memory. A sanitized label operation may rewrite the
working PDF to avoid retaining password values in earlier revisions.

## Native and dependency boundary

The MIP implementation uses Microsoft's native MIP SDK C++ library through a native helper.
It does not shell out to Azure CLI, require `az login`, reuse Azure CLI tokens, or use a
third-party MIP SDK wrapper. The Rust layer is limited to authentication state, PKCE browser
callback, profile/tenant JSON decoding, staging and the rights policy seam.

Official SDK packages are used for macOS, Windows, and Linux targets where Microsoft provides
them. The helper and SDK redistributables are staged and signed only in explicitly configured SDK
builds. macOS, Windows, and Linux packages also carry the SDK's license terms and third-party
notices; this does not replace the approval required before distribution. Microsoft documents
additional platform-specific native runtime requirements in its
[SDK setup guide](https://learn.microsoft.com/en-us/information-protection/develop/setup-configure-mip).

The optional authentication path uses Rust OAuth/PKCE, HTTPS with system trust roots, browser and JSON libraries, plus
the URL parser re-exported by OAuth. It reuses the workspace's `craft-mime` for bounded, strict
Base64URL decoding rather than maintaining another codec. The desktop already uses `craft-mime`;
a standalone authentication build also pulls in its other dependencies. OAuth and HTTPS may
themselves use Base64 transitively. The TLS provider also uses `ring`, which includes native C
and assembly; replacing cryptography locally would not make this safer. Windows staging uses the
existing workspace platform helper.

The adapter checks that the protection and policy token responses identify the same Microsoft
account before pairing them. A missing or mismatched account stops sign-in, and a failed attempt
to add an account leaves an existing signed-in account available.
Label refresh runs outside the UI thread, reports SDK failures rather than treating them as an
empty policy, and discards a previous account's labels before loading the new account's policy.
PDFs requested during sign-in wait until the account change finishes before they are inspected.

The native adapter stays at the platform layer, including Windows owner-only staging; the
engine depends only on the lower-layer policy contract. Frontends install the adapter.
