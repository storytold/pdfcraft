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
