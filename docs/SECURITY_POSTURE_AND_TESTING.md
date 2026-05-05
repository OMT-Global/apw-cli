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
- direct executable fallback responses are still bounded by the same maximum
  response size before JSON decoding
- timed-out requests do not cache or persist partially returned credentials

## Required release gates

Run these before publishing:

```bash
cargo fmt --manifest-path rust/Cargo.toml -- --check
cargo clippy --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path rust/Cargo.toml --all-targets
cargo test --manifest-path rust/Cargo.toml --test legacy_parity
cargo test --manifest-path rust/Cargo.toml --test native_app_e2e
cargo build --manifest-path rust/Cargo.toml --release
./scripts/build-native-app.sh
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

## Archive policy

The Deno implementation is archived and not part of the supported security
surface. Use it only for compatibility audit work.

Archive rules: [ARCHIVE_POLICY.md](ARCHIVE_POLICY.md)
