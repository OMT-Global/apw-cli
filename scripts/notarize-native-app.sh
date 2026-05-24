#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="${APW_APP_BUNDLE_PATH:-$ROOT_DIR/native-app/dist/APW.app}"
CLI_BIN="${APW_CLI_BINARY_PATH:-$ROOT_DIR/rust/target/release/apw}"
REQUIRED="${APW_NOTARIZE_REQUIRED:-0}"
DRY_RUN="${APW_NOTARIZE_DRY_RUN:-0}"

codesigning_env=(
  APPLE_DEVELOPER_CERT_P12
  APPLE_CERT_PASSWORD
)

notarytool_env=(
  APPLE_NOTARY_KEY_ID
  APPLE_NOTARY_KEY_ISSUER
  APPLE_NOTARY_PRIVATE_KEY
)

required_env=("${codesigning_env[@]}" "${notarytool_env[@]}")

missing=()
for name in "${required_env[@]}"; do
  if [[ -z "${!name:-}" ]]; then
    missing+=("$name")
  fi
done

if [[ "${#missing[@]}" -gt 0 ]]; then
  message="Apple notarization credentials are incomplete; missing: ${missing[*]}"
  if [[ "$REQUIRED" == "1" ]]; then
    echo "$message" >&2
    exit 1
  fi
  echo "::warning::$message. Skipping notarization."
  exit 0
fi

if [[ ! -d "$APP_DIR" ]]; then
  echo "APW app bundle not found: $APP_DIR" >&2
  exit 1
fi

if [[ "$DRY_RUN" == "1" ]]; then
  echo "Notarization inputs are present for $APP_DIR; dry run requested."
  exit 0
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Notarization requires macOS; current platform is $(uname -s)." >&2
  exit 1
fi

for tool in codesign ditto security spctl xcrun; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Required notarization tool not found on PATH: $tool" >&2
    exit 1
  fi
done

decode_base64() {
  local value="$1"
  local output="$2"
  if ! printf '%s' "$value" | base64 --decode >"$output" 2>/dev/null; then
    printf '%s' "$value" | base64 -D >"$output"
  fi
}

tmp_dir="$(mktemp -d)"
keychain="$tmp_dir/apw-notarization.keychain-db"
cert_path="$tmp_dir/developer-id.p12"
notary_key_path="$tmp_dir/notary-key.p8"
zip_path="$tmp_dir/APW.app.zip"
keychain_password="$(uuidgen | tr -d '-')"
signing_identity="${APPLE_DEVELOPER_IDENTITY:-Developer ID Application}"

cleanup() {
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

decode_base64 "$APPLE_DEVELOPER_CERT_P12" "$cert_path"
decode_base64 "$APPLE_NOTARY_PRIVATE_KEY" "$notary_key_path"
chmod 0600 "$cert_path" "$notary_key_path"

security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$cert_path" -k "$keychain" -P "$APPLE_CERT_PASSWORD" -T /usr/bin/codesign -T /usr/bin/security
security set-key-partition-list -S apple-tool:,apple: -s -k "$keychain_password" "$keychain"

existing_keychains=()
while IFS= read -r keychain_path; do
  existing_keychains+=("${keychain_path//\"/}")
done < <(security list-keychains -d user)
security list-keychains -d user -s "$keychain" "${existing_keychains[@]}"

if [[ -x "$CLI_BIN" ]]; then
  codesign --force --options runtime --timestamp --sign "$signing_identity" "$CLI_BIN"
fi

codesign --force --deep --options runtime --timestamp --sign "$signing_identity" "$APP_DIR"
codesign --verify --deep --strict --verbose=2 "$APP_DIR"

ditto -c -k --keepParent "$APP_DIR" "$zip_path"
xcrun notarytool submit "$zip_path" \
  --key "$notary_key_path" \
  --key-id "$APPLE_NOTARY_KEY_ID" \
  --issuer "$APPLE_NOTARY_KEY_ISSUER" \
  --wait
xcrun stapler staple "$APP_DIR"
spctl --assess --type execute --verbose=2 "$APP_DIR"

echo "APW.app signed, notarized, and stapled: $APP_DIR"
