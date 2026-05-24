#!/usr/bin/env bash
set -euo pipefail

SDEF="native-app/Resources/APW.sdef"
APP_INTENTS="native-app/Sources/APW/APWAutomationIntents.swift"
BROKER_CORE_TESTS="native-app/Tests/NativeAppTests/BrokerCoreTests.swift"
BUILD_SCRIPT="scripts/build-native-app.sh"

require_line() {
  local file="$1"
  local pattern="$2"
  local message="$3"
  if ! grep -Fq "$pattern" "$file"; then
    echo "$message" >&2
    exit 1
  fi
}

if [ ! -f "$SDEF" ]; then
  echo "Missing AppleScript dictionary: $SDEF" >&2
  exit 1
fi

python3 - "$SDEF" <<'PY'
import sys
import xml.etree.ElementTree as ET

ET.parse(sys.argv[1])
PY

require_line "$SDEF" 'command name="request login"' "AppleScript dictionary must expose request login."
require_line "$SDEF" 'command name="request fill"' "AppleScript dictionary must expose request fill."
require_line "$SDEF" "user still approves" "AppleScript dictionary must document user mediation."

require_line "$APP_INTENTS" "struct APWLoginIntent" "Shortcuts login intent is missing."
require_line "$APP_INTENTS" "struct APWFillIntent" "Shortcuts fill intent is missing."
require_line "$APP_INTENTS" "AppShortcutsProvider" "Shortcuts provider is missing."
require_line "$APP_INTENTS" "BrokerAutomation.performResponseData" "Shortcuts intents must route through BrokerAutomation."

require_line "$BUILD_SCRIPT" 'cp "$PACKAGE_DIR/Resources/APW.sdef" "$RESOURCES_DIR/APW.sdef"' "APW.sdef must be copied into the app bundle."
require_line "$BUILD_SCRIPT" "<key>NSAppleScriptEnabled</key>" "App bundle must enable AppleScript."
require_line "$BUILD_SCRIPT" "<key>OSAScriptingDefinition</key>" "App bundle must publish the scripting definition."

require_line "$BROKER_CORE_TESTS" "testAutomationEnvelopeMatchesBrokerRequestContract" "Automation envelope parity test is missing."
require_line "$BROKER_CORE_TESTS" "testAutomationResponseUsesInjectedBrokerServer" "Automation broker dispatch test is missing."
require_line "$BROKER_CORE_TESTS" "testAutomationRejectsNonHTTPSURLsBeforeBrokerDispatch" "Automation URL rejection test is missing."

echo "Native automation configuration test passed."
