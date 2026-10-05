#!/usr/bin/env python3
"""Distribution contract tests. Apple/build tools are mocked, not trust-tested.

Run with `just test-packaging`. Real macOS acceptance: docs/macos-distribution.md.
"""

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[1]
NAME = "sourcefour-universal-apple-darwin"
APP = "target/universal-apple-darwin/release/Sourcefour.app"

# Exercise the actual shell script, including its error handling and ordering.
# The test fixture deliberately has no real signing credentials or toolchain.
MOCK = r'''#!/usr/bin/env python3
import json, os, pathlib, sys, zipfile
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ["CALL_LOG"], "a") as log:
    log.write(json.dumps([name, *args]) + "\n")
app = pathlib.Path("target/universal-apple-darwin/release/Sourcefour.app")
failure = os.environ.get("FAIL_AT", "")
if name == "uname":
    print("arm64")
elif name == "cargo":
    if args[0] == "pkgid": print("path+file:///fixture#sourcefour@0.1.6")
    if args[0] == "packager" and "--version" not in args:
        binary = app / "Contents/MacOS/sourcefour"
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_text("#!/bin/sh\nexit 0\n")
        binary.chmod(0o755)
elif name == "lipo" and args[0] == "-archs":
    print("x86_64 arm64")
elif name == "plutil":
    print("0.1.6" if args[1] == "CFBundleShortVersionString" else "13.0")
elif name == "codesign" and "--display" in args:
    print("TeamIdentifier=TESTTEAM")
elif name == "productbuild":
    pathlib.Path(args[-1]).write_bytes(b"pkg fixture")
elif name == "pkgutil" and args[0] == "--payload-files":
    print("./Sourcefour.app/Contents/MacOS/sourcefour")
elif name == "xcrun":
    if args[:2] == ["notarytool", "submit"]:
        print(json.dumps({"id": "test-submission", "status": "Invalid" if failure == "notary" else "Accepted"}))
    if args[:2] == ["stapler", "staple"] and args[-1].endswith(".app"):
        if failure == "app-staple": sys.exit(1)
        (app / "ticket-fixture").write_text("nested app ticket")
    if args[:2] == ["stapler", "validate"] and args[-1].endswith(".app"):
        if not (pathlib.Path(args[-1]) / "ticket-fixture").exists(): sys.exit(1)
elif name == "spctl":
    if failure == "extracted-gatekeeper" and args[-1].endswith(".app") and args[-1] != str(app):
        sys.exit(1)
elif name == "jq":
    data = json.loads(pathlib.Path(args[-1]).read_text())
    print(data.get("status" if ".status" in args[1] else "id", ""))
elif name == "ditto":
    if args[0] == "-c":
        source = pathlib.Path(args[-2])
        with zipfile.ZipFile(args[-1], "w") as archive:
            for path in source.rglob("*"):
                if path.is_file(): archive.write(path, path.relative_to(source.parent))
    else:
        with zipfile.ZipFile(args[-2]) as archive:
            archive.extractall(args[-1])
            for item in archive.infolist():
                pathlib.Path(args[-1], item.filename).chmod(item.external_attr >> 16)
'''


class DistributionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="sourcefour packaging ")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        script_dir = self.root / ".github/scripts"
        script_dir.mkdir(parents=True)
        self.script = script_dir / "package-macos-pkg.sh"
        shutil.copy2(ROOT / ".github/scripts/package-macos-pkg.sh", self.script)
        commands = self.root / "bin"
        commands.mkdir()
        mock = commands / "mock"
        mock.write_text(MOCK)
        mock.chmod(0o755)
        for name in "cargo codesign ditto jq lipo pkgutil plutil productbuild rustup spctl xcrun uname".split():
            (commands / name).symlink_to(mock)
        self.log = self.root / "calls.jsonl"
        self.env = dict(os.environ, PATH=f"{commands}:{os.environ['PATH']}", CALL_LOG=str(self.log))
        for name in ("APPLE_APPLICATION_SIGNING_IDENTITY", "APPLE_INSTALLER_SIGNING_IDENTITY",
                     "APPLE_NOTARY_ISSUER_ID", "APPLE_NOTARY_KEY_ID"):
            self.env[name] = "fixture"
        self.env["APPLE_TEAM_ID"] = "TESTTEAM"
        for name in ("APPLE_SIGNING_KEYCHAIN", "APPLE_NOTARY_KEY_PATH"):
            path = self.root / name
            path.touch()
            self.env[name] = str(path)

    def package(self, *args, failure=""):
        result = subprocess.run(["bash", str(self.script), "--output-dir", "output with spaces", *args],
                                cwd=self.root, env=dict(self.env, FAIL_AT=failure),
                                text=True, capture_output=True)
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        return result, calls

    def test_signed_artifacts_and_order(self):
        result, calls = self.package()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for extension in ("pkg", "zip"):
            artifact = self.root / "output with spaces" / f"{NAME}.{extension}"
            expected = hashlib.sha256(artifact.read_bytes()).hexdigest()
            self.assertEqual(Path(str(artifact) + ".sha256").read_text().split(), [expected, artifact.name])
        archive = self.root / "output with spaces" / f"{NAME}.zip"
        with zipfile.ZipFile(archive) as zipped:
            self.assertIn("Sourcefour.app/Contents/MacOS/sourcefour", zipped.namelist())
            self.assertIn("Sourcefour.app/ticket-fixture", zipped.namelist())
            self.assertTrue(all(name.startswith("Sourcefour.app/") for name in zipped.namelist()))
        submits = [call for call in calls if call[:3] == ["xcrun", "notarytool", "submit"]]
        self.assertEqual(len(submits), 1)
        self.assertTrue(submits[0][3].endswith(".pkg"))
        staple = calls.index(["xcrun", "stapler", "staple", APP])
        zip_index = next(i for i, call in enumerate(calls) if call[:2] == ["ditto", "-c"])
        self.assertLess(staple, zip_index)
        self.assertEqual(sum(call[:2] == ["codesign", "--force"] for call in calls[staple:]), 0)
        self.assertTrue(any(call[:3] == ["xcrun", "stapler", "validate"] for call in calls[zip_index:]))
        self.assertTrue(any(call[:4] == ["spctl", "--assess", "--type", "execute"] for call in calls[zip_index:]))

    def test_unsigned_skips_notarization_but_emits_both_artifacts(self):
        result, calls = self.package("--unsigned")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(any(call[0] in ("xcrun", "spctl") for call in calls))
        for extension in ("pkg", "zip"):
            self.assertTrue((self.root / "output with spaces" / f"{NAME}.{extension}.sha256").is_file())
        self.assertIn("must not be distributed", result.stderr)

    def test_failed_trust_checks_do_not_emit_release_checksums(self):
        for failure in ("notary", "app-staple", "extracted-gatekeeper"):
            with self.subTest(failure=failure):
                result, _ = self.package(failure=failure)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(list((self.root / "output with spaces").glob("*.sha256")))

    def test_cask_uses_zip_and_configured_appdir(self):
        sha = "a" * 64
        cask = subprocess.check_output(["bash", str(ROOT / "scripts/render-cask.sh"), "0.1.6", sha], text=True)
        self.assertIn(f'sha256 "{sha}"', cask)
        self.assertIn('version "0.1.6"', cask)
        self.assertIn(f'/v#{{version}}/{NAME}.zip"', cask)
        self.assertIn('app "Sourcefour.app"', cask)
        self.assertIn('binary "#{appdir}/Sourcefour.app/Contents/MacOS/sourcefour"', cask)
        self.assertIn('uninstall quit: "no.lisethsolutions.sourcefour"', cask)
        for forbidden in ('pkg ', 'pkgutil:', '/Applications', 'sudo', 'zap '):
            self.assertNotIn(forbidden, cask)

    def test_release_collects_both_formats_and_hashes_zip(self):
        upload = (ROOT / ".github/workflows/macos-pkg.yml").read_text()
        self.assertIn("name: artifacts-build-macos-pkg", upload)
        for extension in ("pkg", "pkg.sha256", "zip", "zip.sha256"):
            self.assertIn(f"target/distrib/*.{extension}", upload)
        release = (ROOT / ".github/workflows/release.yml").read_text().split("  publish-homebrew-cask:", 1)[1]
        self.assertIn(f'--pattern "{NAME}.zip"', release)
        self.assertIn(f"shasum -a 256 dist/{NAME}.zip", release)
        self.assertNotIn(f"{NAME}.pkg", release)


if __name__ == "__main__":
    unittest.main(verbosity=2)
