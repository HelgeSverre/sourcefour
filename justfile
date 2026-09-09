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

# Build an unsigned dist/*.pkg, to check the installer without Apple's certs.
[group('dist')]
pkg:
    .github/scripts/package-macos-pkg.sh --unsigned --output-dir dist

# Install the packaged app into /Applications.
[group('dist')]
install: package
    rm -rf /Applications/Sourcefour.app
    cp -R dist/Sourcefour.app /Applications/
    @echo "Installed /Applications/Sourcefour.app"

# Remove the installed app. Settings in ~/Library/Application Support stay.
[group('dist')]
uninstall:
    rm -rf /Applications/Sourcefour.app
    @echo "Removed /Applications/Sourcefour.app"

# Reinstall the packaged app.
[group('dist')]
reinstall:
    just uninstall && just install

# Regenerate the app icon in every packaging format.
[group('dist')]
icon:
    python3 scripts/make-icon.py

# Recapture every screenshot the website ships.
[group('dist')]
screenshots:
    ./scripts/screenshots.sh

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
