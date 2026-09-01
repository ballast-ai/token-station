#!/usr/bin/env bash
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat >&2 <<'EOF'
usage:
  scripts/release-latest-formal.sh start --confirm <vX.Y.Z> [--repo <owner/repo>] [--remote <name>]
  scripts/release-latest-formal.sh recover-draft --run <id> --confirm <vX.Y.Z> [--repo <owner/repo>] [--remote <name>]
  scripts/release-latest-formal.sh prepare --dir <empty-directory> [--repo <owner/repo>]
  scripts/release-latest-formal.sh sign --dir <release-directory> --release-key <file> --updater-key <file> [--pub-date <RFC3339>] [--notes-file <file>]
  scripts/release-latest-formal.sh publish --dir <release-directory> --confirm <vX.Y.Z> [--repo <owner/repo>] [--notes-file <file>]

stages:
  start    Verify the latest remote main commit, create its formal tag, and wait for the draft.
  recover-draft  Recover one draft from the allowlisted artifacts of a completed Release run.
  prepare  Download the exact draft assets into an empty transfer directory.
  sign     Sign the downloaded assets on the offline trusted host.
  publish  Verify the signed assets, apply the release notes, and publish the stable Release.
EOF
  exit 2
}

fail() {
  echo "formal release stopped: $*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command is not available: $1"
}

read_version() {
  node -p "require('$root/apps/desktop/package.json').version"
}

require_clean_checkout() {
  [[ -z "$(git -C "$root" status --porcelain --untracked-files=normal)" ]] ||
    fail "the working tree is not clean"
}

require_tagged_checkout() {
  local tag=$1
  local head
  local tag_commit
  head=$(git -C "$root" rev-parse HEAD)
  tag_commit=$(git -C "$root" rev-parse "${tag}^{}" 2>/dev/null) ||
    fail "the local tag does not exist: $tag"
  [[ "$head" == "$tag_commit" ]] || fail "HEAD is not the commit for $tag"
}

require_formal_notes() {
  local version=$1
  local notes_file=$2
  node "$root/scripts/check-formal-release-notes.mjs" --version "$version" --file "$notes_file"
}

resolve_release_dir() {
  local release_dir=$1
  local absolute
  absolute=$(node -e 'process.stdout.write(require("node:path").resolve(process.argv[1]))' "$release_dir")
  case "$absolute" in
    "$root"|"$root/"*) fail "the release transfer directory must be outside the source checkout" ;;
  esac
  printf '%s\n' "$absolute"
}

load_github_variable() {
  local name=$1
  local repo=$2
  local value=${!name:-}
  if [[ -z "$value" ]]; then
    value=$(gh variable get "$name" --repo "$repo" 2>/dev/null) ||
      fail "set $name or configure the matching GitHub Actions variable"
    [[ -n "$value" ]] || fail "$name is empty"
    printf -v "$name" '%s' "$value"
    export "$name"
  fi
}

require_formal_github_config() {
  local repo=$1
  local version=$2
  local enabled
  enabled=$(gh variable get TOKEN_STATION_FORMAL_ARTIFACTS_ENABLED --repo "$repo" 2>/dev/null) ||
    fail "configure the TOKEN_STATION_FORMAL_ARTIFACTS_ENABLED GitHub Actions variable"
  [[ "$enabled" == "true" ]] ||
    fail "set TOKEN_STATION_FORMAL_ARTIFACTS_ENABLED to true"

  load_github_variable TOKEN_STATION_RELEASE_PUBKEY_HEX "$repo"
  load_github_variable TOKEN_STATION_UPDATER_PUBKEY "$repo"

  local secret_names
  secret_names=$(gh secret list --repo "$repo" --app actions --json name --jq '.[].name') ||
    fail "could not inspect GitHub Actions secret names"
  local -a required_secrets=(
    APPLE_CERTIFICATE \
    APPLE_CERTIFICATE_PASSWORD \
    APPLE_SIGNING_IDENTITY \
    APPLE_KEYCHAIN_PASSWORD \
    APPLE_API_ISSUER \
    APPLE_API_KEY \
    APPLE_API_KEY_CONTENT
  )
  if [[ "$version" != "2.0.0" ]]; then
    required_secrets+=(
      WINDOWS_CERTIFICATE
      WINDOWS_CERTIFICATE_PASSWORD
      WINDOWS_TIMESTAMP_URL
    )
  fi
  local secret
  for secret in "${required_secrets[@]}"; do
    grep -Fqx "$secret" <<<"$secret_names" ||
      fail "configure the $secret GitHub Actions secret before creating a formal tag"
  done
}

