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
- the plaintext demo credentials file (`~/.apw/native-app/credentials.json`) is
  **never** materialized by default. Set `APW_DEMO=1` before `apw app install`
  / `apw app launch` to opt into the bundled `example.com` bootstrap
  credential. Without `APW_DEMO`, `apw login` returns
  `no_credential_source` for the demo domain. (issue #14)
- external CLI fallback is opt-in via `fallbackProvider` +
  `fallbackProviderPath`, requires an absolute executable path that:
  - is not `~`-prefixed
  - resolves via `fs::canonicalize` (symlinks followed)
  - is a regular file owned by the current effective uid
  - has the execute bit set and is **not** world-writable
  Validation failures surface as typed `InvalidConfig` errors. (issue #1)
- external fallback exec is bounded:
  - per-invocation wall-clock timeout `APW_FALLBACK_TIMEOUT_MS`
    (default 15000)
  - per-process invocation cap `APW_FALLBACK_INVOCATION_LIMIT` (default 5)
  - Timeouts kill the child via `SIGKILL` and return
    `CommunicationTimeout`; rate-limit breaches return a typed
    `GenericError` without crashing. (issue #3)

### Runtime broker hardening

- the v2 app broker uses a same-user local UNIX socket under `~/.apw/native-app/`
- `status --json` exposes app/broker readiness while retaining legacy daemon diagnostics
- requests and responses use typed JSON envelopes with bounded payload sizes
- bootstrap credentials are stored in a local runtime file for the supported demo domain only

### Timeouts and failure modes

A single broker IPC exchange between the Rust CLI and the Swift broker is
bounded on both halves of the connection by a shared timeout. (issue #2)

| Side  | Constant                       | Value          | Behavior on timeout                                |
| ----- | ------------------------------ | -------------- | -------------------------------------------------- |
| Rust  | `BROKER_REQUEST_TIMEOUT_MS`    | 3000 ms        | `send_request` returns `Status::CommunicationTimeout`; CLI exits non-zero, no credential leak |
| Swift | `brokerRequestTimeoutMs`       | 3000 ms        | per-connection `SO_RCVTIMEO`/`SO_SNDTIMEO`; broker drops the client and continues serving         |

External fallback exec is bounded separately by
`APW_FALLBACK_TIMEOUT_MS` (default 15000 ms; see issue #3).

A regression test in `rust/src/native_app.rs`
(`broker_request_times_out_when_peer_never_replies`) parks a Unix-socket
acceptor that never replies and asserts the CLI aborts within
`BROKER_REQUEST_TIMEOUT_MS + 1s`.

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

Archive rules: [ARCHIVE_POLICY.md](ARCHIVE_POLICY.md)
