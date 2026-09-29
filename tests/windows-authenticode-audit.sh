#!/usr/bin/env bash
set -euo pipefail

readonly project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly test_root="$(mktemp -d "${TMPDIR:-/tmp}/token-station-windows-audit.XXXXXX")"
readonly fake_bin="$test_root/bin"
readonly source_root="$test_root/source"
readonly bundle_root="$test_root/bundle"
readonly binary="$test_root/token-station-desktop.exe"
readonly rust_sysroot="$test_root/rust"
readonly private_cargo_home="$test_root/cargo"
readonly node_bin_dir="$(dirname "$(command -v node)")"
trap 'rm -rf -- "$test_root"' EXIT

mkdir -p "$fake_bin" "$source_root/scripts" "$bundle_root/msi" "$rust_sysroot" "$private_cargo_home"
touch "$bundle_root/msi/token-station.msi"

cat >"$source_root/scripts/official-packages.py" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
echo plugin-test
SCRIPT

cat >"$binary" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
[[ "${1:-}" == "--self-test-bundled-plugins" && -n "${2:-}" ]]
cat >"$2" <<'JSON'
{"passed":true,"bundle":{"id":"com.tokenstation.desktop"},"storage":{"data_directory_private":true,"private_file_verified":true,"credential_read":false},"gateway":{"loadable":true},"plugins":[{"id":"plugin-test","source":"builtin","loadable":true}]}
JSON
SCRIPT

cat >"$fake_bin/uname" <<'SCRIPT'
#!/usr/bin/env bash
echo MINGW64_NT
SCRIPT

cat >"$fake_bin/cygpath" <<'SCRIPT'
#!/usr/bin/env bash
[[ "${1:-}" == "-w" && $# -eq 2 ]]
printf '%s\n' "$2"
SCRIPT

cat >"$fake_bin/strings" <<'SCRIPT'
#!/usr/bin/env bash
echo plugin-test
SCRIPT

cat >"$fake_bin/powershell.exe" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
command_text="${*: -1}"
if [[ "$command_text" == *".msi"* ]]; then
  echo "${TEST_INSTALLER_STATUS:?}"
else
  echo "${TEST_BINARY_STATUS:?}"
fi
SCRIPT

chmod +x "$source_root/scripts/official-packages.py" "$binary" "$fake_bin"/*

run_audit() {
  env \
    PATH="$fake_bin:$node_bin_dir:/usr/bin:/bin" \
    TEST_BINARY_STATUS="$1" \
    TEST_INSTALLER_STATUS="$2" \
    "$project_root/scripts/audit-desktop-artifact.sh" \
      --mode production \
      --binary "$binary" \
      --bundle-root "$bundle_root" \
      --source-root "$source_root" \
      --rust-sysroot "$rust_sysroot" \
      --private-cargo-home "$private_cargo_home" \
      "${@:3}"
}

run_audit NotSigned NotSigned --unsigned-windows >/dev/null
run_audit Valid Valid >/dev/null

if run_audit Valid NotSigned --unsigned-windows >"$test_root/unexpected-signed" 2>&1; then
  echo "unsigned Windows audit accepted a signed executable" >&2
  exit 1
fi
grep -Fq "unexpected signature status" "$test_root/unexpected-signed"

if run_audit NotSignedButUnexpected NotSigned --unsigned-windows >"$test_root/fuzzy-status" 2>&1; then
  echo "unsigned Windows audit accepted a non-exact status" >&2
  exit 1
fi
grep -Fq "unexpected signature status" "$test_root/fuzzy-status"

if run_audit NotSigned NotSigned >"$test_root/missing-signature" 2>&1; then
  echo "signed Windows audit accepted unsigned artifacts" >&2
  exit 1
fi
grep -Fq "signature is not valid" "$test_root/missing-signature"

echo "Windows Authenticode audit policy: PASS"