require_repo_remote() {
  local remote=$1
  local repo=$2
  local remote_url
  remote_url=$(git -C "$root" remote get-url "$remote" 2>/dev/null) ||
    fail "Git remote does not exist: $remote"
  case "$remote_url" in
    "https://github.com/$repo"|"https://github.com/$repo.git"|"git@github.com:$repo"|"git@github.com:$repo.git") ;;
    *) fail "remote $remote points to $remote_url, not github.com/$repo" ;;
  esac
}

start_release() {
  local repo="ballast-ai/token-station"
  local remote="origin"
  local confirm=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --confirm) [[ $# -ge 2 ]] || usage; confirm=$2; shift 2 ;;
      --repo) [[ $# -ge 2 ]] || usage; repo=$2; shift 2 ;;
      --remote) [[ $# -ge 2 ]] || usage; remote=$2; shift 2 ;;
      *) usage ;;
    esac
  done

  require_command git
  require_command gh
  require_command node
  require_clean_checkout
  [[ "$(git -C "$root" branch --show-current)" == "main" ]] ||
    fail "start must run on the main branch"
  require_repo_remote "$remote" "$repo"
  gh auth status --hostname github.com >/dev/null 2>&1 || fail "GitHub CLI authentication failed"

  git -C "$root" fetch --prune "$remote" main --tags
  local head
  local remote_head
  head=$(git -C "$root" rev-parse HEAD)
  remote_head=$(git -C "$root" rev-parse "$remote/main")
  [[ "$head" == "$remote_head" ]] ||
    fail "local main is not the latest $remote/main commit ($remote_head)"

  local version
  local tag
  local notes_file
  version=$(read_version)
  tag="v$version"
  notes_file="$root/docs/release/$tag.md"
  [[ "$confirm" == "$tag" ]] || fail "rerun with --confirm $tag"
  require_formal_notes "$version" "$notes_file"
  require_formal_github_config "$repo" "$version"
  node "$root/scripts/check-release-readiness.mjs" --version "$version" --formal

  if git -C "$root" show-ref --verify --quiet "refs/tags/$tag"; then
    fail "the local formal tag already exists: $tag"
  fi
  local remote_tag_status
  set +e
  git -C "$root" ls-remote --exit-code --tags "$remote" "refs/tags/$tag" >/dev/null 2>&1
  remote_tag_status=$?
  set -e
  case "$remote_tag_status" in
    0) fail "the remote formal tag already exists: $tag" ;;
    2) ;;
    *) fail "could not verify whether the remote tag exists: $tag" ;;
  esac

  local successful_full_ci
  successful_full_ci=$(gh api --method GET \
    "repos/$repo/actions/workflows/full-ci.yml/runs" \
    -f "head_sha=$head" -f branch=main -f event=push -f per_page=100 \
    --jq '[.workflow_runs[] | select(.conclusion == "success")] | length')
  [[ "$successful_full_ci" =~ ^[1-9][0-9]*$ ]] ||
    fail "commit $head has no successful push-event Full CI run on main"

  git -C "$root" tag -a "$tag" -m "Token Station $tag"
  if ! git -C "$root" push "$remote" "refs/tags/$tag"; then
    git -C "$root" tag -d "$tag" >/dev/null 2>&1 || true
    fail "the tag push failed; inspect the remote before retrying"
  fi

  echo "Pushed $tag at $head. Waiting for the Release workflow."
  local run_id=""
  local run_url=""
  local run_row=""
  local deadline=$((SECONDS + 180))
  while [[ $SECONDS -lt $deadline ]]; do
    run_row=$(gh run list --repo "$repo" --workflow release.yml --event push --commit "$head" \
      --limit 20 --json databaseId,headBranch,url \
      --jq ".[] | select(.headBranch == \"$tag\") | [.databaseId, .url] | @tsv" | head -n 1)
    if [[ -n "$run_row" ]]; then
      IFS=$'\t' read -r run_id run_url <<<"$run_row"
      break
    fi
    sleep 3
  done
  [[ -n "$run_id" ]] ||
    fail "the Release workflow did not appear within 180 seconds; inspect GitHub Actions"

  echo "Release workflow: $run_url"
  gh run watch "$run_id" --repo "$repo" --exit-status

  local observed_tag
  local is_draft
  local is_prerelease
  IFS=$'\t' read -r observed_tag is_draft is_prerelease < <(
    gh release view "$tag" --repo "$repo" --json tagName,isDraft,isPrerelease \
      --jq '[.tagName, .isDraft, .isPrerelease] | @tsv'
  )
  [[ "$observed_tag" == "$tag" && "$is_draft" == "true" && "$is_prerelease" == "false" ]] ||
    fail "the workflow did not create the expected formal draft: $tag"

  echo "Formal draft is ready: $tag"
  echo "Next: scripts/release-latest-formal.sh prepare --dir <empty-transfer-directory>"
}

