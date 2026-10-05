# macOS distribution and Homebrew acceptance checks

The release ships the existing universal `Sourcefour.app` twice: inside the
signed PKG and at the root of `sourcefour-universal-apple-darwin.zip`. The cask
uses only the ZIP; the PKG download and its checksum remain available.
Publish the generated tap cask only alongside the release containing the ZIP.
Do not repoint the 0.1.5 cask at a ZIP that does not exist.

## Signing and Gatekeeper

The binary and app are Developer ID Application signed with hardened runtime
and secure timestamps. The PKG is Developer ID Installer signed and submitted
to Apple's notary service. Apple [creates tickets for nested code as well as
the submitted container](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow).
Thus this single PKG submission also covers the unchanged signed app.

The packaging script staples and validates the PKG, then **staples and validates
the app itself**. It archives that app with `ditto --sequesterRsrc --keepParent`,
extracts the ZIP into a temporary directory, and verifies the extracted app's
signature, stapled ticket, and Gatekeeper execution assessment. A ZIP cannot
itself carry a stapled ticket. No re-signing or bundle modification is allowed
between notarization and archiving, apart from stapling. The already-built PKG
is unchanged by stapling the source app after packaging.

Any failure aborts the packaging job before upload or checksum publication.
`--unsigned` emits both formats for local layout tests only; it deliberately
skips trust checks and must never be used for a release.

`just test-packaging` runs portable contract tests with mocked Apple/build
commands. They exercise output layout, checksums, command ordering, rejected
notarization, failed app stapling, failed extracted-app Gatekeeper assessment,
the cask generator, and release wiring. They do **not** establish actual
notarization, offline launch, or Homebrew lifecycle behavior.

## Signed artifact checks (macOS 13+)

Run the `macOS Package` workflow on the candidate branch, download its
`artifacts-build-macos-pkg` artifact, and extract the Actions wrapper into a
working directory. The workflow does not publish a release or update the tap.
In that directory:

```sh
shasum -a 256 -c sourcefour-universal-apple-darwin.pkg.sha256
shasum -a 256 -c sourcefour-universal-apple-darwin.zip.sha256
pkgutil --check-signature sourcefour-universal-apple-darwin.pkg
xcrun stapler validate sourcefour-universal-apple-darwin.pkg
spctl --assess --type install --verbose=4 sourcefour-universal-apple-darwin.pkg

check_dir="$(mktemp -d)"
ditto -x -k sourcefour-universal-apple-darwin.zip "$check_dir"
app="$check_dir/Sourcefour.app"
test -x "$app/Contents/MacOS/sourcefour"
lipo -archs "$app/Contents/MacOS/sourcefour" # must include arm64 and x86_64
codesign --verify --deep --strict --verbose=2 "$app"
codesign --display --verbose=4 "$app" # check expected TeamIdentifier + runtime
xcrun stapler validate "$app"
spctl --assess --type execute --verbose=4 "$app"
```

Also download the ZIP using Safari on a clean test Mac/VM and extract it in
Finder. Verify quarantine is present with `xattr -p com.apple.quarantine
Sourcefour.app`. Disconnect networking **before the first launch**, then open
the app in Finder. Expect the normal downloaded-app confirmation, not an
unidentified-developer or damaged-app block. Test on Apple Silicon and Intel
and record macOS versions. Do not remove quarantine or disable Gatekeeper.
An online `spctl` result alone is not proof of offline first-launch behavior.

## Fresh cask install and uninstall

Use a disposable macOS account/VM with a writable Homebrew prefix and current
Homebrew. No Sourcefour app, cask, formula, CLI link, or PKG receipt should
already exist. Formula and cask both expose `sourcefour`; remove the formula
before this test rather than forcing a conflicting CLI link.

