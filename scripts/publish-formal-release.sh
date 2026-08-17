#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: scripts/publish-formal-release.sh --version <x.y.z> --dir <release-directory> [--repo <owner/repo>]" >&2
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
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && -d "$release_dir" && -n "$repo" ]] || usage
: "${TOKEN_STATION_RELEASE_PUBKEY_HEX:?set the trusted CLI release public key}"
: "${TOKEN_STATION_UPDATER_PUBKEY:?set the trusted Tauri updater public key}"

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
node "$root/scripts/check-release-assets.mjs" --version "$version" --dir "$release_dir"
cargo run --locked --offline --manifest-path "$root/Cargo.toml" \
  -p token-station-release --bin ts-release -- verify \
  --pubkey "$TOKEN_STATION_RELEASE_PUBKEY_HEX" "$release_dir/manifest.json"
for artifact in \
  "$release_dir/token-station_${version}_aarch64.app.tar.gz" \
  "$release_dir/token-station_${version}_x86_64.app.tar.gz"; do
  cargo run --locked --offline --manifest-path "$root/Cargo.toml" \
    -p token-station-release --bin ts-release -- verify-updater \
    --pubkey "$TOKEN_STATION_UPDATER_PUBKEY" "$artifact"
done

tag="v$version"
IFS=$'\t' read -r observed_tag is_draft is_prerelease < <(
  gh release view "$tag" --repo "$repo" --json tagName,isDraft,isPrerelease \
    --jq '[.tagName, .isDraft, .isPrerelease] | @tsv'
)
[[ "$observed_tag" == "$tag" ]] || { echo "release tag mismatch: $observed_tag" >&2; exit 1; }
[[ "$is_draft" == "true" ]] || { echo "refusing to replace assets on a published release: $tag" >&2; exit 1; }
[[ "$is_prerelease" == "false" ]] || { echo "formal draft must not be a pre-release: $tag" >&2; exit 1; }

gh release upload "$tag" "$release_dir"/* --clobber --repo "$repo"
comparison_dir=$(mktemp -d "${TMPDIR:-/tmp}/token-station-release-publish.XXXXXX")
trap 'rm -rf "$comparison_dir"' EXIT
find "$release_dir" -mindepth 1 -maxdepth 1 -type f -exec basename {} \; | sort >"$comparison_dir/local"
gh release view "$tag" --repo "$repo" --json assets --jq '.assets[].name' | sort >"$comparison_dir/remote"
if ! diff -u "$comparison_dir/local" "$comparison_dir/remote"; then
  echo "draft asset names do not match the verified local release" >&2
  exit 1
fi

gh release edit "$tag" --repo "$repo" --draft=false --prerelease=false --title "Token Station $tag"
echo "formal release published: https://github.com/$repo/releases/tag/$tag"
