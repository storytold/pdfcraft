# iPhone/iPad POC gaps and Apple publishing

Status, 2026-11-16: **upstream POC proposal, not an official release**. No Apple account enrollment, certificate
import, signing/upload, tester invitation, review submission or release is enabled.
The [Apple platform review](../../docs/apple-platform-review.md) is the central
ownership, build/package, readiness and UX-impact record; do not maintain a second
copy of that matrix here.

Related: [implementation POC](https://github.com/storytold/pdfcraft/pull/7549),
[mobile request](https://github.com/storytold/pdfcraft/issues/2085),
[Mac Store tracking](https://github.com/storytold/pdfcraft/issues/7540),
[shared CI issue](https://github.com/storytold/craft-common-actions/issues/259),
[shared workflow POC](https://github.com/storytold/craft-common-actions/pull/260),
[mobile RFC](https://github.com/storytold/craft-contrib/pull/60),
[existing shared host](https://github.com/storytold/craft-libs/pull/632), and
[native-boundary evidence](https://github.com/storytold/craft-libs/pull/1909).
The scoped host/distribution policy is proposed in
[storytold/craft-contrib#63](https://github.com/storytold/craft-contrib/pull/63).
These proposals are not adopted dependencies or standards.

## Unsupported controls for future review

This is the mobile shell's exposure inventory, not a claim that the Rust engine
lacks these capabilities. Add one supported workflow with existing engine/API
semantics and iPhone+iPad evidence before exposing its controls.

| Area | POC limitation |
| --- | --- |
| Edit | No document text/image editing, annotations/comments or full desktop edit panels |
| Convert/create/combine/extract | No mobile conversion, creation, merge/split or extraction workflow |
| E-Sign/forms/protection | No signature/form editing, signing UI or encryption configuration |
| Organize | Page selection and explicit clockwise rotation only; no thumbnails/reorder/insert/delete or batch editor |
| Document lifecycle | No undo/redo UI, original overwrite, tabs, multi-document session or crash restoration; copy-based saving and dirty-close confirmation remain |
| File history | Native Files recents/folders/providers, not another app history/bookmark database |
| Text/OCR/speech | Embedded page text is selectable/copyable; no OCR, aligned text overlay or custom read-aloud player |
| Accessibility/localization | Large-text/landscape operational checks exist; full VoiceOver speech, every split-view size and localization are not certified |
| Distribution | No official identity, production Store build counter, signed hardware, privacy audit, TestFlight or App Review result |

The existing Home/tools/search/pins, appearance, artwork and shared metadata work
is part of the upstream implementation proposal. Check the PR's source SHA and
exact validation results rather than treating older POC snapshots as current.

## Authorized Apple account setup

Simulator testing needs no paid membership. Personal Team device testing is
limited and does not provide TestFlight/App Store distribution.

If needed, Storytold's authorized legal entity should enroll as an organization.
The Account Holder owns eligibility, D-U-N-S/legal/seller information, agreements,
payment/fee waiver and team access. Do not accidentally use a contributor/fork
seller identity or assume the unregistered POC bundle ID is an authorized production ID.

1. Register the authorized app identifier/capabilities and appropriate signing
   identity/profile in Apple Developer **Certificates, Identifiers & Profiles**.
2. Create the matching **App Store Connect > Apps** record and authorized API access
   under **Users and Access > Integrations > App Store Connect API**.
3. Keep the `.p12` with private key, its password, provisioning profile and upload
   `.p8` in the organization's secret store. A `.cer` alone cannot sign; an API
   `.p8` is not a signing certificate. Base64 is not encryption.
4. After workflow acceptance, put proposed `IOS_DISTRIBUTION_P12_BASE64`,
   `IOS_DISTRIBUTION_P12_PASSWORD`, `IOS_APPSTORE_PROFILE_BASE64`,
   `ASC_API_PRIVATE_KEY_P8_BASE64` in **the app repository > Settings >
   Environments > ios-testflight > Environment secrets**. Keep team/bundle/app and
   API key/issuer IDs in environment variables. These names are a proposal,
   not an activated credential interface.
5. Restrict refs/reviewers, explicitly map secrets into an environment-bound shared
   job, use temporary keychains with failure cleanup, and keep production approval
   in a separate environment. Never expose secrets to PR/fork code or put them in
   shared-actions source. Apple's team-key permissions can span apps despite
   GitHub repository scoping.

macOS Developer ID and Mac App Store signing/profile requirements differ from
iOS; use the lane-specific setup in the shared proposal, not reused desktop
notarization secrets. No certificates/private keys belong in code, comments,
releases, artifacts or logs.

## Private flighting and public review

After an approved signed upload, wait for Apple processing and record the build
ID. Use private internal TestFlight groups first. Disable automatic pilot-group
distribution and audit existing automatic assignments before upload.

Internal testers are authorized App Store Connect users (currently up to 100);
invitation-only external testers are a separate option (up to 10,000). External
testing can require Beta App Review, particularly the first build. Public
TestFlight links are a further choice. Builds currently expire after 90 days.
Do not choose an internal-only restriction if the same build must reach external
testers or customers.

Complete privacy/support URLs, accurate screenshots/features, age rating,
cryptography/export-compliance and other required metadata before production
submission. Select the processed build and explicitly request **production App
Review**. Beta approval is not production approval. Prefer manual first public
release after approval. Keep upload, groups/invitations, external review, production
submission and release separate; none is performed by this POC.

Guideline 2.2 keeps demos/betas/trials out of the public Store and directs eligible
pre-release testing to TestFlight. TestFlight builds must still follow the
guidelines and be intended for public distribution. A POC disclaimer or changed
name is not a workaround for an incomplete product or permanent private delivery.

Official references and concrete remaining app blockers are linked from the
[central review](../../docs/apple-platform-review.md#official-sources).
