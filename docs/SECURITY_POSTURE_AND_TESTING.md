# Security posture and testing

This document describes the current security posture of the Rust implementation
and the release checks expected before publishing binaries or a Homebrew formula.

Release reference version: `v1.2.0`

## Security posture

### Config and secret handling

- config lives in `~/.apw/config.json`
- `~/.apw` is created with mode `0700`
- `config.json` is written atomically with mode `0600`
- malformed, stale, or schema-invalid config is rejected and cleared
- on supported macOS paths, session secret material is kept in the user keychain

### Session lifecycle

- persisted session metadata includes `createdAt`
- expired sessions force re-authentication
- failed launch state is preserved so follow-up commands report the real runtime
  failure before a misleading session error

### Transport and parser hardening

- bounded message sizes
- typed status/error envelopes
- timeout handling and retry behavior in the client
- helper frame validation before JSON decode
- malformed helper payloads and invalid response schemas map to explicit errors

### Runtime bridge hardening

- native-host attach/disconnect state is persisted
- `status --json` exposes `host`, deprecated `bridge`, and `daemon.preflight` diagnostics
- direct helper launch failures remain visible through `lastLaunch*` metadata

## Required release gates

Run these before publishing:

```bash
cargo fmt --manifest-path rust/Cargo.toml -- --check
cargo clippy --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path rust/Cargo.toml --all-targets
cargo test --manifest-path rust/Cargo.toml --test legacy_parity
cargo build --manifest-path rust/Cargo.toml --release
./scripts/build-native-host.sh
```

## Security-focused regression coverage

The Rust test suite already covers:

- invalid PIN rejection before transport use
- invalid URL rejection before auth dependency
- stable JSON status shape
- launch failure precedence over session errors
- helper/parser malformed payload rejection
- oversized config and oversized helper payload handling
- native-host attach/disconnect and error propagation
- SRP message validation and proof verification checks

The dedicated integration target is:

```bash
cargo test --manifest-path rust/Cargo.toml --test security_regressions
```

## Manual host validation

Some risk remains host-specific and should be checked on the real target macOS
machine:

```bash
./scripts/browser-host-smoke.sh --pw-domain example.com
```

This writes a timestamped evidence bundle under:

```text
dist/host-smoke/<timestamp>/
```

Recommended contents to review:

- daemon startup logs
- `status --json` snapshots
- auth output
- password and OTP query results
- helper crash or launch diagnostics

## Archive policy

The Deno implementation is archived and not part of the supported security
surface. Use it only for compatibility audit work.

Archive rules: [docs/ARCHIVE_POLICY.md](/Users/johnteneyckjr./src/apw/docs/ARCHIVE_POLICY.md)
