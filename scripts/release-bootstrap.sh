#!/usr/bin/env bash
set -euo pipefail

print_help() {
  cat <<'EOF'
Usage: ./scripts/release-bootstrap.sh [OPTIONS]

Run end-to-end release checks, build a release binary, create a tag, and optionally
run Homebrew source-smoke install, and optionally publish GitHub release assets.

Options:
  --tag VERSION        Release tag (default: v<version from rust/Cargo.toml>)
  --skip-tests         Skip cargo test runs
  --skip-brew-smoke    Skip Homebrew smoke step
  --host-smoke         Run the local Chrome/browser bridge host smoke after build
  --pw-domain DOMAIN   Required with --host-smoke; domain for `apw pw list`
  --otp-domain DOMAIN  Optional with --host-smoke; domain for `apw otp list`
  --push               Push the release tag to `origin`
  --publish            Create/update GitHub release and upload release tarball
  --allow-dirty        Allow non-clean working tree for release
  --help               Show this help message

Examples:
  ./scripts/release-bootstrap.sh
  ./scripts/release-bootstrap.sh --tag v1.2.1 --push --publish
EOF
}

ROOT_DIR="$(cd -- "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_MANIFEST="$ROOT_DIR/rust/Cargo.toml"
VERIFY_SCRIPT="$ROOT_DIR/.github/scripts/verify-version-sync.sh"
BIN_PATH="$ROOT_DIR/rust/target/release/apw"

TAG=""
PUSH_TAG=0
SKIP_TESTS=0
BREW_SMOKE=1
ALLOW_DIRTY=0
PUBLISH_RELEASE=0
HOST_SMOKE=0
HOST_SMOKE_PW_DOMAIN=""
HOST_SMOKE_OTP_DOMAIN=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tag)
      if [[ -z "${2:-}" ]]; then
        echo "--tag requires a value."
        print_help
        exit 1
      fi
      TAG="$2"
      shift 2
      ;;
    --skip-tests)
      SKIP_TESTS=1
      shift
      ;;
    --skip-brew-smoke)
      BREW_SMOKE=0
      shift
      ;;
    --host-smoke)
      HOST_SMOKE=1
      shift
      ;;
    --pw-domain)
      HOST_SMOKE_PW_DOMAIN="${2:-}"
      shift 2
      ;;
    --otp-domain)
      HOST_SMOKE_OTP_DOMAIN="${2:-}"
      shift 2
      ;;
    --push)
      PUSH_TAG=1
      shift
      ;;
    --publish)
      PUSH_TAG=1
      PUBLISH_RELEASE=1
      shift
      ;;
    --allow-dirty)
      ALLOW_DIRTY=1
      shift
      ;;
    --help)
      print_help
      exit 0
      ;;
    *)
      echo "Unknown argument: $1"
      print_help
      exit 1
      ;;
  esac
done

if [[ ! -f "$CARGO_MANIFEST" ]]; then
  echo "Expected manifest not found: $CARGO_MANIFEST"
  exit 1
fi

if [[ ! -f "$VERIFY_SCRIPT" ]]; then
  echo "Expected version-sync helper not found: $VERIFY_SCRIPT"
  exit 1
fi

if [[ "$HOST_SMOKE" -eq 1 ]] && [[ -z "$HOST_SMOKE_PW_DOMAIN" ]]; then
  echo "--pw-domain is required when --host-smoke is enabled."
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install Rust: https://rustup.rs/"
  exit 1
fi

read_rust_version() {
  local version
  version="$(awk -F ' = ' '/^version = / {gsub(/"/, "", $2); print $2; exit}' "$CARGO_MANIFEST")"
  if [[ -z "${version}" ]]; then
    echo "Unable to read version from $CARGO_MANIFEST"
    exit 1
  fi
  echo "$version"
}

