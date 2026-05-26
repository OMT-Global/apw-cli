#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

fake_hdiutil="$WORK_DIR/hdiutil"
fake_log="$WORK_DIR/hdiutil.log"

cat > "$fake_hdiutil" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "$FAKE_HDIUTIL_LOG"
if [[ "${1:-}" == "create" ]]; then
  output="${@: -1}"
  mkdir -p "$(dirname "$output")"
  printf 'fake dmg\n' > "$output"
fi
EOF
chmod +x "$fake_hdiutil"

mkdir -p \
  "$ROOT_DIR/rust/target/release" \
  "$ROOT_DIR/native-app/dist/APW.app/Contents/MacOS"

printf '#!/usr/bin/env bash\necho apw\n' > "$ROOT_DIR/rust/target/release/apw"
chmod +x "$ROOT_DIR/rust/target/release/apw"
printf '#!/usr/bin/env bash\necho APW\n' > "$ROOT_DIR/native-app/dist/APW.app/Contents/MacOS/APW"
chmod +x "$ROOT_DIR/native-app/dist/APW.app/Contents/MacOS/APW"
printf '<plist version="1.0"><dict></dict></plist>\n' > "$ROOT_DIR/native-app/dist/APW.app/Contents/Info.plist"

rm -f "$ROOT_DIR/dist/apw-macos-v9.9.9.dmg" "$ROOT_DIR/dist/apw-macos-v9.9.9.dmg.sha256"

FAKE_HDIUTIL_LOG="$fake_log" \
HDIUTIL_BIN="$fake_hdiutil" \
APW_SKIP_DMG_MOUNT_SMOKE=1 \
  "$ROOT_DIR/scripts/package-release-dmg.sh" v9.9.9 >"$WORK_DIR/package.out"

test -f "$ROOT_DIR/dist/apw-macos-v9.9.9.dmg"
test -f "$ROOT_DIR/dist/apw-macos-v9.9.9.dmg.sha256"
grep -q "create" "$fake_log"
grep -Eq "^[0-9a-f]{64}  apw-macos-v9.9.9.dmg$" "$ROOT_DIR/dist/apw-macos-v9.9.9.dmg.sha256"

rm -f "$ROOT_DIR/dist/apw-macos-v9.9.9.dmg" "$ROOT_DIR/dist/apw-macos-v9.9.9.dmg.sha256"
