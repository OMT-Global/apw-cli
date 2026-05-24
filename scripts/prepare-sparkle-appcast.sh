#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: ./scripts/prepare-sparkle-appcast.sh --archive PATH --release-notes PATH --updates-dir DIR --generate-appcast PATH [--feed-url URL]

Prepare a Sparkle updates directory and run Sparkle's generate_appcast tool.
The tool is expected to sign archives, release notes, and the appcast using
Sparkle's configured EdDSA key material. Do not pass private keys on this
script's command line.

Options:
  --archive PATH            Signed/notarized APW.app update archive.
  --release-notes PATH      Markdown release notes for this archive.
  --updates-dir DIR         Directory holding Sparkle update archives.
  --generate-appcast PATH   Path to Sparkle's generate_appcast executable.
  --feed-url URL            Feed URL; default is APW's production appcast URL.
  -h, --help                Show this help.
USAGE
}

FEED_URL="https://github.com/OMT-Global/apw-cli/releases/latest/download/appcast.xml"
ARCHIVE_PATH=""
RELEASE_NOTES_PATH=""
UPDATES_DIR=""
GENERATE_APPCAST=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --archive)
      ARCHIVE_PATH="${2:-}"
      shift 2
      ;;
    --release-notes)
      RELEASE_NOTES_PATH="${2:-}"
      shift 2
      ;;
    --updates-dir)
      UPDATES_DIR="${2:-}"
      shift 2
      ;;
    --generate-appcast)
      GENERATE_APPCAST="${2:-}"
      shift 2
      ;;
    --feed-url)
      FEED_URL="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

fail() {
  echo "prepare-sparkle-appcast: $*" >&2
  exit 1
}

[ -n "$ARCHIVE_PATH" ] || fail "--archive is required"
[ -n "$RELEASE_NOTES_PATH" ] || fail "--release-notes is required"
[ -n "$UPDATES_DIR" ] || fail "--updates-dir is required"
[ -n "$GENERATE_APPCAST" ] || fail "--generate-appcast is required"

case "$FEED_URL" in
  https://*) ;;
  *) fail "--feed-url must be an https URL" ;;
esac

[ -f "$ARCHIVE_PATH" ] || fail "archive not found: $ARCHIVE_PATH"
[ -f "$RELEASE_NOTES_PATH" ] || fail "release notes not found: $RELEASE_NOTES_PATH"
[ -x "$GENERATE_APPCAST" ] || fail "generate_appcast is not executable: $GENERATE_APPCAST"

archive_name="$(basename "$ARCHIVE_PATH")"
case "$archive_name" in
  *.zip|*.dmg|*.tar|*.tar.gz|*.tar.xz|*.aar) ;;
  *) fail "archive must be a Sparkle-supported update archive: $archive_name" ;;
esac

feed_file="$(basename "$FEED_URL")"
[ -n "$feed_file" ] || fail "unable to derive appcast file name from --feed-url"

mkdir -p "$UPDATES_DIR"
cp "$ARCHIVE_PATH" "$UPDATES_DIR/$archive_name"
cp "$RELEASE_NOTES_PATH" "$UPDATES_DIR/$archive_name.md"

"$GENERATE_APPCAST" "$UPDATES_DIR"

appcast_path="$UPDATES_DIR/$feed_file"
[ -f "$appcast_path" ] || fail "generate_appcast did not create $appcast_path"

if ! grep -q 'sparkle:edSignature=' "$appcast_path"; then
  fail "$appcast_path does not contain Sparkle EdDSA signatures"
fi

if ! grep -q "$archive_name" "$appcast_path"; then
  fail "$appcast_path does not reference $archive_name"
fi

echo "Prepared signed Sparkle appcast: $appcast_path"
