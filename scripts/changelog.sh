#!/usr/bin/env bash
# Regenerate CHANGELOG.md from git-cliff against the given release tag.
# Invoked by cargo-release's `pre-release-hook` (see release.toml).
#
# Usage:
#   scripts/changelog.sh --tag vX.Y.Z

set -euo pipefail

TAG=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --tag)
      TAG="${2:-}"
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

if [[ -z "${TAG}" ]]; then
  echo "usage: $0 --tag <vX.Y.Z>" >&2
  exit 1
fi

if ! command -v git-cliff >/dev/null 2>&1; then
  echo "git-cliff not found; install with: cargo install git-cliff" >&2
  exit 1
fi

git-cliff --tag "${TAG}" --output CHANGELOG.md
git add CHANGELOG.md
