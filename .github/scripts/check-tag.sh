#!/usr/bin/env bash
# Checks that a release tag (v0.4.0) matches the versions in the repository, so a release never ships
# archives named after one version holding a binary and an extension that say another.
#
#   bash .github/scripts/check-tag.sh v0.4.0
set -euo pipefail

tag="${1:?usage: check-tag.sh <tag, e.g. v0.4.0>}"
tag="${tag#refs/tags/}"
want="${tag#v}"
if [[ "$tag" != v* || -z "$want" ]]; then
  echo "error: release tags look like v0.4.0, not '$tag'" >&2
  exit 1
fi

crate=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -n 1)
extension=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' editors/vscode/package.json | head -n 1)

problems=0
check() {
  if [[ "$2" != "$want" ]]; then
    echo "error: tag $tag, but $1 says ${2:-nothing}" >&2
    problems=1
  fi
}
check "Cargo.toml" "$crate"
check "editors/vscode/package.json" "$extension"
if ! grep -q "^## $want\b" CHANGELOG.md; then
  echo "error: CHANGELOG.md has no '## $want' section (rename '## Unreleased' and add the date)" >&2
  problems=1
fi

if [[ $problems -ne 0 ]]; then
  echo "Bump the versions together, commit, then tag that commit." >&2
  exit 1
fi
echo "tag $tag matches Cargo.toml, the extension and CHANGELOG.md"
