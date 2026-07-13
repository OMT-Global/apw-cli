#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/apw-sparkle-test.XXXXXX")"
trap 'rm -rf "$WORK_DIR"' EXIT

archive="$WORK_DIR/APW.app.zip"
notes="$WORK_DIR/APW.app.release.md"
updates="$WORK_DIR/updates"
fake_generate="$WORK_DIR/generate_appcast"
download_prefix="https://github.com/OMT-Global/apw-cli/releases/download/v2.0.0/"
release_url="https://github.com/OMT-Global/apw-cli/releases/tag/v2.0.0"

printf 'fake notarized archive\n' >"$archive"
cat >"$notes" <<'NOTES'
# APW 2.0.0 Security Update

## Security

- Exercise signed Sparkle appcast generation.
NOTES

cat >"$fake_generate" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail

updates_dir=""
critical_update=false
for argument in "$@"; do
  updates_dir="$argument"
  if [ "$argument" = "--critical-update-version" ]; then
    critical_update=true
  fi
done
printf '%s\n' "$@" >"$updates_dir/generate_appcast.args"

critical_element=""
if [ "$critical_update" = true ]; then
  critical_element='      <sparkle:criticalUpdate />'
fi

cat >"$updates_dir/appcast.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <item>
      <title>APW 2.0.0 Security Update</title>
      <sparkle:releaseNotesLink sparkle:edSignature="notes-signed">https://github.com/OMT-Global/apw-cli/releases/tag/v2.0.0</sparkle:releaseNotesLink>
      <link>https://github.com/OMT-Global/apw-cli/releases/tag/v2.0.0</link>
$critical_element
      <enclosure url="https://github.com/OMT-Global/apw-cli/releases/download/v2.0.0/APW.app.zip" sparkle:edSignature="signed" length="22" type="application/octet-stream" />
    </item>
  </channel>
</rss>
XML
FAKE
chmod +x "$fake_generate"

APW_SPARKLE_PRIVATE_ED_KEY="test-ed25519-private-key" \
  "$ROOT_DIR/scripts/prepare-sparkle-appcast.sh" \
  --archive "$archive" \
  --release-notes "$notes" \
  --updates-dir "$updates" \
  --generate-appcast "$fake_generate" \
  --download-url-prefix "$download_prefix" \
  --release-url "$release_url" \
  --critical-update-version "1.9.9"

[ -f "$updates/APW.app.zip" ]
[ -f "$updates/APW.app.zip.md" ]
[ -f "$updates/appcast.xml" ]
grep -q 'sparkle:edSignature="signed"' "$updates/appcast.xml"
grep -q 'sparkle:edSignature="notes-signed"' "$updates/appcast.xml"
grep -Fx -- '--download-url-prefix' "$updates/generate_appcast.args" >/dev/null
grep -Fx -- "$download_prefix" "$updates/generate_appcast.args" >/dev/null
grep -Fx -- '--link' "$updates/generate_appcast.args" >/dev/null
grep -Fx -- "$release_url" "$updates/generate_appcast.args" >/dev/null
grep -Fx -- '--critical-update-version' "$updates/generate_appcast.args" >/dev/null
grep -Fx -- '1.9.9' "$updates/generate_appcast.args" >/dev/null
grep -Fx -- '--ed-key-file' "$updates/generate_appcast.args" >/dev/null
grep -Fx -- '-' "$updates/generate_appcast.args" >/dev/null

if "$ROOT_DIR/scripts/prepare-sparkle-appcast.sh" \
  --archive "$archive" \
  --release-notes "$notes" \
  --updates-dir "$WORK_DIR/missing-private-key" \
  --generate-appcast "$fake_generate" \
  >"$WORK_DIR/missing-private-key.out" 2>"$WORK_DIR/missing-private-key.err"; then
  echo "prepare-sparkle-appcast accepted a missing ephemeral private key." >&2
  exit 1
fi
grep -q "APW_SPARKLE_PRIVATE_ED_KEY is required" "$WORK_DIR/missing-private-key.err"

unsigned_notes_generate="$WORK_DIR/generate_unsigned_notes_appcast"
cat >"$unsigned_notes_generate" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail

updates_dir=""
for argument in "$@"; do
  updates_dir="$argument"
done
cat >"$updates_dir/appcast.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <item>
      <title>APW 2.0.0 Security Update</title>
      <sparkle:releaseNotesLink>https://github.com/OMT-Global/apw-cli/releases/tag/v2.0.0</sparkle:releaseNotesLink>
      <enclosure url="https://github.com/OMT-Global/apw-cli/releases/download/v2.0.0/APW.app.zip" sparkle:edSignature="signed" length="22" type="application/octet-stream" />
    </item>
  </channel>
</rss>
XML
FAKE
chmod +x "$unsigned_notes_generate"

if APW_SPARKLE_PRIVATE_ED_KEY="test-ed25519-private-key" \
  "$ROOT_DIR/scripts/prepare-sparkle-appcast.sh" \
  --archive "$archive" \
  --release-notes "$notes" \
  --updates-dir "$WORK_DIR/unsigned-notes" \
  --generate-appcast "$unsigned_notes_generate" \
  >"$WORK_DIR/unsigned-notes.out" 2>"$WORK_DIR/unsigned-notes.err"; then
  echo "prepare-sparkle-appcast accepted unsigned release notes." >&2
  exit 1
fi
if ! grep -q "unsigned Sparkle release notes" "$WORK_DIR/unsigned-notes.err"; then
  cat "$WORK_DIR/unsigned-notes.err" >&2
  exit 1
fi

missing_security_notes="$WORK_DIR/APW.app.no-security.md"
cat >"$missing_security_notes" <<'NOTES'
# APW 2.0.0 Update

## Changes

- Missing the security section required for critical updates.
NOTES

if APW_SPARKLE_PRIVATE_ED_KEY="test-ed25519-private-key" \
  "$ROOT_DIR/scripts/prepare-sparkle-appcast.sh" \
  --archive "$archive" \
  --release-notes "$missing_security_notes" \
  --updates-dir "$WORK_DIR/missing-security" \
  --generate-appcast "$fake_generate" \
  --critical-update-version "1.9.9" \
  >"$WORK_DIR/missing-security.out" 2>"$WORK_DIR/missing-security.err"; then
  echo "prepare-sparkle-appcast accepted critical update notes without a Security section." >&2
  exit 1
fi
grep -q "critical Sparkle updates require a Security section" "$WORK_DIR/missing-security.err"

echo "Sparkle appcast preparation test passed."
