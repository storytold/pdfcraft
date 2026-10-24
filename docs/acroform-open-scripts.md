# AcroForm scripts on opening

Automatic JavaScript execution for non-XFA AcroForms is disabled on desktop, in headless
`doc_open`/`doc_batch`, and in the browser. Opening does not evaluate document
JavaScripts, catalog OpenAction JavaScript chains or the opening page's additional
open JavaScript. Saved field values and appearances remain unchanged by scripts;
no script revision or viewer request is produced. Signed and certified AcroForms use
the same policy.

This deliberately defers #3062's automatic date/field initialization until
terminating, resource-bounded execution isolation is available. An AcroForm shows
an explanatory warning alongside its form banner without
decoding opening-script sources merely to decide whether to show it; the
notice survives refreshes during the session. Disabling JavaScript globally still
uses the existing preference behavior.

Signed and certified forms show the policy beside their signature-status notice;
the signature panel action remains available. The notice does not rewrite the PDF.

Explicit button and console execution, and scripts triggered by interactive field
edits, retain their existing behavior. Document-level helper scripts still load
for those explicit evaluations. Existing viewer consent checks remain in force.
This policy does not claim those evaluations have a hard wall-clock or memory bound.

## Why automatic execution is deferred

Boa 0.22 has loop/recursion limits and a cooperative async instruction budget.
These do not preempt parsing or a synchronous native operation, and do not impose
a heap limit. A timeout that merely stops waiting leaves computation running, so
it is not used here. No evaluator or abandoned worker is started for AcroForm open.

The existing CLI child-process check harness can terminate a child, but is not a
script service for desktop, engine embedders or the browser and has no portable
memory bound. Worker packaging, IPC, termination and resource limits remain future
work (#1847). The separate XFA execution path is unchanged; its existing timeout
must not be interpreted as a new hard execution/resource guarantee from this change.
Ordinary PDF parsing/rendering and interactive scripting are not hardened here.

## Compatibility

PR #2081 at `418fa15b0995574a61bfa45a4a95038ce0779852` already routes all
`app.openDoc` requests, including relative paths, through consent. Its Windows
`/C/path` and `/C:/path` normalization produces `C:/path`; UNC and URL inputs are
rejected by the local-document resolver. This policy prevents opening scripts
from producing OpenDoc requests when the changes are combined. No #2081 remote
branch or path-consent behavior is changed. The review concern is not presented
as a demonstrated zero-click exploit.

The retained `pdfcraft_forms::open_scripts` collection helper has no production
caller under this policy. Its opening-page selection currently handles explicit
destination arrays only: a named `/GoTo /D` falls back to page zero, for both the
legacy `/Dests` dictionary and `/Names /Dests` name tree. This verified collection
limitation must be resolved before that helper is used for automatic execution;
it does not cause scripts to execute while automatic opening-script execution is
disabled.

Regression coverage: engine `form_open_scripts_*` tests and UI
`opening_form_script_policy_notice_is_visible` and
`signed_form_shows_signature_and_opening_script_policy`, plus the existing explicit
button/console tests.
