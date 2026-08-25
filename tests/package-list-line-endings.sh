#!/usr/bin/env bash
# The official package list must survive a CRLF-emitting producer.
#
# `official-packages.py` used `print`, and on Windows Python's text mode turns
# "\n" into "\r\n". The shell readers use `IFS= read -r`, which strips the
# newline and keeps the carriage return, so a package name arrives as
# "agent-openai\r" and the path built from it breaks in the middle:
#     plugins/official/agent-openai\r/Cargo.toml
# cargo reports that as a manifest that does not exist, which reads as a
# missing file rather than a corrupted name — and it only happens on Windows,
# where nobody runs these scripts by hand.
#
# Two properties: the producer emits bare newlines, and the readers survive a
# carriage return anyway. Either alone would have prevented this; both is
# cheap, and the second is what protects the next producer.
set -euo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fail() { echo "FAIL: $*" >&2; exit 1; }

# The producer writes "\n", never "\r\n", whatever platform it runs on.
if python3 "$root/scripts/official-packages.py" --kind agent --field dir \
  | grep -q $'\r'; then
  fail "official-packages.py emits carriage returns"
fi

# Every reader strips one anyway. Check the source rather than simulating
# Windows: the guard has to be present in each loop, and there are six.
readers=0
for script in build-release.sh build-desktop.sh prepare-desktop-test-plugins.sh; do
  path="$root/scripts/$script"
  loops="$(grep -c 'IFS= read -r package' "$path")"
  guards="$(grep -c "package%\$'\\\\r'" "$path" || true)"
  [[ "$loops" -eq "$guards" ]] \
    || fail "$script has $loops package loops but $guards carriage-return guards"
  readers=$((readers + loops))
done
[[ "$readers" -gt 0 ]] || fail "no package readers found; this test is checking nothing"

# End to end: a producer that does emit CRLF must still yield a usable path.
workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT
printf 'agent-openai\r\nagent-gemini\r\n' >"$workdir/crlf.txt"
names=()
while IFS= read -r package; do
  package="${package%$'\r'}"
  names+=("$package")
done <"$workdir/crlf.txt"
[[ "${names[0]}" == "agent-openai" ]] \
  || fail "carriage return survived the read: ${names[0]@Q}"
[[ -f "$root/plugins/official/${names[0]}/Cargo.toml" ]] \
  || fail "the stripped name does not resolve to a real manifest"

echo "package list line endings: producer clean, $readers readers guarded"
