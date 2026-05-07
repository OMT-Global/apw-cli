#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TAP_NAME="local/apw-smoke-$(date +%s)"
FORMULA_NAME="apw"
CLEANUP_TAP_PATH="$(brew --repository)/Library/Taps/$(dirname "$TAP_NAME")/homebrew-$(basename "$TAP_NAME")"
ARCHIVE_PATH="$(mktemp -u /tmp/apw-src-tarball.XXXXXX).tar.gz"
CLEANUP_DONE=0
VERSION="$(awk -F ' = ' '/^version = / {gsub(/"/, "", $2); print $2; exit}' rust/Cargo.toml)"

cleanup() {
  if [ "$CLEANUP_DONE" -ne 0 ]; then
    return
  fi

  CLEANUP_DONE=1
  if brew list "$TAP_NAME/$FORMULA_NAME" >/dev/null 2>&1; then
    brew uninstall --ignore-dependencies "$TAP_NAME/$FORMULA_NAME" || true
  fi
  rm -rf "$CLEANUP_TAP_PATH"
  rm -f "$ARCHIVE_PATH"
}
trap cleanup EXIT INT TERM

cd "$ROOT_DIR"

tar -czf "$ARCHIVE_PATH" \
  --exclude='.git' \
  --exclude='/.idea' \
  --exclude='/.vscode' \
  -C "$ROOT_DIR" .
ARCHIVE_SHA256="$(shasum -a 256 "$ARCHIVE_PATH" | awk '{print $1}')"

TAP_PATH="$CLEANUP_TAP_PATH"
mkdir -p "$TAP_PATH/Formula"
cat > "$TAP_PATH/Formula/$FORMULA_NAME.rb" <<EOF
class Apw < Formula
  desc "Apple Password CLI and local app broker (macOS-first)"
  homepage "https://github.com/OMT-Global/apw-cli"
  version "$VERSION"
  url "file://$ARCHIVE_PATH"
  sha256 "$ARCHIVE_SHA256"
  license "GPL-3.0-only"

  depends_on "rust" => :build

  def install
    system "bash", "./scripts/build-native-app.sh"
    system "cargo", "build", "--manifest-path", "rust/Cargo.toml", "--release"
    bin.install "rust/target/release/apw"
    libexec.install "native-app/dist/APW.app"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/apw --version")
    assert_match "\"app\"", shell_output("#{bin}/apw status --json 2>&1")
  end
end
EOF

brew install --build-from-source "$TAP_NAME/$FORMULA_NAME"
apw --version
apw status --json
echo "Brew source smoke install succeeded."
