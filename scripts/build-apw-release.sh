#!/usr/bin/env bash
set -euo pipefail

print_help() {
  cat <<'EOF'
Usage: ./scripts/build-apw-release.sh [--install|--no-install] [--brew-smoke|--no-brew-smoke]
                                    [--install-dir /usr/local/bin] [--skip-version]
                                    [--archive-smoke|--no-archive-smoke]

Options:
  --install              install apw to --install-dir (defaults to /usr/local/bin)
  --no-install           skip installation (default)
  --install-dir PATH     destination directory for installation (default /usr/local/bin)
  --brew-smoke           run local Homebrew source smoke test
  --no-brew-smoke        skip Homebrew smoke test (default)
  --skip-version         skip --version and status checks
  --archive-smoke        smoke test the generated release archive (default)
  --no-archive-smoke     skip release archive smoke test
  -h, --help             show this help message

Examples:
  ./scripts/build-apw-release.sh
  ./scripts/build-apw-release.sh --install
  ./scripts/build-apw-release.sh --install --install-dir "$HOME/.local/bin" --brew-smoke
EOF
}

ROOT_DIR="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_PATH="$ROOT_DIR/rust/target/release/apw"
CARGO_MANIFEST="$ROOT_DIR/rust/Cargo.toml"
APP_BUNDLE_PATH="$ROOT_DIR/native-app/dist/APW.app"
VERSION="$(awk -F ' = ' '/^version = / {gsub(/"/, "", $2); print $2; exit}' "$CARGO_MANIFEST")"
ARCHIVE_PATH="$ROOT_DIR/dist/apw-macos-v${VERSION}.tar.gz"
INSTALL_BIN=0
INSTALL_DIR="/usr/local/bin"
BREW_SMOKE=0
SKIP_VERSION_CHECK=0
ARCHIVE_SMOKE=1

if [[ -z "$VERSION" ]]; then
  echo "Unable to read version from $CARGO_MANIFEST"
  exit 1
fi

while [[ $# -gt 0 ]]; do
  case "$1" in
    --install)
      INSTALL_BIN=1
      ;;
    --no-install)
      INSTALL_BIN=0
      ;;
    --install-dir)
      if [[ -z "${2:-}" ]]; then
        echo "--install-dir requires a path."
        print_help
        exit 1
      fi
      INSTALL_DIR="$2"
      shift
      ;;
    --brew-smoke)
      BREW_SMOKE=1
      ;;
    --no-brew-smoke)
      BREW_SMOKE=0
      ;;
    --skip-version)
      SKIP_VERSION_CHECK=1
      ;;
    --archive-smoke)
      ARCHIVE_SMOKE=1
      ;;
    --no-archive-smoke)
      ARCHIVE_SMOKE=0
      ;;
    -h|--help)
      print_help
      exit 0
    ;;
    *)
      echo "Unknown argument: $1"
      print_help
      exit 1
      ;;
  esac
  shift
done

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install Rust: https://rustup.rs/"
  exit 1
fi

if [ ! -f "$CARGO_MANIFEST" ]; then
  echo "Expected manifest not found: $CARGO_MANIFEST"
  exit 1
fi

printf '\n[1/5] Building APW app bundle...\n'
cd "$ROOT_DIR"
"$ROOT_DIR/scripts/build-native-app.sh"

printf '\n[2/5] Building release binary...\n'
cd "$ROOT_DIR"
cargo build --manifest-path "$CARGO_MANIFEST" --release

printf '\n[3/5] Packaging release archive...\n'
rm -rf "$ROOT_DIR/dist/apw" "$ROOT_DIR/dist/APW.app"
mkdir -p "$ROOT_DIR/dist"
cp "$BIN_PATH" "$ROOT_DIR/dist/apw"
cp -R "$APP_BUNDLE_PATH" "$ROOT_DIR/dist/APW.app"
tar -czf "$ARCHIVE_PATH" -C "$ROOT_DIR/dist" apw APW.app
rm -rf "$ROOT_DIR/dist/apw" "$ROOT_DIR/dist/APW.app"
echo "Created: $ARCHIVE_PATH"

if [ "$SKIP_VERSION_CHECK" -ne 1 ]; then
  printf '\n[4/5] Validating binary health...\n'
  "$BIN_PATH" --version
  "$BIN_PATH" status --json
fi

if [ "$ARCHIVE_SMOKE" -eq 1 ]; then
  printf '\n[5/5] Validating release archive smoke...\n'
  smoke_dir="$(mktemp -d)"
  smoke_home="$(mktemp -d)"
  cleanup_smoke() {
    rm -rf "$smoke_dir" "$smoke_home"
  }
  trap cleanup_smoke EXIT
  tar -xzf "$ARCHIVE_PATH" -C "$smoke_dir"
  test -x "$smoke_dir/apw"
  test -x "$smoke_dir/APW.app/Contents/MacOS/APW"
  HOME="$smoke_home" "$smoke_dir/apw" --version
  HOME="$smoke_home" "$smoke_dir/apw" status --json
  HOME="$smoke_home" "$smoke_dir/apw" app install
  trap - EXIT
  cleanup_smoke
fi

if [ "$INSTALL_BIN" -eq 1 ]; then
  printf '\nInstalling to %s...\n' "$INSTALL_DIR"
  if [ ! -d "$INSTALL_DIR" ]; then
    echo "Install directory does not exist: $INSTALL_DIR"
    exit 1
  fi
  sudo cp "$BIN_PATH" "$INSTALL_DIR/apw"
  sudo chmod +x "$INSTALL_DIR/apw"
  echo "Installed: $INSTALL_DIR/apw"
  "$INSTALL_DIR/apw" --version
fi

if [ "$BREW_SMOKE" -eq 1 ]; then
  printf '\nRunning Homebrew source smoke test...\n'
  if [ -f "$ROOT_DIR/packaging/homebrew/install-from-source.sh" ]; then
    "$ROOT_DIR/packaging/homebrew/install-from-source.sh"
  else
    echo "Homebrew smoke script not found: packaging/homebrew/install-from-source.sh"
    exit 1
  fi
fi

printf '\nBuild script complete.\n'
