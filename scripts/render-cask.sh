#!/usr/bin/env bash
# Render Casks/sourcefour.rb for the Homebrew tap from the released PKG checksum.
# Usage: render-cask.sh <version> <sha256>
set -euo pipefail

version="$1"
sha256="$2"

cat <<RUBY
cask "sourcefour" do
  version "${version}"
  sha256 "${sha256}"

  url "https://github.com/HelgeSverre/sourcefour/releases/download/v#{version}/sourcefour-universal-apple-darwin.pkg"
  name "Sourcefour"
  desc "Fast, native Git history browser you launch from your terminal"
  homepage "https://github.com/HelgeSverre/sourcefour"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: :ventura

  pkg "sourcefour-universal-apple-darwin.pkg"
  binary "/Applications/Sourcefour.app/Contents/MacOS/sourcefour"

  uninstall quit:    "no.lisethsolutions.sourcefour",
            pkgutil: "no.lisethsolutions.sourcefour"
end
RUBY
