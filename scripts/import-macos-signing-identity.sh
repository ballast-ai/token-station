#!/usr/bin/env bash
set -euo pipefail

fail() {
  echo "macOS signing identity import failed: $*" >&2
  exit 1
}

require_env() {
  local name=$1
  [[ -n "${!name:-}" ]] || fail "$name is not set"
}

for required in \
  RUNNER_TEMP \
  GITHUB_ENV \
  APPLE_CERTIFICATE \
  APPLE_CERTIFICATE_PASSWORD \
  APPLE_SIGNING_IDENTITY \
  APPLE_KEYCHAIN_PASSWORD
do
  require_env "$required"
done

[[ "$RUNNER_TEMP" == /* && -d "$RUNNER_TEMP" ]] ||
  fail "RUNNER_TEMP must be an existing absolute directory"

case "$GITHUB_ENV" in
  "$RUNNER_TEMP"/*) ;;
  *) fail "GITHUB_ENV must be inside RUNNER_TEMP" ;;
esac

resolve_openssl() {
  local candidate=""
  local brew_prefix=""

  if command -v brew >/dev/null 2>&1; then
    brew_prefix=$(brew --prefix openssl@3 2>/dev/null || true)
    if [[ -n "$brew_prefix" ]]; then
      candidate="$brew_prefix/bin/openssl"
      if [[ -x "$candidate" ]]; then
        printf '%s\n' "$candidate"
        return
      fi
    fi
  fi

  candidate=$(command -v openssl || true)
  [[ -n "$candidate" && -x "$candidate" ]] ||
    fail "OpenSSL is not available"
  "$candidate" version | grep -Fq "OpenSSL 3." ||
    fail "OpenSSL 3 is required to normalize the PKCS#12 bundle"
  printf '%s\n' "$candidate"
}

umask 077

readonly openssl_bin=$(resolve_openssl)
readonly source_bundle="$RUNNER_TEMP/token-station-certificate.p12"
readonly certificate_pem="$RUNNER_TEMP/token-station-certificate.pem"
readonly private_key_pem="$RUNNER_TEMP/token-station-private-key.pem"
readonly compatible_bundle="$RUNNER_TEMP/token-station-certificate-compatible.p12"
readonly keychain="$RUNNER_TEMP/token-station-signing.keychain-db"

cleanup_plaintext_material() {
  /bin/rm -f -- "$certificate_pem" "$private_key_pem" "$compatible_bundle"
}
trap cleanup_plaintext_material EXIT

printf 'APPLE_TEMP_CERTIFICATE_PATH=%s\n' "$source_bundle" >> "$GITHUB_ENV"
printf 'APPLE_TEMP_KEYCHAIN_PATH=%s\n' "$keychain" >> "$GITHUB_ENV"

printf '%s' "$APPLE_CERTIFICATE" | base64 --decode > "$source_bundle"

"$openssl_bin" pkcs12 \
  -in "$source_bundle" \
  -passin env:APPLE_CERTIFICATE_PASSWORD \
  -clcerts -nokeys \
  -out "$certificate_pem"
"$openssl_bin" pkcs12 \
  -in "$source_bundle" \
  -passin env:APPLE_CERTIFICATE_PASSWORD \
  -nocerts -nodes \
  -out "$private_key_pem"

certificate_count=$(grep -Ec -- '-----BEGIN CERTIFICATE-----' "$certificate_pem" || true)
private_key_count=$(grep -Ec -- '-----BEGIN ([A-Z0-9]+ )?PRIVATE KEY-----' "$private_key_pem" || true)
[[ "$certificate_count" == 1 ]] || fail "the bundle must contain exactly one leaf certificate"
[[ "$private_key_count" == 1 ]] || fail "the bundle must contain exactly one private key"

"$openssl_bin" pkey -in "$private_key_pem" -check -noout >/dev/null
subject=$("$openssl_bin" x509 -in "$certificate_pem" -noout -subject -nameopt RFC2253)
grep -Fq "CN=$APPLE_SIGNING_IDENTITY" <<<"$subject" ||
  fail "the certificate subject does not match APPLE_SIGNING_IDENTITY"

certificate_public_key_hash=$(
  "$openssl_bin" x509 -in "$certificate_pem" -pubkey -noout |
    "$openssl_bin" pkey -pubin -outform DER 2>/dev/null |
    "$openssl_bin" dgst -sha256
)
private_public_key_hash=$(
  "$openssl_bin" pkey -in "$private_key_pem" -pubout -outform DER 2>/dev/null |
    "$openssl_bin" dgst -sha256
)
[[ "$certificate_public_key_hash" == "$private_public_key_hash" ]] ||
  fail "the certificate and private key do not match"

# macOS security(1) cannot reliably import OpenSSL 3's default PBES2/AES
# PKCS#12 output. Repackage only inside this ephemeral runner with the
# long-supported PKCS#12 SHA-1/3DES profile before importing it.
"$openssl_bin" pkcs12 -export \
  -inkey "$private_key_pem" \
  -in "$certificate_pem" \
  -name "$APPLE_SIGNING_IDENTITY" \
  -keypbe PBE-SHA1-3DES \
  -certpbe PBE-SHA1-3DES \
  -macalg sha1 \
  -passout env:APPLE_CERTIFICATE_PASSWORD \
  -out "$compatible_bundle"
"$openssl_bin" pkcs12 \
  -in "$compatible_bundle" \
  -passin env:APPLE_CERTIFICATE_PASSWORD \
  -noout

security create-keychain -p "$APPLE_KEYCHAIN_PASSWORD" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$APPLE_KEYCHAIN_PASSWORD" "$keychain"
security import "$compatible_bundle" \
  -k "$keychain" \
  -P "$APPLE_CERTIFICATE_PASSWORD" \
  -T /usr/bin/codesign >/dev/null
security list-keychains -d user -s "$keychain" login.keychain-db
security default-keychain -s "$keychain"
security set-key-partition-list \
  -S apple-tool:,apple:,codesign: \
  -s \
  -k "$APPLE_KEYCHAIN_PASSWORD" \
  "$keychain" >/dev/null
security find-certificate -c "$APPLE_SIGNING_IDENTITY" "$keychain" >/dev/null ||
  fail "the expected signing certificate was not imported"
security find-identity -v -p codesigning "$keychain" |
  grep -Fq "\"$APPLE_SIGNING_IDENTITY\"" ||
  fail "the expected signing identity was not imported"

echo "macOS signing identity import: PASS"
