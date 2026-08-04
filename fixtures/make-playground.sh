#!/bin/bash
# Rebuilds fixtures/playground: a real Git repository for manual testing.
#
# The playground is gitignored and disposable — run this script whenever you
# want a fresh one. It exercises everything the interface renders: parallel
# fixture/* branches, no-ff and octopus merges, tags, meaty file edits for
# diffs, a rename, a deletion, a binary blob, and an oversized file that
# trips the diff safety cap.
set -euo pipefail

root="$(cd "$(dirname "$0")" && pwd)/playground"
rm -rf "$root"
mkdir -p "$root"
cd "$root"

export GIT_AUTHOR_NAME="Playground Author"
export GIT_AUTHOR_EMAIL="author@playground.invalid"
export GIT_COMMITTER_NAME="Playground Author"
export GIT_COMMITTER_EMAIL="author@playground.invalid"

git init -q -b main

commit() { git add -A >/dev/null && git commit -q -m "$1"; }

# ── mainline groundwork ────────────────────────────────────────────────
cat > README.md <<'EOF'
# Playground

A disposable repository for exercising Sourcefour by hand.
EOF
commit "docs: introduce the playground"

mkdir -p src
cat > src/app.rs <<'EOF'
fn main() {
    println!("hello playground");
}
EOF
cat > src/lib.rs <<'EOF'
pub fn answer() -> u32 {
    41
}
EOF
commit "feat: initial application skeleton"

cat > src/lib.rs <<'EOF'
/// The canonical answer, now correct.
pub fn answer() -> u32 {
    42
}

pub fn double(value: u32) -> u32 {
    value * 2
}
EOF
commit "fix: correct the answer and add doubling"
git tag v0.1.0

# ── two parallel feature branches ──────────────────────────────────────
git checkout -q -b fixture/feature-a
cat > src/parser.rs <<'EOF'
pub struct Parser {
    position: usize,
}

impl Parser {
    pub fn new() -> Self {
        Self { position: 0 }
    }
}
EOF
commit "feat(parser): scaffold the parser"
cat >> src/parser.rs <<'EOF'

impl Parser {
    pub fn advance(&mut self) {
        self.position += 1;
    }
}
EOF
commit "feat(parser): advance through input"

git checkout -q main
git checkout -q -b fixture/feature-b
cat > src/render.rs <<'EOF'
pub fn render(frame: u64) -> String {
    format!("frame {frame}")
}
EOF
commit "feat(render): render frames"
sed -i '' 's/frame {frame}/frame #{frame}/' src/render.rs
commit "style(render): number frames with a hash"

git checkout -q main
cat > CHANGELOG.md <<'EOF'
# Changelog

- groundwork
EOF
commit "docs: start a changelog"

git merge -q --no-ff fixture/feature-a -m "Merge branch 'fixture/feature-a'"
git merge -q --no-ff fixture/feature-b -m "Merge branch 'fixture/feature-b'"

# ── rename, delete, binary, oversized ──────────────────────────────────
git mv src/app.rs src/main.rs
commit "refactor: rename app.rs to main.rs"

rm CHANGELOG.md
commit "docs: drop the changelog experiment"

printf 'PNG\x00\x01\x02\x03\x04binary-not-text\x00\xff' > logo.bin
commit "assets: add a binary logo"

seq 1 25000 | sed 's/^/line /' > generated.txt
commit "chore: vendor a generated file"
sed -i '' 's/^line 2$/line two/' generated.txt
commit "chore: regenerate the generated file"
git tag v0.2.0

# ── an octopus merge and an unmerged branch for extra lanes ────────────
git checkout -q -b fixture/octo-a v0.1.0
echo "octopus arm a" > arm-a.txt
commit "feat: octopus arm a"
git checkout -q -b fixture/octo-b v0.1.0
echo "octopus arm b" > arm-b.txt
commit "feat: octopus arm b"
git checkout -q main
git merge -q fixture/octo-a fixture/octo-b -m "Merge arms 'fixture/octo-a' and 'fixture/octo-b'"

git checkout -q -b fixture/wip v0.2.0
cat > src/wip.rs <<'EOF'
// Not merged anywhere: keeps a lane running in the all-refs view.
pub fn unfinished() {}
EOF
commit "wip: an unmerged line of work"
git checkout -q main

echo "Playground ready at $root"
git log --oneline --graph --all | head -20
