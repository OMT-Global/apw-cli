#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

fake_lipo="$WORK_DIR/lipo"
fake_cli="$WORK_DIR/apw"
fake_app="$WORK_DIR/APW"
fake_missing_cli="$WORK_DIR/missing-apw"
fake_log="$WORK_DIR/lipo.log"

cat >"$fake_lipo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "$FAKE_LIPO_LOG"
if [[ "${1:-}" == "-archs" ]]; then
  case "${2:-}" in
    *missing*)
      echo "arm64"
      ;;
    *)
      echo "arm64 x86_64"
      ;;
  esac
else
  echo "unexpected lipo arguments: $*" >&2
  exit 1
fi
EOF
chmod +x "$fake_lipo"
printf '#!/usr/bin/env bash\n' >"$fake_cli"
printf '#!/usr/bin/env bash\n' >"$fake_app"
printf '#!/usr/bin/env bash\n' >"$fake_missing_cli"
chmod +x "$fake_cli" "$fake_app" "$fake_missing_cli"

FAKE_LIPO_LOG="$fake_log" LIPO_BIN="$fake_lipo" \
  "$ROOT_DIR/scripts/verify-universal-binaries.sh" "$fake_cli" "$fake_app" >"$WORK_DIR/pass.out"
grep -q "apw: arm64 x86_64" "$WORK_DIR/pass.out"
grep -q "APW.app: arm64 x86_64" "$WORK_DIR/pass.out"

if FAKE_LIPO_LOG="$fake_log" LIPO_BIN="$fake_lipo" \
  "$ROOT_DIR/scripts/verify-universal-binaries.sh" "$fake_missing_cli" "$fake_app" \
  >"$WORK_DIR/fail.out" 2>"$WORK_DIR/fail.err"; then
  echo "verify-universal-binaries accepted a missing architecture." >&2
  exit 1
fi
grep -q "expected arm64 and x86_64" "$WORK_DIR/fail.err"
