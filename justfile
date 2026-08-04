set shell := ["sh", "-cu"]

[private]
default:
    @just --list

# Run the app on a repo.
[group('run')]
run path="." *ARGS:
    cargo run -p sourcefour -- {{path}} {{ARGS}}

# Run the demo repo.
[group('run')]
demo *ARGS:
    cargo run -p sourcefour -- --demo {{ARGS}}

# Build.
[group('build')]
build:
    cargo build --workspace --locked

# Build release.
[group('build')]
build-release:
    cargo build --workspace --release --locked

# Remove build artifacts.
[group('build')]
clean:
    cargo clean

# Format.
[group('check')]
fmt:
    cargo fmt --all

# Check formatting.
[group('check')]
fmt-check:
    cargo fmt --all -- --check

# Lint.
[group('check')]
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Test, optionally filtered by name.
[group('check')]
test filter="":
    cargo nextest run --workspace --all-targets --locked {{filter}}

# Test one crate.
[group('check')]
test-crate crate:
    cargo nextest run -p {{crate}} --all-targets --locked

# Pre-push gate: everything CI runs.
[group('check')]
check: fmt-check lint test build build-release

# Build dist/Sourcefour.app.
[group('dist')]
package:
    ./scripts/package.sh

# Regenerate the app icon.
[group('dist')]
icon:
    python3 scripts/make-icon.py

# Show what a release would produce.
[group('dist')]
release-plan:
    dist plan

# Cut a release: set the version, tag it, and let CI build and tap it.
[group('dist')]
release version: check
    sed -i '' 's/^version = .*/version = "{{version}}"/' Cargo.toml
    cargo update --workspace
    git commit -am "release: v{{version}}"
    git tag -a "v{{version}}" -m "v{{version}}"
    git push origin HEAD --follow-tags
