#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXTENSION_DIR="$ROOT_DIR/browser-bridge"
HOST_NAME="dev.omt.apw.bridge.chromium"
EXTENSION_ID="ajefblkpgcffjgeaifmhaekckngflbak"
DEFAULT_HELPER_PATH="/System/Cryptexes/App/System/Library/CoreServices/PasswordManagerBrowserExtensionHelper.app/Contents/MacOS/PasswordManagerBrowserExtensionHelper"
SYSTEM_CHROME_MANIFEST="/Library/Google/Chrome/NativeMessagingHosts/com.apple.passwordmanager.json"
USER_CHROME_MANIFEST_DIR="$HOME/Library/Application Support/Google/Chrome/NativeMessagingHosts"
TARGET_MANIFEST="$USER_CHROME_MANIFEST_DIR/${HOST_NAME}.json"

print_help() {
  cat <<'EOF'
Usage: ./scripts/install-browser-bridge.sh [--helper-path PATH]

Install the per-user Chrome native messaging manifest used by the APW browser bridge.
This does not modify Apple's system manifests. Load the unpacked extension from
browser-bridge/ in chrome://extensions after this script succeeds.
EOF
}

HELPER_PATH=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --helper-path)
      if [[ -z "${2:-}" ]]; then
        echo "--helper-path requires a value."
        exit 1
      fi
      HELPER_PATH="$2"
      shift 2
      ;;
    --help)
      print_help
      exit 0
      ;;
    *)
      echo "Unknown argument: $1"
      print_help
      exit 1
      ;;
  esac
done

if [[ -z "$HELPER_PATH" ]] && [[ -f "$SYSTEM_CHROME_MANIFEST" ]]; then
  HELPER_PATH="$(
    ruby -rjson -e 'manifest = JSON.parse(File.read(ARGV[0])); puts manifest.fetch("path")' "$SYSTEM_CHROME_MANIFEST"
  )"
fi

if [[ -z "$HELPER_PATH" ]]; then
  HELPER_PATH="$DEFAULT_HELPER_PATH"
fi

if [[ ! -x "$HELPER_PATH" ]]; then
  echo "Helper binary is not executable: $HELPER_PATH"
  exit 1
fi

mkdir -p "$USER_CHROME_MANIFEST_DIR"

cat >"$TARGET_MANIFEST" <<EOF
{
  "name": "$HOST_NAME",
  "description": "APW Chrome bridge to Apple's PasswordManagerBrowserExtensionHelper",
  "path": "$HELPER_PATH",
  "type": "stdio",
  "allowed_origins": [
    "chrome-extension://$EXTENSION_ID/"
  ]
}
EOF

echo "Installed Chrome native messaging manifest:"
echo "  $TARGET_MANIFEST"
echo
echo "Next steps:"
echo "  1. Open chrome://extensions"
echo "  2. Enable Developer mode"
echo "  3. Click 'Load unpacked' and select:"
echo "       $EXTENSION_DIR"
echo "  4. Confirm the extension ID is:"
echo "       $EXTENSION_ID"
echo "  5. Start the daemon with:"
echo "       apw start --runtime-mode browser --port 10000"
echo "  6. Open the extension popup and keep host=127.0.0.1 port=10000 unless you intentionally changed the daemon bind."