read_highest_existing_version() {
  git tag --list 'v[0-9]*.[0-9]*.[0-9]*' | sed 's/^v//' | awk -F '.' '
    NF != 3 { next }
    {
      major = $1 + 0
      minor = $2 + 0
      patch = $3 + 0
      if (!found || major > best_major || (major == best_major && minor > best_minor) || (major == best_major && minor == best_minor && patch > best_patch)) {
        best_major = major
        best_minor = minor
        best_patch = patch
        found = 1
      }
    }
    END {
      if (found) {
        printf "%d.%d.%d\n", best_major, best_minor, best_patch
      }
    }
  '
}

version_advances_history() {
  local candidate="$1"
  local baseline="$2"

  awk -F '.' -v candidate="$candidate" -v baseline="$baseline" '
    BEGIN {
      split(candidate, c, ".")
      split(baseline, b, ".")
      if ((c[1] + 0) > (b[1] + 0)) exit 0
      if ((c[1] + 0) < (b[1] + 0)) exit 1
      if ((c[2] + 0) > (b[2] + 0)) exit 0
      if ((c[2] + 0) < (b[2] + 0)) exit 1
      if ((c[3] + 0) > (b[3] + 0)) exit 0
      exit 1
    }
  '
}

extract_formula_url() {
  awk -F '\"' '/^[[:space:]]*url[[:space:]]+"/ { print $2; exit }' "$ROOT_DIR/packaging/homebrew/apw.rb"
}

validate_formula_url_matches_tag() {
  local formula_url="$1"
  local expected_tag="$2"
  local required_suffix="refs/tags/${expected_tag}.tar.gz"
  if [[ -z "$formula_url" ]] || [[ "$formula_url" != *"$required_suffix" ]]; then
    echo "Homebrew formula URL does not reference the release tag."
    echo "Formula URL: ${formula_url:-<missing>}"
    echo "Expected suffix: ...${required_suffix}"
    exit 1
  fi
}

publish_release_asset() {
  local artifact="$1"
  local tag="$2"
  local artifact_size

  if ! command -v gh >/dev/null 2>&1; then
    echo "gh CLI not found. Install gh to publish release assets."
    return 1
  fi

  if ! gh auth status -t >/dev/null 2>&1; then
    echo "gh CLI is not authenticated. Run: gh auth login"
    return 1
  fi

  if gh release view "$tag" >/dev/null 2>&1; then
    echo "Updating existing release: $tag"
    gh release upload "$tag" "$artifact" --clobber
  else
    echo "Creating release: $tag"
    gh release create "$tag" --title "APW ${tag}" --notes "Release ${tag}" "$artifact"
  fi

  gh release view "$tag" --json name,tagName,isDraft,isPrerelease,assets \
    --jq '.name'

  artifact_size="$(wc -c < "$artifact")"
  echo "Published ${artifact} (${artifact_size} bytes) for ${tag}"
}

build_release_artifact() {
  local version="$1"
  local artifact_path="$ROOT_DIR/dist/apw-macos-v${version}.tar.gz"
  mkdir -p "$ROOT_DIR/dist"

  cp "$BIN_PATH" "$ROOT_DIR/dist/apw"
  tar -czf "$artifact_path" -C "$ROOT_DIR/dist" apw
  rm "$ROOT_DIR/dist/apw"

  echo "$artifact_path"
}