Test against a release containing the ZIP and its generated cask (a candidate
release/test tap is fine). Review `brew cat --cask helgesverre/tap/sourcefour`
first: it must contain `.zip`, `app`, `#{appdir}` and no `pkg` or `pkgutil`.
Homebrew documents [writable app directories and `HOMEBREW_NO_SUDO`](https://docs.brew.sh/Installation#running-without-sudo).

```sh
set -euo pipefail
export HOMEBREW_NO_SUDO=1
mkdir -p "$HOME/Applications"
appdir="$HOME/Applications"
prefix="$(brew --prefix)"
test -w "$prefix" && test -w "$appdir"
brew install --cask --verbose --appdir="$appdir" \
  helgesverre/tap/sourcefour 2>&1 | tee /tmp/sourcefour-install.log
test -d "$appdir/Sourcefour.app"
test "$(readlink "$prefix/bin/sourcefour")" = "$appdir/Sourcefour.app/Contents/MacOS/sourcefour"
"$prefix/bin/sourcefour" --help
! pkgutil --pkg-info no.lisethsolutions.sourcefour
! grep -E 'Running installer|/usr/sbin/installer|/usr/bin/sudo' /tmp/sourcefour-install.log
open "$appdir/Sourcefour.app" --args --demo
```

Confirm the GUI renders, change a setting, and quit with Cmd+Q before continuing.
In this disposable account, snapshot the existing settings, then uninstall:

```sh
support="$HOME/Library/Application Support/Sourcefour"
settings_copy="$(mktemp -d)/Sourcefour"
ditto "$support" "$settings_copy"
brew uninstall --cask --verbose helgesverre/tap/sourcefour
test ! -e "$appdir/Sourcefour.app"
test ! -L "$prefix/bin/sourcefour"
diff -r "$settings_copy" "$support"
```

Expect no password prompt, no installer invocation and no new PKG receipt.
Do not use `--zap`. Repeat with `appdir=/Applications` and
`--appdir=/Applications` **only when that directory is writable**. Also test a
custom app directory containing spaces. A protected directory can still need
elevation; the cask does not bypass filesystem permissions.

## ZIP-to-ZIP upgrades

Install the older of two signed ZIP-based releases through a test tap. Quit
the app and snapshot settings as above. Update that test tap's generated cask
to the newer real release version and ZIP SHA-256. Do not simulate an upgrade
by merely reinstalling the same version.

```sh
export HOMEBREW_NO_SUDO=1
brew upgrade --cask --verbose sourcefour
test "$(readlink "$prefix/bin/sourcefour")" = "$appdir/Sourcefour.app/Contents/MacOS/sourcefour"
plutil -extract CFBundleShortVersionString raw -o - "$appdir/Sourcefour.app/Contents/Info.plist"
diff -r "$settings_copy" "$support"
"$prefix/bin/sourcefour" --help
open "$appdir/Sourcefour.app" --args --demo
```

Require the new bundle version, working CLI and GUI, unchanged settings before
launch, no elevation, and no PKG receipt. Quit and uninstall the upgraded cask;
require removal of the new app and CLI link while settings remain.

## Migration from the legacy 0.1.5 PKG

Use a separate VM snapshot with the real 0.1.5 PKG cask installed. Record
`brew list --cask --versions sourcefour`,
`pkgutil --pkg-info no.lisethsolutions.sourcefour`, the app/CLI ownership, and
a copy of the settings. Keep the old cask's installed metadata intact.

1. Quit Sourcefour. Point the test tap at the new ZIP cask and attempt
   `HOMEBREW_NO_SUDO=1 brew upgrade --cask --verbose sourcefour`. Record the
   outcome. Root-owned legacy files/receipt can prevent this transition.
2. Restore the snapshot before testing with elevation allowed. Run
   `brew upgrade --cask --verbose sourcefour` as the Homebrew user, **not**
   `sudo brew`. Record any one-time prompt for legacy cleanup. Homebrew uses
   installed cask metadata for uninstall; see its
   [installer implementation](https://docs.brew.sh/rubydoc/Cask/Installer.html).
3. Verify the old receipt is gone, the installed app is the new version, the
   CLI points to the configured app directory, and settings are unchanged.
   Then perform uninstall/reinstall with `HOMEBREW_NO_SUDO=1` to confirm future
   ZIP operations do not inherit the privileged cleanup requirement.

If upgrading cannot cleanly remove the old installation, the explicit migration
path is `brew uninstall --cask sourcefour` while the old installed metadata is
available (allow its one-time elevation), then install the new cask with
`HOMEBREW_NO_SUDO=1` and the chosen `--appdir`. Do not delete Caskroom metadata
to suppress old uninstall actions, and do not use `--force` to hide conflicts.

For a PKG installed directly outside Homebrew, there is no managed cask to
uninstall. Inspect `pkgutil --files no.lisethsolutions.sourcefour`, quit the
app, and have an administrator remove only that legacy app and forget its
receipt before installing the cask. `pkgutil --forget` alone does not remove
files. Never remove `~/Library/Application Support/Sourcefour` as part of
migration. Avoid automatic privileged migration hooks in the new cask.

## Record before releasing

Record commit/tag, macOS versions/architectures, Homebrew version, signing team,
artifact hashes, and pass/fail for: online/offline launch, fresh user/default
appdir installs, ZIP upgrade, uninstall/settings retention, legacy cask
upgrade and direct-PKG migration. Keep legacy elevation results separate from
fresh ZIP results. Pending macOS checks must not be reported as passing merely
because the portable contract tests passed.

## Validation record — 2026-10-05

Candidate packaging run [37277978028](https://github.com/HelgeSverre/sourcefour/actions/runs/37277978028)
built commit `093d236c58fc6d4284f7c0c304505eef76a3fd20` (bundle version
0.1.5) with the new ZIP pipeline. Subsequent changes before release update
only tests, lint expectations, and documentation.

Validated locally on macOS 15.6 (24G84), Apple Silicon, Homebrew
7.0.6-70-gce46735:

- PKG and ZIP SHA-256 matched the workflow artifacts.
- PKG signature, notarization ticket, and installer Gatekeeper assessment passed.
- The extracted app contains arm64 and x86_64 slices. Its signature, hardened
  runtime, stapled ticket, execution Gatekeeper assessment, and
  `syspolicy_check distribution` passed. Signing team: `9Z2L5FBZS3`
  (Liseth Solutions AS).
- Fresh ZIP cask installs and uninstalls passed in `~/Applications`, writable
  `/Applications`, and a custom application path containing spaces, using a
  temporary local test tap with the actual ZIP. All Homebrew lifecycle commands
  used `HOMEBREW_NO_SUDO=1`; logs contained no privileged installer invocation.
- CLI links resolved into each selected app directory, `--help` passed, and
  the signed app's demo rendered through Launch Services. The x86_64 CLI also
  ran under Rosetta.
- Uninstall removed the app and CLI while preserving all support-file hashes.
  The pre-existing 0.1.4 PKG receipt was unchanged by ZIP installs/uninstalls.
- `just check` passed with Rust 1.99: 411 Rust tests, five portable packaging
  tests, formatting, lint, and locked debug/release builds.
- The [three-platform CI matrix](https://github.com/HelgeSverre/sourcefour/actions/runs/37278313238)
  passed at `c160a54ac14a4eeacbed536cfeabba8abd2a968d`.

Candidate hashes:

```text
6a7ff96ded23ad343ee48adec2e72a4b9b3eeef2b8b0429e5e7bc72be63f0a3c  sourcefour-universal-apple-darwin.pkg
c7a8dbf634def4ce7f263b5e5e1e6f974353155f72ee5bf3e6d45f37942c5e04  sourcefour-universal-apple-darwin.zip
```

Not exercised in this environment: disconnected/quarantined first launch,
physical Intel GUI launch, and legacy PKG cask/direct-PKG migration in a
separate clean VM. These remain coverage limits; Rosetta and online trust
checks do not establish those results. ZIP-to-ZIP upgrade will be checked
against the released 0.1.6 ZIP and recorded separately.
