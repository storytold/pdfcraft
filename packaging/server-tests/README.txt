Windows Server acceptance of the packaged Hot Folder OCR service
================================================================

Run only on an elevated, disposable GitHub Actions Windows Server runner.
acceptance.ps1 installs the opt-in MSI, changes a local service logon right,
creates an ephemeral non-admin account and an encrypted SMB share, and injects
process/filesystem faults. Never run it on a workstation or production server.
It refuses an existing PdfCraft installation, service or task directory.

The suite uses the actual packaged CLI, companion and OCR models. All PDFs and
images are generated locally from original synthetic text with PdfCraft's
embedded open fonts. No private scans, external corpus or system fonts are used.
No service account password is accepted from a user: CI generates one, masks it,
and supplies it directly to SCM through CIM, never through command-line arguments,
task JSON or logs. The account, share, service and prior service-logon-right
assignment are cleaned up independently, including after an earlier cleanup error.
Only bounded result/log files are uploaded; the private working directory is not.
Cancellation or host loss can bypass finally blocks: self-hosted hosts must be
ephemeral and destroyed after every run, even when the workflow is cancelled.

How to run
----------
1. Review and push the exact source commit, including this suite.
2. Finish a successful preview-installers.yml or release.yml workflow at that SHA.
   Its event must be push, workflow_dispatch or release, in this repository.
3. Dispatch hot-folder-server.yml on the same ref with installer_run_id,
   expected_sha (full 40 characters), and artifact_name. The default artifact name
   is pdfcraft-preview-windows-x64. The MSI SHA256 file must accompany the MSI.
4. Read both hot-folder-server-windows-2022 and hot-folder-server-windows-2025
   artifacts. A passing report.json and successful cleanup are required.

Example (documentation only; replace all placeholders):
  gh workflow run hot-folder-server.yml --ref REVIEWED_REF \
    -f installer_run_id=SUCCESSFUL_RUN_ID -f expected_sha=FULL_COMMIT_SHA

Before this new workflow reaches the default branch, an existing installer
workflow can invoke it as a reusable workflow in a SECOND manual dispatch, passing
the previous successful installer run ID and skipping its own build jobs. Calling
it from the same still-running installer run is intentionally rejected. All jobs
and the script require the original event to be workflow_dispatch; workflow_call
does not provide a bypass for pull requests, pushes or other automatic callers.

The preview workflow provides that second mode explicitly:
  gh workflow run preview-installers.yml --ref REVIEWED_REF \
    -f mode=server-acceptance -f installer_run_id=SUCCESSFUL_RUN_ID
It skips all installer/processing builds, passes its own exact github.sha as the
expected_sha, and invokes the local reusable workflow from that same commit.
Do not advance REVIEWED_REF between the installer run and acceptance dispatch.
The default mode remains build; pushes never launch the acceptance suite.

Windows Server 2019
-------------------
GitHub retired its hosted Windows Server 2019 image on 30 June 2025. The optional
manual job requires a dedicated, elevated, disposable x64 Server 2019 runner with
labels self-hosted, Windows, X64, pdfcraft-server-2019-disposable. Configure the
hot-folder-server-2019 environment with REQUIRED REVIEWERS before enabling it;
declaring an environment in YAML alone does not enforce review. Keep this runner
out of general runner groups and never run unreviewed PR code on it. Explicitly set
include_server_2019=true and server_2019_confirmation=DISPOSABLE SERVER 2019.
Until that real run passes, Server 2019 is prepared for testing, not runtime-proven.

What a passing run proves
------------------------
- Fresh opt-in MSI registration defaults to LocalService, starts automatically,
  creates no enabled tasks, and protects data/secrets with the stable service SID.
- Actual OCR creates searchable PDF and TXT from an image-only PDF; original bytes
  are preserved. The PDF is reopened and searched through the packaged CLI.
- A write-denied output retains its input until both outputs succeed. Another local
  task continues, and the failed task recovers automatically without manual retry.
- A malformed single PDF is retained while a later valid document is processed.
- Killing the real service during a CPU-active OCR child terminates that child;
  SCM restarts the service and the interrupted work completes without duplicate
  outputs. Stopping during OCR is bounded. A controlled initialization Err also
  recovers through SCM, whose configured delay is 60 seconds.
- An ephemeral non-admin service account authenticates to an encryption-required
  loopback SMB share. Removing that share isolates the failure to its task; a local
  task continues and the UNC task recovers automatically after share recreation.
- A same-version major upgrade preserves task bytes, all executable bytes and the
  SCM account/password, proven by subsequent authenticated SMB OCR without supplying
  the password again. Its test-only MSI copy changes ProductCode and PackageCode
  GUIDs but not embedded binaries. This modifies any original signature; it is never
  published or delivered as an installer. A separate probe installer test checks
  replacement with changed binary payload, including equal-version replacement.

Limits: this is authenticated loopback SMB, not certification of every NAS vendor,
domain controller, gMSA deployment or WAN failure mode. It does not perform a full
server reboot. It verifies automatic startup configuration and real SCM restart,
not a reboot result. Server versions are reported only after their actual runs.

Local validation without installation
------------------------------------
  pwsh -NoProfile -File packaging/server-tests/check.ps1
  pwsh -NoProfile -File packaging/server-tests/check.ps1 -Cli PATH/TO/pdfcraft-cli.exe
  actionlint -config-file packaging/server-tests/actionlint.yaml \
    .github/workflows/hot-folder-server.yml
check.ps1 parses scripts, exercises confinement, preserves empty/single/multiple
CLI-script arrays, rejects missing or changed originals and malformed ProductCodes,
preserves primary failures through cleanup, and mocks the GitHub metadata verifier.
On Windows it also runs check-msi-database.ps1: a tiny synthetic MSI database proves
that upgrade helpers emit exactly one GUID, persist the new identity, preserve the
source and release storage locks before returning. It never opens an installation
session or calls msiexec. The optional -Cli check
also executes those scripts with the existing CLI, using only the read-only
ocr_status tool. It never runs acceptance.ps1 or accesses a real service/account/share.

Primary references
------------------
Hosted images and available labels:
  https://github.com/actions/runner-images
Windows Server 2019 hosted runner retirement:
  https://github.com/actions/runner-images/issues/12045
LocalService presents anonymous network credentials (hence the separate SMB phase):
  https://learn.microsoft.com/en-us/windows/win32/services/localservice-account
SCM account/password configuration using Win32_Service.Change:
  https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/change-method-in-class-win32-service
SMB encryption-required share:
  https://learn.microsoft.com/en-us/powershell/module/smbshare/new-smbshare
Reusable workflow event context:
  https://docs.github.com/en/actions/reference/workflows-and-actions/reusing-workflow-configurations

Portable harness checks
-----------------------
Run `pwsh -NoProfile -File packaging/server-tests/check.ps1` on Windows or Linux.
It checks path confinement using native separators and case rules, including
trailing separators, filesystem roots, sibling prefixes and parent traversal.
It does not run an installer or start a service. Unix checks also reject
different-case roots and backslash sibling names.
