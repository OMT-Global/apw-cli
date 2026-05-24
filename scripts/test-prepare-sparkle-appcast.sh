#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/apw-sparkle-test.XXXXXX")"
trap 'rm -rf "$WORK_DIR"' EXIT

archive="$WORK_DIR/APW.app.zip"
notes="$WORK_DIR/APW.app.release.md"
updates="$WORK_DIR/updates"
fake_generate="$WORK_DIR/generate_appcast"

printf 'fake notarized archive\n' >"$archive"
cat >"$notes" <<'NOTES'
# APW 2.0.0 Security Update

## Security

- Exercise signed Sparkle appcast generation.
NOTES

cat >"$fake_generate" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail

updates_dir="$1"
cat >"$updates_dir/appcast.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <item>
      <title>APW 2.0.0 Security Update</title>
      <enclosure url="https://github.com/OMT-Global/apw-cli/releases/download/v2.0.0/APW.app.zip" sparkle:edSignature="signed" length="22" type="application/octet-stream" />
    </item>
  </channel>
</rss>
XML
FAKE
chmod +x "$fake_generate"

"$ROOT_DIR/scripts/prepare-sparkle-appcast.sh" \
  --archive "$archive" \
  --release-notes "$notes" \
  --updates-dir "$updates" \
  --generate-appcast "$fake_generate"

[ -f "$updates/APW.app.zip" ]
[ -f "$updates/APW.app.zip.md" ]
[ -f "$updates/appcast.xml" ]
grep -q 'sparkle:edSignature="signed"' "$updates/appcast.xml"

echo "Sparkle appcast preparation test passed."
