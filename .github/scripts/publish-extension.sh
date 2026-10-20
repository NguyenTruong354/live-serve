#!/usr/bin/env bash
# Publishes a release's VS Code packages (one .vsix per platform) to a registry:
#   open-vsx     Open VSX, which VSCodium, Cursor, Windsurf and Gitpod install from (token: OVSX_PAT)
#   marketplace  the VS Code Marketplace (token: VSCE_PAT, an Azure DevOps token with Marketplace: Manage)
#
#   OVSX_PAT=... GH_TOKEN=... bash .github/scripts/publish-extension.sh open-vsx v0.4.0
#
# The publishing tools read the token from the environment, so it never shows on a command line.
# Without it the script says so and exits 0, so releases keep working before the secret is set; with
# REQUIRE_TOKEN=1 (a manual run) it fails instead. A version already in the registry is skipped, so
# the script can run again. DRY_RUN=1 downloads and checks the packages but publishes nothing.
set -euo pipefail

registry="${1:?usage: publish-extension.sh <open-vsx|marketplace> <tag, e.g. v0.4.0>}"
tag="${2:?usage: publish-extension.sh <open-vsx|marketplace> <tag, e.g. v0.4.0>}"
case "$registry" in
  open-vsx)
    name="Open VSX"
    token_var=OVSX_PAT
    publish=(npx --yes ovsx@1.2.0 publish)
    ;;
  marketplace)
    name="the VS Code Marketplace"
    token_var=VSCE_PAT
    publish=(npx --yes @vscode/vsce@4.0.0 publish)
    ;;
  *)
    echo "error: the registry is open-vsx or marketplace, not '$registry'" >&2
    exit 1
    ;;
esac

version="${tag#v}"
if [[ "$tag" != v* || ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: release tags look like v0.4.0, not '$tag'" >&2
  exit 1
fi

if [[ -z "${!token_var:-}" ]]; then
  if [[ "${REQUIRE_TOKEN:-}" == 1 ]]; then
    echo "error: $token_var is not set; add it under Settings > Secrets and variables > Actions" >&2
    exit 1
  fi
  echo "::notice::$token_var is not set, so the extension is not published to $name (add the secret to turn this on)"
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

echo "Publishing ${#packages[@]} packages of $tag to $name:"
printf '  %s\n' "${packages[@]##*/}"
if [[ "${DRY_RUN:-}" == 1 ]]; then
  echo "DRY_RUN=1: not publishing"
  exit 0
fi
"${publish[@]}" --packagePath "${packages[@]}" --skip-duplicate
