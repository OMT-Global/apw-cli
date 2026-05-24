#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="$(mktemp -d)"
BIN_PATH="$ROOT_DIR/rust/target/release/apw"
APP_PATH="$ROOT_DIR/native-app/dist/APW.app"
DMG_PATH="$ROOT_DIR/dist/apw-macos-v9.9.9.dmg"
CHECKSUM_PATH="$DMG_PATH.sha256"
cleanup() {
  rm -f "$DMG_PATH" "$CHECKSUM_PATH"
  rm -rf "$APP_PATH"
  rm -f "$BIN_PATH"
  if [[ -f "$WORK_DIR/apw.backup" ]]; then
    mkdir -p "$(dirname "$BIN_PATH")"
    mv "$WORK_DIR/apw.backup" "$BIN_PATH"
  fi
  if [[ -d "$WORK_DIR/APW.app.backup" ]]; then
    mkdir -p "$(dirname "$APP_PATH")"
    mv "$WORK_DIR/APW.app.backup" "$APP_PATH"
  fi
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
  srcfolder=""
  for ((i = 1; i <= $#; i++)); do
    if [[ "${!i}" == "-srcfolder" ]]; then
      next=$((i + 1))
      srcfolder="${!next}"
      break
    fi
  done
  if [[ -z "$srcfolder" ]]; then
    echo "missing -srcfolder" >&2
    exit 1
  fi
  test -x "$srcfolder/bin/apw"
  test -f "$srcfolder/APW.app/Contents/Info.plist"
  test -x "$srcfolder/APW.app/Contents/MacOS/APW"
  test -L "$srcfolder/Applications"
  [[ "$(readlink "$srcfolder/Applications")" == "/Applications" ]]
  output="${@: -1}"
  mkdir -p "$(dirname "$output")"
  printf 'fake dmg\n' > "$output"
fi
EOF
chmod +x "$fake_hdiutil"

if [[ -f "$BIN_PATH" ]]; then
  mv "$BIN_PATH" "$WORK_DIR/apw.backup"
fi
if [[ -d "$APP_PATH" ]]; then
  mv "$APP_PATH" "$WORK_DIR/APW.app.backup"
fi

mkdir -p \
  "$(dirname "$BIN_PATH")" \
  "$APP_PATH/Contents/MacOS"

printf '#!/usr/bin/env bash\necho apw\n' > "$BIN_PATH"
chmod +x "$BIN_PATH"
printf '#!/usr/bin/env bash\necho APW\n' > "$APP_PATH/Contents/MacOS/APW"
chmod +x "$APP_PATH/Contents/MacOS/APW"
printf '<plist version="1.0"><dict></dict></plist>\n' > "$APP_PATH/Contents/Info.plist"

rm -f "$DMG_PATH" "$CHECKSUM_PATH"

FAKE_HDIUTIL_LOG="$fake_log" \
HDIUTIL_BIN="$fake_hdiutil" \
APW_SKIP_DMG_MOUNT_SMOKE=1 \
  "$ROOT_DIR/scripts/package-release-dmg.sh" v9.9.9 >"$WORK_DIR/package.out"

test -f "$DMG_PATH"
test -f "$CHECKSUM_PATH"
grep -q "create" "$fake_log"
grep -Eq "^[0-9a-f]{64}  apw-macos-v9.9.9.dmg$" "$CHECKSUM_PATH"
