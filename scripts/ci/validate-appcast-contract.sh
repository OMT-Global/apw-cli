#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DOC_PATH="$ROOT_DIR/docs/IN_APP_UPDATES.md"
TEMPLATE_PATH="$ROOT_DIR/packaging/sparkle/appcast.template.xml"
FEED_URL="https://github.com/OMT-Global/apw-cli/releases/latest/download/appcast.xml"
MDM_KEY="com.omt.apw.updatesDisabled"

require_file() {
  if [ ! -f "$1" ]; then
    echo "Missing required appcast contract file: $1" >&2
    exit 1
  fi
}

require_pattern() {
  file="$1"
  pattern="$2"
  description="$3"
  if ! grep -Eq "$pattern" "$file"; then
    echo "Missing appcast contract requirement in $file: $description" >&2
    exit 1
  fi
}

require_file "$DOC_PATH"
require_file "$TEMPLATE_PATH"

require_pattern "$DOC_PATH" "Sparkle 2" "Sparkle 2 decision"
require_pattern "$DOC_PATH" "$FEED_URL" "stable project-controlled feed URL"
require_pattern "$DOC_PATH" "SUFeedURL" "Info.plist feed key"
require_pattern "$DOC_PATH" "SUPublicEDKey" "Sparkle EdDSA public key"
require_pattern "$DOC_PATH" "SUVerifyUpdateBeforeExtraction=true" "pre-extraction update verification"
require_pattern "$DOC_PATH" "SURequireSignedFeed=true" "signed appcast enforcement"
require_pattern "$DOC_PATH" "$MDM_KEY" "managed preference to disable updates"
require_pattern "$DOC_PATH" "codesign --deep --strict --verify APW\\.app" "codesign release gate"
require_pattern "$DOC_PATH" "spctl --assess --type execute --verbose APW\\.app" "Gatekeeper release gate"
require_pattern "$DOC_PATH" "xcrun stapler validate APW\\.app" "notarization staple release gate"
require_pattern "$DOC_PATH" "sparkle:criticalUpdate" "security update appcast marker"

require_pattern "$TEMPLATE_PATH" "xmlns:sparkle=\"http://www\\.andymatuschak\\.org/xml-namespaces/sparkle\"" "Sparkle namespace"
require_pattern "$TEMPLATE_PATH" "<title>APW [0-9]+\\.[0-9]+\\.[0-9]+ Security Update</title>" "security update title"
require_pattern "$TEMPLATE_PATH" "<sparkle:version>[0-9]+\\.[0-9]+\\.[0-9]+</sparkle:version>" "machine version"
require_pattern "$TEMPLATE_PATH" "sparkle:releaseNotesLink sparkle:edSignature=" "signed release notes link"
require_pattern "$TEMPLATE_PATH" "<sparkle:criticalUpdate" "critical update marker"
require_pattern "$TEMPLATE_PATH" "url=\"https://github\\.com/OMT-Global/apw-cli/releases/download/v[0-9]+\\.[0-9]+\\.[0-9]+/APW\\.app\\.zip\"" "release archive URL"
require_pattern "$TEMPLATE_PATH" "sparkle:edSignature=" "signed archive enclosure"
require_pattern "$TEMPLATE_PATH" "length=\"[0-9]+\"" "archive length"

if command -v xmllint >/dev/null 2>&1; then
  xmllint --noout "$TEMPLATE_PATH"
fi

echo "Appcast contract validation passed."
