# Git & GitHub Collaboration Glossary

Canonical terminology for working with Git remotes, branching, pull requests, and conflict resolution in open source projects.

## Remotes and Locations

**Local repository**:
The Git database (`.git` directory) and working tree on your local machine containing your active branch, commits, and staged files.
_Avoid_: Local branch directory, my project folder

**Fork**:
A remote server-side clone of a repository hosted under your own GitHub account that gives you push permissions.
_Avoid_: Branch copy, cloud replica

**origin**:
The default alias name given to the remote repository from which a local repository was cloned or to which you push personal branches (usually your fork in open source).
_Avoid_: Master remote, parent repo

**upstream**:
The alias name pointing to the original, central repository from which you created your fork, used to fetch the latest changes made by other contributors.
_Avoid_: Main repo, original fork

## Collaboration & Integration

**Pull Request (PR)**:
A GitHub mechanism to propose changes from a branch in your repository or fork into a target branch of another repository, updating dynamically as new commits are pushed to the source branch.
_Avoid_: Merge request ticket, diff upload

**Merge**:
An integration strategy that combines histories from two branches by creating a new merge commit containing the combined changes with multiple parents.
_Avoid_: Code gluing, squashing

**Rebase**:
An integration strategy that replays your branch's unique commits on top of another base branch commit-by-commit to produce a linear history.
_Avoid_: Re-committing, branch swapping

**Merge conflict**:
A state where Git halts an integration because two branches modified the exact same lines of a file differently or one deleted a file that the other edited.
_Avoid_: Code crash, overwrite error
