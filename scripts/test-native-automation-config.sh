#!/usr/bin/env bash
set -euo pipefail

SDEF="native-app/Resources/APW.sdef"
APP_INTENTS="native-app/Sources/APW/APWAutomationIntents.swift"
APPLE_SCRIPT_COMMANDS="native-app/Sources/APW/APWAppleScriptCommands.swift"
BROKER_CORE_TESTS="native-app/Tests/NativeAppTests/BrokerCoreTests.swift"
THREAT_MODEL="docs/THREAT_MODEL.md"
SECURITY_POSTURE="docs/SECURITY_POSTURE_AND_TESTING.md"
ENTITLEMENTS="native-app/APW.entitlements"
BUILD_SCRIPT="scripts/build-native-app.sh"
PLIST_RENDERER="scripts/render-native-app-info-plist.sh"

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
require_line "$SDEF" '<cocoa class="APWRequestLoginCommand"/>' "AppleScript login command must be wired to a Cocoa command class."
require_line "$SDEF" '<cocoa class="APWRequestFillCommand"/>' "AppleScript fill command must be wired to a Cocoa command class."
require_line "$SDEF" "user still approves" "AppleScript dictionary must document user mediation."

require_line "$APP_INTENTS" "struct APWLoginIntent" "Shortcuts login intent is missing."
require_line "$APP_INTENTS" "struct APWFillIntent" "Shortcuts fill intent is missing."
require_line "$APP_INTENTS" "AppShortcutsProvider" "Shortcuts provider is missing."
require_line "$APP_INTENTS" "BrokerAutomation.performResponseDataAsync" "Shortcuts intents must route through asynchronous BrokerAutomation."
if grep -Fq "@MainActor" "$APP_INTENTS"; then
  echo "Shortcuts intents must not run broker requests synchronously on MainActor." >&2
  exit 1
fi

require_line "$APPLE_SCRIPT_COMMANDS" "final class APWRequestLoginCommand: NSScriptCommand" "AppleScript login command implementation is missing."
require_line "$APPLE_SCRIPT_COMMANDS" "final class APWRequestFillCommand: NSScriptCommand" "AppleScript fill command implementation is missing."
require_line "$APPLE_SCRIPT_COMMANDS" "BrokerAutomation.performResponseData" "AppleScript commands must route through BrokerAutomation."
require_line "$APPLE_SCRIPT_COMMANDS" "performOffMainThreadWhilePumpingRunLoop" "AppleScript commands must avoid blocking the main AppKit event loop."

require_line "$BUILD_SCRIPT" 'cp "$PACKAGE_DIR/Resources/APW.sdef" "$RESOURCES_DIR/APW.sdef"' "APW.sdef must be copied into the app bundle."
require_line "$PLIST_RENDERER" "<key>NSAppleScriptEnabled</key>" "App bundle must enable AppleScript."
require_line "$PLIST_RENDERER" "<key>OSAScriptingDefinition</key>" "App bundle must publish the scripting definition."
if grep -Fq "com.apple.security.app-sandbox" "$ENTITLEMENTS"; then
  require_line "$THREAT_MODEL" "App Sandbox" "Threat model must describe the sandboxed automation posture when sandbox entitlement is present."
else
  require_line "$THREAT_MODEL" "not sandboxed" "Threat model must document APW.app automation when App Sandbox is absent."
  require_line "$THREAT_MODEL" "prompt fatigue" "Threat model must document prompt-fatigue risk for scriptable automation."
  require_line "$SECURITY_POSTURE" "without the App" "Security posture must document the current unsandboxed automation surface."
  require_line "$SECURITY_POSTURE" "rate limiting/coalescing" "Security posture must track prompt-fatigue rate limiting or coalescing follow-up."
fi
require_line "native-app/Sources/NativeAppLib/BrokerCore.swift" "runScriptableBrokerApp" "Scriptable app launches must run an AppKit event loop."
require_line "native-app/Sources/NativeAppLib/BrokerCore.swift" "NSApplication.shared" "Scriptable app launches must initialize NSApplication."

require_line "$BROKER_CORE_TESTS" "testAutomationEnvelopeMatchesBrokerRequestContract" "Automation envelope parity test is missing."
require_line "$BROKER_CORE_TESTS" "testAutomationResponseUsesInjectedBrokerServer" "Automation broker dispatch test is missing."
require_line "$BROKER_CORE_TESTS" "testAutomationRejectsNonHTTPSURLsBeforeBrokerDispatch" "Automation URL rejection test is missing."

echo "Native automation configuration test passed."
