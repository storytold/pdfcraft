# PR Proposal 0034: Retain Existing Appearances Without Subtype in Vector Printing

- **PR ID**: `PR-0034`
- **Issue Reference**: Fixes #3532, #3633
- **Severity**: Medium (Print Quality / Vector Fidelity)
- **Component**: `crates/print/src/lib.rs`, `crates/print/src/tests.rs`

## Description
When promoting existing field appearance streams to page XObjects for vector printing, appearance streams lacking an explicit `/Subtype` key in their stream dictionary were causing strict PDF consumers and Arlington model validators to drop or ignore the `/Do` invocation, leading to missing form artwork in printed output.

## Resolution
- In `crates/print/src/lib.rs`, check if an appearance stream's dictionary lacks `/Subtype`.
- If absent, create a separate stream copy with `Subtype = Object::name("Form")` added to its dictionary and register the promoted copy into `/Resources /XObject`.
- The source document's objects and streams remain untouched.
- Added unit test in `crates/print/src/tests.rs` verifying that appearances lacking `/Subtype` are promoted to Form XObjects upon imposition.
