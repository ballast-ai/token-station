#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: scripts/prepare-formal-release.sh --version <x.y.z> --dir <empty-directory> [--repo <owner/repo>]" >&2
  exit 2
}

version=""
release_dir=""
repo="ballast-ai/token-station"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) version=${2:-}; shift 2 ;;
    --dir) release_dir=${2:-}; shift 2 ;;
    --repo) repo=${2:-}; shift 2 ;;
    *) usage ;;
  esac
done
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && -n "$release_dir" && -n "$repo" ]] || usage

if [[ -e "$release_dir" ]]; then
  [[ -d "$release_dir" ]] || { echo "release transfer path is not a directory: $release_dir" >&2; exit 1; }
  [[ -z "$(find "$release_dir" -mindepth 1 -maxdepth 1 -print -quit)" ]] || {
    echo "release transfer directory must be empty: $release_dir" >&2
    exit 1
  }
else
  mkdir -p "$release_dir"
fi

tag="v$version"
IFS=$'\t' read -r observed_tag is_draft is_prerelease < <(
  gh release view "$tag" --repo "$repo" --json tagName,isDraft,isPrerelease \
    --jq '[.tagName, .isDraft, .isPrerelease] | @tsv'
)
[[ "$observed_tag" == "$tag" ]] || { echo "release tag mismatch: $observed_tag" >&2; exit 1; }
[[ "$is_draft" == "true" ]] || { echo "formal release must still be a draft: $tag" >&2; exit 1; }
[[ "$is_prerelease" == "false" ]] || { echo "formal draft must not be a pre-release: $tag" >&2; exit 1; }

gh release download "$tag" --repo "$repo" --dir "$release_dir"
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
node "$root/scripts/check-release-inputs.mjs" --version "$version" --dir "$release_dir"
echo "formal release preparation: PASS ($tag)"
