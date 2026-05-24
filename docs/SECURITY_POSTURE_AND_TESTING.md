# Security posture and testing

This document describes the current security posture of the Rust CLI and local
APW app broker and the release checks expected before publishing binaries or a
Homebrew formula.

Release reference version: `v2.0.0`

## Security posture

### Config and secret handling

- legacy runtime config lives in `~/.apw/config.json`
- the v2 app broker uses `~/.apw/native-app/`
- `~/.apw` is created with mode `0700`
- config and status files are written with mode `0600`; plaintext bootstrap
  credential files are never persisted by default
- demo bootstrap credentials are written only when `APW_DEMO=1` is explicitly
  set for bootstrap tests
- legacy session secret material is kept in the user keychain when the `v1.x`
  compatibility path is used
- external CLI fallback requires both configuration (`fallbackProvider` +
  `fallbackProviderPath`) and an explicit `apw login --external-fallback <url>`
  invocation, requires an absolute executable path, marks JSON output as
  `transport: "external_cli"` / `securityMode: "reduced_external_cli"`, and does
  not cache returned credentials

### Runtime broker hardening

- the v2 app broker uses a same-user local UNIX socket under `~/.apw/native-app/`
- `status --json` exposes app/broker readiness while retaining legacy daemon diagnostics
- requests and responses use typed JSON envelopes with bounded payload sizes
- bootstrap credentials are read from a local runtime file for the supported
  demo domain only; the app does not create that plaintext file on default
  launch

### Timeouts and failure modes

- native app UNIX-socket requests use a `3s` read/write timeout
- a hung broker socket returns a non-zero `CommunicationTimeout` error instead
  of blocking the CLI indefinitely
- direct executable fallback runs the APW app bundle under a `5s` wall-clock
  timeout, reads at most the configured maximum response size from each of
  stdout and stderr via bounded pipe reads, and terminates the child (process
  group) on timeout so a hung or runaway fallback cannot block or exhaust CLI
  memory
- `apw doctor` CI diagnostics run external tool probes (`xcodebuild`, `cargo`,
  `detect-secrets`, `security find-identity`) under the same bounded-read
  helper with a `3s` per-probe timeout, so a misconfigured shim does not hang
  the doctor command
- timed-out requests do not cache or persist partially returned credentials

### Diagnostic-bundle export

`apw doctor --bundle <path>` writes a tar.gz that operators can attach to
support requests. The bundle is deterministic and fails closed rather than
shipping incompletely-redacted material.

Layout (see `rust/src/bundle.rs` for the source of truth):

```
apw-doctor-bundle/
  manifest.json                # bundleVersion, files, redaction guarantees
  doctor.json                  # full `apw doctor --json` payload
  environment.json             # `apw doctor --ci` environment checks
  os.json                      # uname, arch/os, sw_vers on macOS
  native-app/file-listing.json # path/size/mode for files under ~/.apw/native-app/
```

Redaction guarantees:

- environment variables are never read or copied into the bundle
- file contents under `~/.apw/native-app/` are never read — only the metadata
  listing (relative path, byte size, octal mode, file type) is included
- `credentials.json`, `config.json`, and `broker.log` are explicitly excluded
- every string in the bundle JSON is scanned for token-like patterns
  (long high-entropy alphanumeric runs, common vendor key prefixes, and the
  in-tree demo password sentinel); a match aborts the bundle with an
  `InvalidConfig` (102) error and does not write the archive
- the archive file is written mode `0600`

If an operator needs to share broker logs or config they attach those
separately, after redacting by hand.

## Required release gates

Run these before publishing:

```bash
cargo fmt --manifest-path rust/Cargo.toml -- --check
cargo clippy --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path rust/Cargo.toml --all-targets
cargo test --manifest-path rust/Cargo.toml --test legacy_parity
cargo test --manifest-path rust/Cargo.toml --test native_app_e2e
./scripts/build-universal-release.sh
./scripts/verify-universal-binaries.sh
```

## Security-focused regression coverage

The Rust test suite covers:

- invalid PIN rejection before transport use
- invalid URL rejection before auth dependency
- stable JSON status shape
- launch failure precedence over session errors
- malformed or oversized payload rejection
- native app socket timeout handling
- native app diagnostics and `APW_DEMO=1` bootstrap credential file initialization
- end-to-end v2 app install, launch, status, doctor, and login flows
- direct-exec fallback, unsupported-domain handling, denial handling, and malformed broker response mapping
- diagnostic-bundle layout, archive permissions, and fail-closed redaction
  when a plausible credential pattern would otherwise reach the bundle
- external fallback provider path hardening, including relative paths, `~`, world-writable
  executables, and symlink targets
- diagnostic-bundle redaction and fail-closed aborts when staged diagnostics look
  credential-like
- threat-model drift checks so retired UDP/browser-helper/private-helper
  surfaces do not re-enter the supported v2 broker boundary without an explicit
  documentation update

The threat matrix and residual-risk owners are tracked in
[THREAT_MODEL.md](THREAT_MODEL.md). When a new credential surface is added,
update that matrix and add a focused regression in the Rust or Swift suite that
proves the documented mitigation.

The native app Swift test suite covers:

- localized approval prompt copy for APW-owned UI
- accessibility labels for the credential approval window and buttons
- broker envelope parsing, permission checks, denial handling, and typed AuthenticationServices fallback errors

## Accessibility and localization audit

Rerun this checklist before each minor release and when changing APW-owned UI.
Apple-owned AuthenticationServices picker UI is out of scope for direct labels,
but the APW surfaces around it remain in scope.

- VoiceOver announces the APW credential approval window with a clear label.
- VoiceOver announces Allow and Deny buttons with action-oriented labels.
- Keyboard-only users can reach and activate every APW-owned approval control.
- APW-owned user-visible strings are loaded from `Localizable.strings`.
- At least one non-English `.lproj` resource ships in `APW.app`.
- Reduced motion, high contrast, and increased text size do not hide or truncate APW-owned approval text.

## Archive policy

The Deno implementation is archived and not part of the supported security
surface. Use it only for compatibility audit work.

Archive rules: [ARCHIVE_POLICY.md](ARCHIVE_POLICY.md)
