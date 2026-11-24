# Windows, Linux and macOS release-ref nightly: supervised manual go-live

**Enabled for supervised manual runs, not yet qualified.** Ported from Skylar's (`@skylartaylor`)
`storytold/photocraft#16594`. The producer (`nightly.yml`, main) and the signer
(`nightly-sign.yml`, release) keep only their exact context guards and run when one of the reviewed
manual actors dispatches them. There is no schedule yet. The installed policy
[`.github/nightly-signing-policy.json`](../.github/nightly-signing-policy.json) is the v3 policy.
Once `nightly.yml` lands on `main`, populate `producer_workflow_id` with its registration ID before
promoting the policy to `release`:

```bash
gh api repos/storytold/pdfcraft/actions/workflows/nightly.yml --jq .id
```

The callers pass no platform secrets. The unsigned reusable caller's maximum OIDC/attestation
permissions are for its isolated attester; its compiler narrows to contents and Actions read
only, with no OIDC, attestation write, release environment or platform signing secrets.
Shared tooling is pinned to `605d58766e8f536564da8fc81d3676ac86816dea`, the craft-common-actions
main commit that merged [#339](https://github.com/storytold/craft-common-actions/pull/339) (bounded
lineage compares and an issue reread at the end of candidate admission) on top of
[#342](https://github.com/storytold/craft-common-actions/pull/342) (policy v3, the `platforms` input
and the Linux and macOS legs). Both callers name it in `uses` and `tooling-revision`. A pin is
reviewed source availability, not provider, queue, native signing or crypto qualification.
Existing stable workflows are unchanged.

