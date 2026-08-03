# ADR 0002: Use a hybrid Git backend

## Status

Accepted.

## Decision

Use gix for repository discovery and read-only repository access: refs, worktrees, commit objects, history traversal, trees, changed files, and diffs. Use the installed `git` executable for explicit network or repository-changing operations, beginning with fetch.

Invoke Git with `Command` arguments and an explicit working directory (or Git's explicit `-C` argument). Do not invoke a shell. Operation runners must support streamed progress, cancellation, reaping, duplicate exclusion, and typed outcomes.

## Consequences

Read paths remain in-process and incremental, while operations use the user's existing Git credential, SSH, proxy, remote-helper, and configuration environment. This is an architectural boundary, not a claim that every gix or Git operation has already been tested.
