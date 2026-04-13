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
- config, status, and bootstrap credential files are written with mode `0600`
- legacy session secret material is kept in the user keychain when the `v1.x`
  compatibility path is used
- external CLI fallback is opt-in via `fallbackProvider` +
  `fallbackProviderPath`, requires an absolute executable path, and does not
  cache returned credentials

### Runtime broker hardening

- the v2 app broker uses a same-user local UNIX socket under `~/.apw/native-app/`
- `status --json` exposes app/broker readiness while retaining legacy daemon diagnostics
- requests and responses use typed JSON envelopes with bounded payload sizes
- bootstrap credentials are stored in a local runtime file for the supported demo domain only

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
- native app diagnostics and bootstrap credential file initialization
- end-to-end v2 app install, launch, status, doctor, and login flows
- direct-exec fallback, unsupported-domain handling, denial handling, and malformed broker response mapping

## Archive policy

The Deno implementation is archived and not part of the supported security
surface. Use it only for compatibility audit work.

Archive rules: [docs/ARCHIVE_POLICY.md](/Users/johnteneyckjr./src/apw/docs/ARCHIVE_POLICY.md)
