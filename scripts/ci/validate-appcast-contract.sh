#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DOC_PATH="$ROOT_DIR/docs/IN_APP_UPDATES.md"
TEMPLATE_PATH="$ROOT_DIR/packaging/sparkle/appcast.template.xml"
PREPARE_SCRIPT="$ROOT_DIR/scripts/prepare-sparkle-appcast.sh"
PREPARE_TEST="$ROOT_DIR/scripts/test-prepare-sparkle-appcast.sh"
RELEASE_WORKFLOW="$ROOT_DIR/.github/workflows/release.yml"
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
require_file "$PREPARE_SCRIPT"
require_file "$PREPARE_TEST"
require_file "$RELEASE_WORKFLOW"

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
require_pattern "$DOC_PATH" "prepare-sparkle-appcast\\.sh" "release appcast preparation helper"
require_pattern "$DOC_PATH" "generate_appcast" "Sparkle appcast generation tool"
require_pattern "$DOC_PATH" "SPARKLE_GENERATE_APPCAST" "release runner generate_appcast configuration"

require_pattern "$TEMPLATE_PATH" "xmlns:sparkle=\"http://www\\.andymatuschak\\.org/xml-namespaces/sparkle\"" "Sparkle namespace"
require_pattern "$TEMPLATE_PATH" "<title>APW [0-9]+\\.[0-9]+\\.[0-9]+ Security Update</title>" "security update title"
require_pattern "$TEMPLATE_PATH" "<sparkle:version>[0-9]+\\.[0-9]+\\.[0-9]+</sparkle:version>" "machine version"
require_pattern "$TEMPLATE_PATH" "sparkle:releaseNotesLink sparkle:edSignature=" "signed release notes link"
require_pattern "$TEMPLATE_PATH" "<sparkle:criticalUpdate" "critical update marker"
require_pattern "$TEMPLATE_PATH" "url=\"https://github\\.com/OMT-Global/apw-cli/releases/download/v[0-9]+\\.[0-9]+\\.[0-9]+/APW\\.app\\.zip\"" "release archive URL"
require_pattern "$TEMPLATE_PATH" "sparkle:edSignature=" "signed archive enclosure"
require_pattern "$TEMPLATE_PATH" "length=\"[0-9]+\"" "archive length"

require_pattern "$PREPARE_SCRIPT" "generate_appcast" "Sparkle appcast generation invocation"
require_pattern "$PREPARE_SCRIPT" "sparkle:edSignature=" "signed appcast output enforcement"
require_pattern "$PREPARE_SCRIPT" "Do not pass private keys" "private key handling guardrail"
require_pattern "$PREPARE_TEST" "Sparkle appcast preparation test passed" "helper regression test"
require_pattern "$RELEASE_WORKFLOW" "prepare-sparkle-appcast\\.sh" "release appcast preparation step"
require_pattern "$RELEASE_WORKFLOW" "SPARKLE_GENERATE_APPCAST" "release appcast generator variable"
require_pattern "$RELEASE_WORKFLOW" "dist/appcast\\.xml" "release appcast asset upload"
require_pattern "$RELEASE_WORKFLOW" "APW\\.app-\\$\\{\\{ github\\.ref_name \\}\\}\\.zip" "release Sparkle app archive upload"

if command -v xmllint >/dev/null 2>&1; then
  xmllint --noout "$TEMPLATE_PATH"
fi

echo "Appcast contract validation passed."
