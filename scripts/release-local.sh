#!/usr/bin/env bash
# Check, tag and publish a release; GitHub builds every platform.
#
#   scripts/release-local.sh 0.6.2 notes.md
#
# Runs the checks locally, tags v<version>, pushes main and the tag, starts
# .github/workflows/release.yml on main for that tag (so it reuses the
# previous release's build cache), waits for it, then puts notes.md above the
# download table the workflow wrote.
#
# The version must already be in Cargo.toml and committed.
set -euo pipefail

v=${1:?usage: release-local.sh <version> <notes.md>}
notes=${2:?usage: release-local.sh <version> <notes.md>}
tag=v$v
repo=rugen-dev/rugen-engine
cd "$(git rev-parse --show-toplevel)"

grep -q "^version = \"$v\"" Cargo.toml || { echo "Cargo.toml is not at $v"; exit 1; }
[ -z "$(git status --porcelain --untracked-files=no)" ] || { echo "commit first"; exit 1; }
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && { echo "$tag exists"; exit 1; }
[ -s "$notes" ] || { echo "empty notes"; exit 1; }

echo "== checks"
cargo fmt --all --check
cargo clippy --workspace --all-targets -q -- -D warnings
if cargo test --workspace -q 2>&1 | grep -E "FAILED|panicked"; then
  echo "tests failed"; exit 1
fi

echo "== tag and push $tag"
git tag -a "$tag" -m "rugen $v"
git push -q origin HEAD:main "$tag"

echo "== build on GitHub"
gh workflow run release.yml --repo "$repo" --ref main -f tag="$tag"
sleep 10
run=$(gh run list --repo "$repo" --workflow release.yml --limit 1 --json databaseId -q '.[0].databaseId')
gh run watch "$run" --repo "$repo" --exit-status >/dev/null || {
  echo "release build failed: https://github.com/$repo/actions/runs/$run"; exit 1; }

echo "== notes"
body=$(mktemp)
{ cat "$notes"; echo; gh release view "$tag" --repo "$repo" --json body -q .body; } > "$body"
gh release edit "$tag" --repo "$repo" --notes-file "$body" --latest
rm -f "$body"
gh release view "$tag" --repo "$repo" --json assets -q '.assets[].name'
echo "done: https://github.com/$repo/releases/tag/$tag"
