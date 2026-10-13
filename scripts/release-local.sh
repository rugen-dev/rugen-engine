#!/usr/bin/env bash
# Build a release on this machine and publish it on GitHub, without Actions.
#
#   scripts/release-local.sh 0.6.2 notes.md
#
# Builds the Linux packages (portable tarball, .deb, .rpm), tags v<version>,
# pushes main and the tag, and creates the GitHub release with notes.md on
# top of the download table. The release is a pre-release (Linux only) unless
# RUGEN_FULL=1, in which case the Windows and macOS packages are added by a
# manual run of .github/workflows/release.yml on the tag.
#
# The version must already be in Cargo.toml and committed. The official key
# is the server's Ed25519 public key (`key` in https://play.rugen.dev/health);
# it is public, but a release without it trusts the development key.
set -euo pipefail

v=${1:?usage: release-local.sh <version> <notes.md>}
notes=${2:?usage: release-local.sh <version> <notes.md>}
tag=v$v
repo=rugen-dev/rugen-engine
cd "$(git rev-parse --show-toplevel)"

# the same public key CI uses (repository variable)
: "${RUGEN_OFFICIAL_KEY:=$(gh variable get RUGEN_OFFICIAL_KEY --repo $repo)}"
export RUGEN_OFFICIAL_KEY
[ -n "$RUGEN_OFFICIAL_KEY" ] || { echo "set RUGEN_OFFICIAL_KEY"; exit 1; }
grep -q "^version = \"$v\"" Cargo.toml || { echo "Cargo.toml is not at $v"; exit 1; }
[ -z "$(git status --porcelain --untracked-files=no)" ] || { echo "commit first"; exit 1; }
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && { echo "$tag exists"; exit 1; }

echo "== checks"
cargo fmt --all --check
cargo clippy --workspace --all-targets -q -- -D warnings
cargo test --workspace -q 2>&1 | grep -E "FAILED|panicked" && { echo "tests failed"; exit 1; }

echo "== build"
cargo build --release -p rugen --bin rugen
out=dist/$tag
rm -rf "$out"; mkdir -p "$out"
d=$out/rugen-$v-linux-x86_64
mkdir -p "$d/chars" "$d/stages"
cp README.md LICENSE-MIT LICENSE-APACHE packaging/rugen.desktop "$d/"
cp -r motif "$d/"
cp chars/README.md "$d/chars/"; cp stages/README.md "$d/stages/"
cp target/release/rugen "$d/"
tar -C "$out" -czf "$d.tar.gz" "rugen-$v-linux-x86_64"
rm -rf "$d"
cargo deb -p rugen --no-build -o "$out/" >/dev/null
cargo generate-rpm -p crates/app -o "$out/" >/dev/null
ls -1 "$out"

echo "== notes"
url=https://github.com/$repo/releases/download/$tag
link() { f=$(cd "$out" && ls $1 | head -1); echo "[\`$f\`]($url/$f)"; }
full=${RUGEN_FULL:-0}
{
  cat "$notes"
  echo
  echo "## Downloads"
  echo
  if [ "$full" = 1 ]; then
    echo "### Windows"
    echo
    echo "| Version | File |"
    echo "|---|---|"
    echo "| 64-bit (x64), most PCs | [\`rugen-$v-windows-x64.zip\`]($url/rugen-$v-windows-x64.zip) |"
    echo "| 32-bit (x86), older PCs | [\`rugen-$v-windows-x86.zip\`]($url/rugen-$v-windows-x86.zip) |"
    echo "| ARM64 (Snapdragon / Copilot+ PCs) | [\`rugen-$v-windows-arm64.zip\`]($url/rugen-$v-windows-arm64.zip) |"
    echo
    echo "Unzip anywhere and run \`rugen.exe\`. Fighters go in \`chars/\`, stages in \`stages/\`."
    echo
  fi
  echo "### Linux (x86_64)"
  echo
  echo "| Package | File |"
  echo "|---|---|"
  echo "| Fedora / RHEL / openSUSE (.rpm) | $(link "rugen-$v-*.rpm") |"
  echo "| Debian / Ubuntu / Mint (.deb) | $(link "rugen_$v-*.deb") |"
  echo "| Any distro, portable (.tar.gz) | $(link "rugen-$v-linux-x86_64.tar.gz") |"
  echo
  echo "Packages install the game for every user; fighters and stages then go in"
  echo "\`~/.local/share/rugen/chars\` and \`~/.local/share/rugen/stages\`."
  echo "The tarball is a self-contained folder."
  echo
  if [ "$full" = 1 ]; then
    echo "### macOS"
    echo
    echo "| Mac | File |"
    echo "|---|---|"
    echo "| Apple Silicon (M1 and later) | [\`rugen-$v-macos-arm64.tar.gz\`]($url/rugen-$v-macos-arm64.tar.gz) |"
    echo "| Intel | [\`rugen-$v-macos-x64.tar.gz\`]($url/rugen-$v-macos-x64.tar.gz) |"
    echo
    echo "Unsigned: the first time, right-click \`rugen\` → Open (or \`xattr -dr com.apple.quarantine <folder>\`)."
  else
    echo "Pre-release: Linux packages only. Windows and macOS players: use the"
    echo "[latest full release](https://github.com/$repo/releases/latest)."
  fi
  echo
  echo "Online hub: [rugen-server](https://github.com/rugen-dev/rugen-server) · Docs: https://rugen.dev"
} > "$out/notes.md"

echo "== publish $tag"
git tag -a "$tag" -m "rugen $v"
git push -q origin HEAD:main "$tag"
pre=--prerelease; [ "$full" = 1 ] && pre=--latest
gh release create "$tag" --repo "$repo" --title "rugen $v" --notes-file "$out/notes.md" $pre \
  "$out"/*.tar.gz "$out"/*.deb "$out"/*.rpm
echo "done: https://github.com/$repo/releases/tag/$tag"
