# Sourcefour

A fast, native Git history browser you launch from your terminal. Press one
command inside a repository and get the useful half of SourceTree: the commit
graph, refs, commit details, and diffs — nothing to configure, nothing to log
into.

Built with [GPUI](https://www.gpui.rs) for GPU-rendered native UI and
[gitoxide](https://github.com/GitoxideLabs/gitoxide) for reading repositories.

## Install

```sh
brew install helgesverre/tap/sourcefour
```

Or build from source:

```sh
cargo install --git https://github.com/HelgeSverre/sourcefour sourcefour
```

## Use

```sh
sourcefour            # browse the repository containing the working directory
sourcefour ~/code/foo # browse a specific repository
sourcefour --demo     # browse a generated demo repository
```

## Development

Everything runs through [`just`](https://github.com/casey/just); `just --list`
shows the recipes, grouped into run, build, check, and dist.

```sh
just run      # launch the app on the current repository
just test     # cargo nextest across the workspace
just check    # the full pre-push gate — the same one CI runs
just package  # build dist/Sourcefour.app (ad-hoc signed, local use)
```

## Releasing

CI runs format, lint, tests, and locked debug/release builds on macOS and Linux
for every push and pull request.

Releases are cut by [cargo-dist](https://opensource.axo.dev/cargo-dist/).
Pushing a version tag runs the test suite first, then builds macOS binaries for
both architectures, publishes a GitHub Release, and updates the Homebrew
formula in [`helgesverre/homebrew-tap`](https://github.com/HelgeSverre/homebrew-tap).

```sh
just release 0.2.0
```

Requires a `HOMEBREW_TAP_TOKEN` repository secret: a fine-grained personal
access token scoped to `HelgeSverre/homebrew-tap` with **Contents: Read and
write**.

## License

MIT or Apache-2.0, at your option.
