# Rust migration and parity

APW now treats the Rust implementation as the only supported runtime and release
path.

Release reference version: `v1.2.0`

## Current maintenance policy

- Supported implementation: [`rust/`](/Users/johnteneyckjr./src/apw/rust)
- Archived implementation: [`legacy/deno/`](/Users/johnteneyckjr./src/apw/legacy/deno)
- Packaging, release, fixes, and hardening land only in Rust
- The Deno archive exists for historical inspection and compatibility review only

Archive rules: [docs/ARCHIVE_POLICY.md](/Users/johnteneyckjr./src/apw/docs/ARCHIVE_POLICY.md)

## Parity target

The compatibility target is the public command contract from the historical Deno
CLI, not the old implementation details.

Rust currently covers the same operational surface for:

- `auth request`
- `auth response`
- `auth logout`
- `status`
- `start`
- `pw list`
- `pw get`
- `otp list`
- `otp get`

The default human-facing command behavior remains the contract. Structured output
and diagnostics are additive behind explicit JSON/status surfaces.

## Known compatibility framing

- On modern macOS, `auto` resolves to native companion-host mode because direct
  helper launch is not always permitted from the CLI parent process.
- This is treated as an operational compatibility bridge, not a removal of CLI
  behavior.
- Direct and launchd-compatible native helper paths remain available as explicit
  diagnostic modes.

## Automated parity coverage

Primary Rust gates:

```bash
cargo fmt --manifest-path rust/Cargo.toml -- --check
cargo clippy --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path rust/Cargo.toml --all-targets
```

Legacy parity harness:

```bash
cargo test --manifest-path rust/Cargo.toml --test legacy_parity
```

The parity suite exercises the Rust CLI against preserved legacy fixtures for:

- auth request shape
- auth response failure mapping
- status JSON shape
- password and OTP query behavior
- command matrix expectations

## Optional archive audit

If you still have Deno installed and want a direct historical spot-check, the
archived implementation can be run manually:

```bash
cd legacy/deno
deno test --allow-env --allow-read --allow-write --allow-net src/*.test.ts
```

This is an audit path only. It is not part of the supported runtime or release
flow.

## Release expectations

Before tagging a public release:

1. Keep versioned surfaces in sync
2. Run the Rust gates
3. Run the parity harness
4. Run the security regression matrix
5. Publish only from the Rust path

Related docs:

- [docs/INSTALLATION.md](/Users/johnteneyckjr./src/apw/docs/INSTALLATION.md)
- [docs/SECURITY_POSTURE_AND_TESTING.md](/Users/johnteneyckjr./src/apw/docs/SECURITY_POSTURE_AND_TESTING.md)
