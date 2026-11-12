# Source licensing self-check

Run `cargo xtask license-check` to validate the checked-out source, offline. The command
does not download assets, fetch models, inspect installers, or generate dependency notices.
It reuses `cargo xtask assets` for attribution, asset policy and locked dependency versions,
then checks the documents recorded in `LICENSES.toml`.

Each document has a repository-relative source, a unique portable logical destination and
the SHA-256 of its audited text. Destinations are validation metadata, not instructions to
copy files into packages. Required project documents and every asset's `licence_file` must
be mapped. Every dependency-bundled asset must have a document mapping matching its existing
`ATTRIBUTION.toml` licence expression. Foxit has an explicit required document. Licence
texts for egui's default fonts and CMaps are retained in `licenses/`, with their pinned
upstream origins recorded in the manifest. Existing font and asset texts stay in place.

Missing files, empty or invalid text, changed hashes, duplicate mappings, case-insensitive
destination collisions, parent-file conflicts and unsafe paths fail the check. Source
symlinks must resolve inside the checkout. OCR models need complete attribution and valid
hashes and destination names; the model binaries and downloaded licence texts need not be
present. The optional craft-fonts checkout remains governed by its own attribution and
the existing asset validator; this command does not audit that external checkout.

When changing attribution, edit `ATTRIBUTION.toml`, run `cargo xtask assets --write`. Generated attribution consistency remains checked only
by the existing asset validator. When changing licence or notice text, re-audit
its provenance before updating the corresponding hash. Do not replace a licence with
guessed terms or update a hash merely to silence a failure. `LICENSES.toml` is validated
with unknown fields rejected.

The existing Linux source-validation step in `.github/workflows/ci.yml` and `cargo xtask ci`
run `license-check` in place of `assets`; the asset validator runs once within that command.
This Stage 1 check establishes source-document consistency, not completed-package
compliance or legal sufficiency. Installer inspection, dependency-notice generation and
font-delivery packaging changes remain separate work.
