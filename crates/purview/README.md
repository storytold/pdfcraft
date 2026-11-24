# pdfcraft-purview

Microsoft Purview Information Protection integration for PdfCraft.

The lower-layer `pdfcraft-purview-policy` crate provides the Rust policy model and provider
contract; this crate provides the native MIP SDK adapter. Ordinary builds do not compile a native helper or include the authentication
dependencies, and detected protected files fail closed. The `mip-sdk` feature enables the
`mip-auth` Rust authentication feature and requires an official MIP SDK 1.18.x-compatible
package. Live use also requires an Entra public-client application registration.

The engine and automation layers use this crate's rights model; decryption is delegated to
Microsoft's SDK. The POC is not safe for real sensitive documents: recovery snapshots,
optimized output and read-only saves can write decrypted content without restoring protection.

## Native and dependency boundary

The MIP implementation uses Microsoft's native MIP SDK C++ library through a native helper.
It does not shell out to Azure CLI, require `az login`, reuse Azure CLI tokens, or use a
third-party MIP SDK wrapper. The Rust layer is limited to authentication state, PKCE browser
callback, profile/tenant JSON decoding, staging and the rights policy seam.

Official SDK packages are used for macOS, Windows, and Linux targets where Microsoft provides
them. The helper and SDK redistributables are staged and signed only in explicitly configured SDK
builds. Microsoft documents additional platform-specific native runtime requirements in its
[SDK setup guide](https://learn.microsoft.com/en-us/information-protection/develop/setup-configure-mip).

The optional authentication path uses Rust OAuth/PKCE, HTTPS with system trust roots, browser and JSON libraries, plus
the URL parser re-exported by OAuth. It reuses the workspace's `craft-mime` for bounded, strict
Base64URL decoding rather than maintaining another codec. The desktop already uses `craft-mime`;
a standalone authentication build also pulls in its other dependencies. OAuth and HTTPS may
themselves use Base64 transitively. The TLS provider also uses `ring`, which includes native C
and assembly; replacing cryptography locally would not make this safer. Windows staging uses the
existing workspace platform helper.

The native adapter stays at the platform layer, including Windows owner-only staging; the
engine depends only on the lower-layer policy contract. Frontends install the adapter.
