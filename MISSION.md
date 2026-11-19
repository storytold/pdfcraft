# Mission: Git & GitHub Open Source Contribution Workflow

## Why
Contribute confidently to open-source projects like `pdfcraft` by understanding the relationship between local repositories, forks, and upstream branches. Master the workflow of updating pull requests, syncing with upstream changes, and resolving merge conflicts without fear of losing work or breaking commit history.

## Success looks like
- Inspect any local repository and instantly identify whether it pushes to a personal fork or an upstream repo.
- Commit new changes locally and update an active Pull Request on GitHub with zero friction.
- Pull in latest changes from the upstream maintainers (`upstream/main`) into a PR branch cleanly.
- Confidently diagnose, edit, and resolve merge conflicts, verify the build, and finish the merge/rebase.

## Constraints
- Working directly in the existing `pdfcraft` local development environment on Linux.
- Using standard `git` CLI tools and code editor inspection.
- Adhering to the repository's strict contributor rules (e.g. `AGENTS.md`).

## Out of scope
- Advanced Git plumbing internals (packfiles, index binary formats).
- Git hosting server administration or custom Git hooks design.
- Force-pushing to shared protected branches (like upstream `main`).
