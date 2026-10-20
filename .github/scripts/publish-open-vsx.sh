#!/usr/bin/env bash
# Publishes a release's VS Code packages (one .vsix per platform) to Open VSX, the registry VSCodium,
# Cursor, Windsurf and Gitpod install from.
#
#   OVSX_PAT=... GH_TOKEN=... bash .github/scripts/publish-open-vsx.sh v0.4.0
#
# OVSX_PAT is an Open VSX access token for the nguyentruong354 namespace; ovsx reads it from the
# environment, so it never shows on a command line. Without it the script says so and exits 0, so
# releases keep working before the secret is set; with REQUIRE_TOKEN=1 (a manual run) it fails instead.
# A version already on Open VSX is skipped, so the script can run again. DRY_RUN=1 downloads and
# checks the packages but publishes nothing.
set -euo pipefail

tag="${1:?usage: publish-open-vsx.sh <tag, e.g. v0.4.0>}"
version="${tag#v}"
if [[ "$tag" != v* || ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: release tags look like v0.4.0, not '$tag'" >&2
  exit 1
fi

if [[ -z "${OVSX_PAT:-}" ]]; then
  if [[ "${REQUIRE_TOKEN:-}" == 1 ]]; then
    echo "error: OVSX_PAT is not set; add it under Settings > Secrets and variables > Actions" >&2
    exit 1
  fi
  echo "::notice::OVSX_PAT is not set, so the extension is not published to Open VSX (add the secret to turn this on)"
  exit 0
fi

repo="${GITHUB_REPOSITORY:-NguyenTruong354/live-serve}"
dir=$(mktemp -d)
gh release download "$tag" --repo "$repo" --pattern '*.vsix' --dir "$dir"

shopt -s nullglob
packages=("$dir"/*.vsix)
if (( ${#packages[@]} == 0 )); then
  echo "error: release $tag has no .vsix files" >&2
  exit 1
fi
for package in "${packages[@]}"; do
  if [[ "$(basename "$package")" != live-serve-*-"$version".vsix ]]; then
    echo "error: $(basename "$package") is not a $version package" >&2
    exit 1
  fi
done

echo "Publishing ${#packages[@]} packages of $tag to Open VSX:"
printf '  %s\n' "${packages[@]##*/}"
if [[ "${DRY_RUN:-}" == 1 ]]; then
  echo "DRY_RUN=1: not publishing"
  exit 0
fi
npx --yes ovsx@1.2.0 publish --packagePath "${packages[@]}" --skip-duplicate