recover_draft() {
  local repo="ballast-ai/token-station"
  local remote="origin"
  local run_id=""
  local confirm=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --run) [[ $# -ge 2 ]] || usage; run_id=$2; shift 2 ;;
      --confirm) [[ $# -ge 2 ]] || usage; confirm=$2; shift 2 ;;
      --repo) [[ $# -ge 2 ]] || usage; repo=$2; shift 2 ;;
      --remote) [[ $# -ge 2 ]] || usage; remote=$2; shift 2 ;;
      *) usage ;;
    esac
  done
  [[ "$run_id" =~ ^[1-9][0-9]*$ && "$confirm" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || usage

  require_command git
  require_command gh
  require_command node
  require_command cargo
  require_clean_checkout
  [[ "$(git -C "$root" branch --show-current)" == "main" ]] ||
    fail "recover-draft must run on the main branch"
  require_repo_remote "$remote" "$repo"
  gh auth status --hostname github.com >/dev/null 2>&1 || fail "GitHub CLI authentication failed"

  git -C "$root" fetch --prune "$remote" main --tags
  local head
  local remote_head
  head=$(git -C "$root" rev-parse HEAD)
  remote_head=$(git -C "$root" rev-parse "$remote/main")
  [[ "$head" == "$remote_head" ]] ||
    fail "local main is not the latest $remote/main commit ($remote_head)"
  [[ "$(git -C "$root" cat-file -t "$confirm" 2>/dev/null)" == "tag" ]] ||
    fail "the formal release tag must exist and be annotated: $confirm"
  local tag_commit
  tag_commit=$(git -C "$root" rev-parse "$confirm^{}")
  git -C "$root" merge-base --is-ancestor "$tag_commit" "$remote/main" ||
    fail "the release tag is not on $remote/main: $confirm"

  local version
  version=$(git -C "$root" show "$confirm:apps/desktop/package.json" | node -e \
    'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>process.stdout.write(JSON.parse(s).version))')
  [[ "$confirm" == "v$version" ]] || fail "the release tag does not match its desktop version: $confirm"
  git -C "$root" cat-file -e "$confirm:docs/release/$confirm.md" 2>/dev/null ||
    fail "the release tag has no formal release notes: docs/release/$confirm.md"

  if gh release view "$confirm" --repo "$repo" >/dev/null 2>&1; then
    fail "a GitHub Release already exists for $confirm"
  fi

  local workflow_name
  local event
  local status
  local conclusion
  local run_url
  IFS=$'\t' read -r workflow_name event status conclusion run_url < <(
    gh run view "$run_id" --repo "$repo" --json workflowName,event,status,conclusion,url \
      --jq '[.workflowName, .event, .status, .conclusion, .url] | @tsv'
  )
  [[ "$workflow_name" == "Release" ]] || fail "run $run_id is not a Release workflow run"
  [[ "$event" == "push" || "$event" == "workflow_dispatch" ]] ||
    fail "run $run_id has an unsupported event: $event"
  [[ "$status" == "completed" ]] || fail "run $run_id is not completed"
  case "$conclusion" in
    success|failure|cancelled) ;;
    *) fail "run $run_id has an unsupported conclusion: $conclusion" ;;
  esac

  local artifact_root
  local release_dir
  artifact_root=$(mktemp -d "${TMPDIR:-/tmp}/token-station-formal-run.XXXXXX")
  release_dir=$(mktemp -d "${TMPDIR:-/tmp}/token-station-formal-draft.XXXXXX")
  trap "rm -rf '$artifact_root' '$release_dir'" EXIT
  local -a artifacts=(
    dist-aarch64-apple-darwin
    dist-aarch64-unknown-linux-gnu
    dist-x86_64-apple-darwin
    dist-x86_64-unknown-linux-gnu
    token-station-desktop-aarch64-apple-darwin
    token-station-desktop-x86_64-apple-darwin
    token-station-desktop-x86_64-pc-windows-msvc
    token-station-desktop-linux-x86_64
  )
  local artifact
  for artifact in "${artifacts[@]}"; do
    mkdir "$artifact_root/$artifact"
    gh run download "$run_id" --repo "$repo" --name "$artifact" --dir "$artifact_root/$artifact"
  done

  "$root/scripts/collect-formal-release-artifacts.sh" \
    --version "$version" --artifacts-dir "$artifact_root" --out-dir "$release_dir"
  cargo run --locked --manifest-path "$root/Cargo.toml" \
    -p token-station-release --bin ts-release -- manifest \
    --version "$version" --out "$release_dir/manifest.json" \
    "$release_dir"/token-station-cli-*.tar.gz
  node "$root/scripts/check-release-inputs.mjs" --version "$version" --dir "$release_dir"

  gh release create "$confirm" --repo "$repo" --draft --verify-tag \
    --title "Token Station $confirm" \
    --notes "Draft. Awaiting offline CLI and updater signatures. Run the prepare, sign, and publish formal-release scripts." \
    "$release_dir"/*.tar.gz "$release_dir"/*.dmg "$release_dir"/*.msi \
    "$release_dir"/*.deb "$release_dir"/*.AppImage "$release_dir"/*.rpm \
    "$release_dir/manifest.json"
  local observed_tag
  local is_draft
  local is_prerelease
  IFS=$'\t' read -r observed_tag is_draft is_prerelease < <(
    gh release view "$confirm" --repo "$repo" --json tagName,isDraft,isPrerelease \
      --jq '[.tagName, .isDraft, .isPrerelease] | @tsv'
  )
  [[ "$observed_tag" == "$confirm" && "$is_draft" == "true" && "$is_prerelease" == "false" ]] ||
    fail "the recovered draft does not match $confirm"

  rm -rf "$artifact_root" "$release_dir"
  trap - EXIT
  echo "Recovered formal draft from $run_url: $confirm"
  echo "Next on the tagged checkout: scripts/release-latest-formal.sh prepare --dir <empty-transfer-directory>"
}

prepare_release() {
  local repo="ballast-ai/token-station"
  local release_dir=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --dir) [[ $# -ge 2 ]] || usage; release_dir=$2; shift 2 ;;
      --repo) [[ $# -ge 2 ]] || usage; repo=$2; shift 2 ;;
      *) usage ;;
    esac
  done
  [[ -n "$release_dir" ]] || usage

  require_command git
  require_command gh
  require_command node
  release_dir=$(resolve_release_dir "$release_dir")
  local version
  local tag
  version=$(read_version)
  tag="v$version"
  require_clean_checkout
  require_tagged_checkout "$tag"
  require_formal_notes "$version" "$root/docs/release/$tag.md"
  "$root/scripts/prepare-formal-release.sh" --version "$version" --dir "$release_dir" --repo "$repo"
  echo "Next on the offline trusted host: scripts/release-latest-formal.sh sign --dir '$release_dir' --release-key <file> --updater-key <file>"
}

sign_release() {
  local release_dir=""
  local release_key=""
  local updater_key=""
  local pub_date=""
  local notes_file=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --dir) [[ $# -ge 2 ]] || usage; release_dir=$2; shift 2 ;;
      --release-key) [[ $# -ge 2 ]] || usage; release_key=$2; shift 2 ;;
      --updater-key) [[ $# -ge 2 ]] || usage; updater_key=$2; shift 2 ;;
      --pub-date) [[ $# -ge 2 ]] || usage; pub_date=$2; shift 2 ;;
      --notes-file) [[ $# -ge 2 ]] || usage; notes_file=$2; shift 2 ;;
      *) usage ;;
    esac
  done
  [[ -n "$release_dir" && -n "$release_key" && -n "$updater_key" ]] || usage

  require_command git
  require_command node
  require_command cargo
  release_dir=$(resolve_release_dir "$release_dir")
  local version
  local tag
  version=$(read_version)
  tag="v$version"
  [[ -n "$notes_file" ]] || notes_file="$root/docs/release/$tag.md"
  [[ -n "$pub_date" ]] || pub_date=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
  require_clean_checkout
  require_tagged_checkout "$tag"
  require_formal_notes "$version" "$notes_file"
  : "${TOKEN_STATION_RELEASE_PUBKEY_HEX:?set the trusted CLI release public key}"
  : "${TOKEN_STATION_UPDATER_PUBKEY:?set the trusted Tauri updater public key}"
  : "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:?set the updater private-key password}"

  "$root/scripts/sign-formal-release.sh" \
    --version "$version" \
    --dir "$release_dir" \
    --release-key "$release_key" \
    --updater-key "$updater_key" \
    --pub-date "$pub_date" \
    --notes-file "$notes_file"
  echo "Next on the online release host: scripts/release-latest-formal.sh publish --dir '$release_dir' --confirm '$tag'"
}

publish_release() {
  local repo="ballast-ai/token-station"
  local release_dir=""
  local confirm=""
  local notes_file=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --dir) [[ $# -ge 2 ]] || usage; release_dir=$2; shift 2 ;;
      --confirm) [[ $# -ge 2 ]] || usage; confirm=$2; shift 2 ;;
      --repo) [[ $# -ge 2 ]] || usage; repo=$2; shift 2 ;;
      --notes-file) [[ $# -ge 2 ]] || usage; notes_file=$2; shift 2 ;;
      *) usage ;;
    esac
  done
  [[ -n "$release_dir" ]] || usage

  require_command git
  require_command gh
  require_command node
  require_command cargo
  release_dir=$(resolve_release_dir "$release_dir")
  local version
  local tag
  version=$(read_version)
  tag="v$version"
  [[ -n "$notes_file" ]] || notes_file="$root/docs/release/$tag.md"
  [[ "$confirm" == "$tag" ]] || fail "rerun with --confirm $tag"
  require_clean_checkout
  require_tagged_checkout "$tag"
  require_formal_notes "$version" "$notes_file"
  gh auth status --hostname github.com >/dev/null 2>&1 || fail "GitHub CLI authentication failed"
  load_github_variable TOKEN_STATION_RELEASE_PUBKEY_HEX "$repo"
  load_github_variable TOKEN_STATION_UPDATER_PUBKEY "$repo"

  "$root/scripts/publish-formal-release.sh" \
    --version "$version" \
    --dir "$release_dir" \
    --repo "$repo" \
    --notes-file "$notes_file"
}

command_name=${1:-}
[[ -n "$command_name" ]] || usage
shift
case "$command_name" in
  start) start_release "$@" ;;
  recover-draft) recover_draft "$@" ;;
  prepare) prepare_release "$@" ;;
  sign) sign_release "$@" ;;
  publish) publish_release "$@" ;;
  *) usage ;;
esac