**First runs.** The first runs are manual dispatches in this order: `nightly.yml` on main (only a
listed manual actor passes the producer's plan step), then `nightly-sign.yml` on `release` with that
producer run ID and attempt, by a listed manual actor.

**Manual actors.** `bflatastic`, `dylanmtaylor`, `echelon`, `FredApps` and `skylartaylor`, in the
producer's `manual-actors` and the policy's `manual_actors`. The list mirrors the release-managers
team (id 19837241), leaving out `echelon-robot`; update it by PR in both places when the team
changes.

**Platform rows.** The producer caller lists
`platforms: '["windows-x86_64","linux-x86_64","macos-aarch64"]'` on its single job: one producer
run builds every listed platform for one all-or-nothing release (one failed or skipped platform
stops that night's release, Windows included), and the release-ref signer admits platforms only from
the protected policy, so there is no separate Linux or macOS caller job and the signer caller has
no platform input.

## Installed policy values and their sources

Read on 2026-11-24; re-read `producer_workflow_id` and anything else that may have moved before the
policy is promoted to `release`.

| Field | Value | Source |
|---|---|---|
| `repository_id` | `1398086162` | `gh api repos/storytold/pdfcraft --jq .id` |
| `producer_workflow_id` | `null` (populate after `nightly.yml` lands on `main`) | `gh api repos/storytold/pdfcraft/actions/workflows/nightly.yml --jq .id` |
| `producer_tooling_revision`, `signer_tooling_revision`, `producer_workflow_ref` | `605d5876…` (craft-common-actions #339) | The callers' pin |
| `manual_actors` / `automatic_signer_actors` | the five release managers / `[]` | release-managers team (id 19837241) members who can push; equals the producer caller's `manual-actors` |
| `max_age_seconds`, `max_artifact_bytes` | `86400`, `2147483648` | Go-live checklist |
| `platforms` | three rows | Stable Learning Machines identities (Windows Authenticode publisher subject, Linux OpenPGP primary fingerprint `2B76741B414AAD6CD33E631213A005C0B9D7C3ED`, macOS Developer ID team `DJ6XS33FX8`) |
| `candidate_policy.required_workflows` | CI (`ci.yml`, workflow `372163538`), 3 jobs | Workflow list API; jobs that succeeded on recent main push runs |
| `candidate_policy.sonar` | `storytold` / `storytold_pdfcraft`, app `12526` | `sonar-project.properties`; the SonarCloud check on main commits |

## Local regression checks

`python3 packaging/nightly-release-tests.py` verifies:
- `nightly.yml` and `nightly-sign.yml` have exact context guards (every `if:` form and continuation
  line is read), share one 40-hex-character `craft-common-actions` pin in `uses` and
  `tooling-revision`, have no registered schedule, and enforce the no-cache / no-platform-secret
  boundary.
- `nightly-sign.yml` preserves its plain root concurrency block (`pdfcraft-nightly-release`,
  `queue: max`, `cancel-in-progress: false`).
- `.github/nightly-signing-policy.json` matches the callers' pin, platforms, manual actors,
  `storytold/pdfcraft` repository identity, `ci.yml` main-push workflow, and `sonar-project.properties`.

## The app side: what a Nightly build is

The build is selected at compile time: `PDFCRAFT_BUILD_CHANNEL=nightly-v1` makes `build.rs` emit
`cfg(pdfcraft_nightly_v1)`, set the Windows `ProductName` to `PdfCraft Nightly` and
`ProductVersion` to the exact `PDFCRAFT_VERSION`, and compile in that identity. With
`PDFCRAFT_BUILD_CHANNEL` unset, empty or `stable` the app is the stable one; anything else is a
build error, and so is a Nightly build without `PDFCRAFT_VERSION`, `PDFCRAFT_BUILD_SHA` (40
lowercase hex, named by the version) and `PDFCRAFT_BUILD_DATE` (the version's own date).

A Nightly build is a separate app beside the stable one, with two identities:
`ai.storyteller.pdfcraft.nightly` is the package one (macOS bundle, Linux `.desktop`, the
Launcher's) and `ai.storyteller.pdfcraft.nightly-v1` is the window one (Wayland app id, the
Windows AUMID), so both taskbar entries and both icons stay distinct.

### Launch protocol

The Launcher starts it as `PdfCraft --launch-profile nightly-v1 --profile-root <absolute dir> -- [files…]`
(separate tokens; everything after `--` is a file). A Nightly build without a profile says "Open
PdfCraft Nightly from ArtCraft Launcher" (a native dialog on a desktop session) and exits 2; a
stable build refuses the profile. `--version`, `-V` and the Launcher's
`… --profile-root <dir> --version` are answered before the root is validated, logging starts, the
process identity is set, IPC is claimed or any window exists: they print
`pdfcraft <version> (nightly-v1)` and exit 0.

`PDFCRAFT_CONFIG_DIR` names the same root when a launcher does not use the two tokens (the Arch
packaging wrapper does this); it goes through exactly the same validation and fences, and an empty
value is no override. A command line that names `--launch-profile` or `--profile-root` is parsed
by the strict protocol alone.

### The data root

Every piece of user state lives inside the profile, and `profile_root` refuses a root that
overlaps any stable data folder (both `%APPDATA%`/`%LOCALAPPDATA%\PdfCraft`, the XDG
`pdfcraft` folders, `~/Library/{Application Support,Caches,Preferences,Saved Application
State}` entries, `/usr/bin/pdfcraft`, `/opt/pdfcraft`, the Flatpak folder, the install folder and
the running bundle), a portable copy (`portable.txt`/`PdfCraft.portable` beside a `pdfcraft`
executable, or a `PdfCraftData` component), `FLATPAK_ID`, a non-directory, a dangling or escaping
alias, or a folder it cannot write.

| State | Location inside the profile |
|---|---|
| `config.toml`, the settings store | `<profile>/config.toml` |
| eframe's `app.ron` (window and panel state) | `<profile>/app.ron` |
| Logs | `<profile>/logs` |
| Crash recovery and autosave | `<profile>/Recovery` |
| OCR Verify dictionaries | `<profile>/Dictionary` |
| The last Copy Image in Original Format | `<profile>/Clipboard` |
| Single-instance socket | `<profile>/Instance` |

`Recovery`, `Dictionary`, `Clipboard` and `Instance` must resolve inside the profile, because a
link at one of them would send Nightly's writes into another app's folder. The legacy
`app.ron`/`PrintCraft` import and the PrintCraft folder migration do not run in a Nightly launch,
and neither do the updater (`Help ▸ Check for updates` opens the releases page), the control
server, inherited `PDFCRAFT_CONTROL_*`, the stable agent link and the shared theme suite.

The Nightly session record is app-private health, not telemetry: `<profile>/nightly-session.json`
exists while a window runs and is removed by the controlled exit (`on_exit`, which is the only
exit path macOS has). A record found by the next start means the last one was killed or crashed;
the window says so once and clears it.

### Local checks

`python3 packaging/nightly-tests.py` runs the synthetic suites (launch/plist/signing/archive
policy); `.github/workflows/nightly-packaging.yml` builds the unsigned Windows ZIP, the Linux tar
and the macOS bundle behind the `full-ci` label or a dispatch, checking each the way the Launcher
unpacks it.
