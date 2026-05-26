#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: ./scripts/render-native-app-info-plist.sh OUTPUT VERSION EXECUTABLE_NAME

Render APW.app's Info.plist. When APW_SPARKLE_PUBLIC_ED_KEY is set, the plist
also includes the Sparkle update keys required by docs/IN_APP_UPDATES.md.
USAGE
}

if [ "$#" -ne 3 ]; then
  usage >&2
  exit 2
fi

OUTPUT_PATH="$1"
VERSION="$2"
EXECUTABLE_NAME="$3"
SPARKLE_FEED_URL="https://github.com/OMT-Global/apw-cli/releases/latest/download/appcast.xml"
SPARKLE_PUBLIC_ED_KEY="${APW_SPARKLE_PUBLIC_ED_KEY:-}"

mkdir -p "$(dirname "$OUTPUT_PATH")"

{
  cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>$EXECUTABLE_NAME</string>
  <key>CFBundleIdentifier</key>
  <string>dev.omt.apw</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>APW</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundleVersion</key>
  <string>$VERSION</string>
  <key>LSUIElement</key>
  <true/>
EOF

  if [ -n "$SPARKLE_PUBLIC_ED_KEY" ]; then
    cat <<EOF
  <key>SUFeedURL</key>
  <string>$SPARKLE_FEED_URL</string>
  <key>SUPublicEDKey</key>
  <string>$SPARKLE_PUBLIC_ED_KEY</string>
  <key>SUVerifyUpdateBeforeExtraction</key>
  <true/>
  <key>SURequireSignedFeed</key>
  <true/>
  <key>SUEnableAutomaticChecks</key>
  <true/>
  <key>SUAllowsAutomaticUpdates</key>
  <false/>
  <key>SUAutomaticallyUpdate</key>
  <false/>
EOF
  fi

  cat <<'EOF'
</dict>
</plist>
EOF
} >"$OUTPUT_PATH"
