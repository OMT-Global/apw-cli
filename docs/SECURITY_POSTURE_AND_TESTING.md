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
- supported fallback providers are `1password`, `bitwarden`, `keepassxc`, and
  `pass`; all four reuse the same validated absolute-path execution model
  (owner-only, no `~`, no relative paths, `0755`-or-tighter mode, bounded
  output, process-group timeout)
- the `keepassxc` provider additionally requires `fallbackProviderDatabase`
  (an absolute `.kdbx` path) and reads the master password from the
  `APW_KEEPASSXC_PASSWORD` environment variable, feeding it to
  `keepassxc-cli` over stdin; the password is never written to disk or cached
- the `pass` provider relies on `gpg-agent` for the unlock, so APW never
  handles the master key

### Runtime broker hardening

- the v2 app broker uses a same-user local UNIX socket under `~/.apw/native-app/`
- `status --json` exposes native app broker readiness; legacy
  daemon/host/bridge diagnostics are archived under `legacy/`
- requests and responses use typed JSON envelopes with bounded payload sizes
- Shortcuts and AppleScript automation entrypoints build the same broker
  request envelope as CLI `apw login` / `apw fill`; they do not read credential
  material directly or bypass the broker's user-mediated response path
- bootstrap credentials are read from a local runtime file for the supported
  demo domain only; the app does not create that plaintext file on default
  launch

### Timeouts and failure modes

- native app UNIX-socket requests use a `3s` read/write timeout
- a hung broker socket returns a non-zero `CommunicationTimeout` error instead
  of blocking the CLI indefinitely
- AuthenticationServices returns stable APW broker codes for cancellation,
  generic failure, invalid response, not-handled, and unknown errors;
  SDK-specific cases are collapsed into `unknown` until APW explicitly
  promotes them into the wire contract
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
cargo test --manifest-path rust/Cargo.toml --test native_app_e2e
./scripts/ci/validate-appcast-contract.sh
./scripts/build-universal-release.sh
./scripts/verify-universal-binaries.sh
```

In-app updates must follow the signed Sparkle appcast contract in
[IN_APP_UPDATES.md](IN_APP_UPDATES.md). Release automation must not publish an
appcast until the APW.app archive passes code-signing, Gatekeeper, and
notarization staple validation.

Before claiming Phase 3 complete for a public release, run the real-hardware
notarized broker validation in
[PHASE3_HARDWARE_VALIDATION.md](PHASE3_HARDWARE_VALIDATION.md). This check is
manual because CI cannot prove that the native iCloud Keychain picker appears
for a notarized app with associated-domain entitlements.

## Security-focused regression coverage

The Rust test suite covers:

- invalid PIN rejection before transport use
- invalid URL rejection before auth dependency
- stable native app JSON status shape
- launch failure precedence over session errors
- malformed or oversized payload rejection
- native app socket timeout handling
- native app diagnostics and `APW_DEMO=1` bootstrap credential file initialization
- end-to-end v2 app install, launch, status, doctor, and login flows
- Shortcuts / AppleScript automation envelope parity for `login` and `fill`
- direct-exec fallback, unsupported-domain handling, denial handling, and malformed broker response mapping
- signed appcast contract requirements for the future APW.app in-app update channel
- a manual notarized-hardware validation contract for the Phase 3
  AuthenticationServices broker flow
- diagnostic-bundle layout, archive permissions, and fail-closed redaction
  when a plausible credential pattern would otherwise reach the bundle
- external fallback provider path hardening, including relative paths, `~`, world-writable
  executables, and symlink targets
- external fallback lookups for `1password`, `bitwarden`, `keepassxc`, and
  `pass`, including KeePassXC master-password stdin feeding and the typed
  errors for missing config, missing database, and missing entries
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
- AuthenticationServices broker routing for `login` and `fill` success and
  failure cases via injected test brokers, including the guarantee that
  `credentials.json` is not consulted unless `APW_DEMO=1`

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

The historical Deno, browser bridge, native host, and legacy Rust helper
implementations are archived and not part of the supported security surface.
Use them only for compatibility audit work.

Archive rules: [ARCHIVE_POLICY.md](ARCHIVE_POLICY.md)
