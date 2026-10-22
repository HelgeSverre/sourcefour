set shell := ["sh", "-cu"]

[private]
default:
    @just --list

# Run app
[group('run')]
run path="." *ARGS:
    cargo run -p sourcefour -- {{path}} {{ARGS}}

# Run demo
[group('run')]
demo *ARGS:
    cargo run -p sourcefour -- --demo {{ARGS}}

# Build debug
[group('build')]
build:
    cargo build --workspace --locked

# Build release
[group('build')]
build-release:
    cargo build --workspace --release --locked

# Clean builds
[group('build')]
clean:
    cargo clean

# Format
[group('check')]
fmt:
    cargo fmt --all

# Check format
[group('check')]
fmt-check:
    cargo fmt --all -- --check

# Lint
[group('check')]
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Run tests
[group('check')]
test filter="":
    cargo nextest run --workspace --all-targets --locked {{filter}}

# Test crate
[group('check')]
test-crate crate:
    cargo nextest run -p {{crate}} --all-targets --locked

# Test packaging
[group('check')]
test-packaging:
    python3 scripts/test-macos-distribution.py
    python3 scripts/test-windows-distribution.py

# Run CI checks
[group('check')]
check: fmt-check lint test test-packaging build build-release

# Package app
[group('dist')]
package:
    ./scripts/package.sh

# Package unsigned PKG/ZIP
[group('dist')]
pkg:
    .github/scripts/package-macos-pkg.sh --unsigned --output-dir dist

# Install app
[group('dist')]
install: package uninstall
    cp -R dist/Sourcefour.app /Applications/
    @echo "Installed /Applications/Sourcefour.app"

# Uninstall app
[group('dist')]
uninstall:
    @[ ! -e /Applications/Sourcefour.app ] || rm -rf /Applications/Sourcefour.app 2>/dev/null || sudo rm -rf /Applications/Sourcefour.app
    @echo "Removed /Applications/Sourcefour.app"

# Reinstall app
[group('dist')]
reinstall: install

# Generate icons
[group('dist')]
icon:
    python3 scripts/make-icon.py

# Capture screenshots
[group('dist')]
screenshots:
    ./scripts/screenshots.sh

# Plan release
[group('dist')]
release-plan:
    dist plan

# Tag and publish release
[group('dist')]
release version: check
    sed -i '' 's/^version = .*/version = "{{version}}"/' Cargo.toml
    cargo update --workspace
    git commit -am "release: v{{version}}"
    git tag -a "v{{version}}" -m "v{{version}}"
    git push origin HEAD --follow-tags