normalize_tag() {
  local value="$1"
  value="${value#v}"
  if [[ ! "$value" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Invalid tag/version format: $1"
    exit 1
  fi
  echo "$value"
}

if [[ -z "$TAG" ]]; then
  TAG="v$(read_rust_version)"
else
  TAG="v$(normalize_tag "$TAG")"
fi

cd "$ROOT_DIR"

if [[ "$ALLOW_DIRTY" -ne 1 ]] && [[ -n "$(git status --porcelain)" ]]; then
  echo "Working tree is not clean. Use --allow-dirty if you want to continue anyway."
  exit 1
fi

if git rev-parse "$TAG" >/dev/null 2>&1; then
  echo "Tag $TAG already exists."
  exit 1
fi

highest_existing_version="$(read_highest_existing_version)"
if [[ -n "$highest_existing_version" ]] && ! version_advances_history "${TAG#v}" "$highest_existing_version"; then
  echo "Release version ${TAG#v} does not advance repository tag history (latest existing: v${highest_existing_version})."
  echo "Bump rust/Cargo.toml and synced release surfaces before running release-bootstrap."
  exit 1
fi

printf '\n[1/8] Verifying version sync across release surfaces...\n'
bash "$VERIFY_SCRIPT" "$CARGO_MANIFEST" \
  "$ROOT_DIR/rust/src/cli.rs" \
  "$ROOT_DIR/rust/src/types.rs" \
  "$ROOT_DIR/packaging/homebrew/apw.rb" \
  "$ROOT_DIR/README.md" \
  "$ROOT_DIR/docs/INSTALLATION.md" \
  "$ROOT_DIR/docs/MIGRATION_AND_PARITY.md"

printf '\n[2/8] Running release gates...\n'
cargo fmt --manifest-path "$CARGO_MANIFEST" -- --check
cargo clippy --manifest-path "$CARGO_MANIFEST" --all-targets -- -D warnings
if [[ "$SKIP_TESTS" -eq 0 ]]; then
  cargo test --manifest-path "$CARGO_MANIFEST"
  cargo test --manifest-path "$CARGO_MANIFEST" --test legacy_parity
  cargo test --manifest-path "$CARGO_MANIFEST" --test security_regressions
fi

printf '\n[3/8] Building release binary...\n'
cargo build --manifest-path "$CARGO_MANIFEST" --release

printf '\n[4/8] Health check release binary...\n'
"$BIN_PATH" --version
"$BIN_PATH" status --json

if [[ "$HOST_SMOKE" -eq 1 ]]; then
  printf '\n[5/8] Running browser host smoke...\n'
  if [[ -n "$HOST_SMOKE_OTP_DOMAIN" ]]; then
    "$ROOT_DIR/scripts/browser-host-smoke.sh" \
      --bin "$BIN_PATH" \
      --pw-domain "$HOST_SMOKE_PW_DOMAIN" \
      --otp-domain "$HOST_SMOKE_OTP_DOMAIN"
  else
    "$ROOT_DIR/scripts/browser-host-smoke.sh" \
      --bin "$BIN_PATH" \
      --pw-domain "$HOST_SMOKE_PW_DOMAIN"
  fi
fi

printf '\n[6/8] Creating tag %s...\n' "$TAG"
git tag -a "$TAG" -m "chore: release ${TAG}"
VERSION="${TAG#v}"
formula_url="$(extract_formula_url)"
validate_formula_url_matches_tag "$formula_url" "$TAG"

if [[ "$PUSH_TAG" -eq 1 ]]; then
  printf '\n[7/8] Pushing tag %s...\n' "$TAG"
  git push origin "$TAG"
  if [[ "$BREW_SMOKE" -eq 1 ]]; then
    if ! command -v brew >/dev/null 2>&1; then
      echo "Homebrew required for smoke step but not found on PATH."
      exit 1
    fi
    ./packaging/homebrew/install-from-source.sh
    echo "Release tag pushed and Homebrew source smoke complete."
  else
    echo "Release tag pushed. Brew smoke disabled."
  fi
else
  echo "Release tag created locally: $TAG"
  if [[ "$BREW_SMOKE" -eq 1 ]]; then
    if ! command -v brew >/dev/null 2>&1; then
      echo "Homebrew required for smoke step but not found on PATH."
      exit 1
    fi
    ./packaging/homebrew/install-from-source.sh
    echo "Release tag created and Homebrew source smoke complete."
  else
    echo "Release tag created locally. Brew smoke disabled."
  fi
fi

if [[ "$PUBLISH_RELEASE" -eq 1 ]]; then
  printf '\n[8/8] Building release tarball and publishing assets for %s...\n' "$TAG"
  ARTIFACT_PATH="$(build_release_artifact "$VERSION")"
  echo "Created release artifact: $ARTIFACT_PATH"
  publish_release_asset "$ARTIFACT_PATH" "$TAG"
fi

printf '\nRelease artifact and publish checks complete.\n'
echo "Release bootstrap completed successfully."
