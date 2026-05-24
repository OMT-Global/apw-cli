# Rust migration and parity

APW now treats the Rust CLI plus local APW app broker as the primary `v2.0.0`
runtime. The historical parity line remains preserved for migration and audit
work.

Release reference version: `v2.0.0`

## Current maintenance policy

- Supported v2 implementation: [`rust/`](../rust/) + `native-app/`
- Archived implementation: [`legacy/deno/`](../legacy/deno/)
- Archived browser/helper implementation:
  [`legacy/browser-bridge/`](../legacy/browser-bridge/),
  [`legacy/native-host/`](../legacy/native-host/), and
  [`legacy/rust-src/`](../legacy/rust-src/)
- Packaging, release, fixes, and hardening land in the Rust CLI and native app
- Legacy daemon/browser-helper code is no longer in the active Rust module tree
  and is preserved only for historical audit work

## Removed commands

The following legacy daemon CLI subcommands were removed from the active Rust
CLI for the `v2.1.0` cliff:

| Subcommand   | Replacement                  |
| ------------ | ---------------------------- |
| `apw start`  | `apw app launch`             |
| `apw pw`     | `apw login` / `apw fill`     |
| `apw otp`    | (no v2 replacement planned)  |
| `apw auth`   | (no v2 replacement; v2 broker is app-mediated) |

Operators with scripts pinned to these commands must migrate before upgrading to
v2.1.0.

Archive rules: [ARCHIVE_POLICY.md](ARCHIVE_POLICY.md)

## Parity target

The compatibility target for `v1.x` remains the public command contract from
the historical Deno CLI, not the old implementation details.

The `v2.0.0` line intentionally changes that contract:

- app-assisted credential requests replace the primary auth flow
- vault-wide password listing is no longer a primary goal
- OTP parity is not guaranteed

The command migration matrix is tracked in
[NATIVE_MIGRATION.md](NATIVE_MIGRATION.md).

## Automated coverage

Primary Rust gates:

```bash
cargo fmt --manifest-path rust/Cargo.toml -- --check
cargo clippy --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path rust/Cargo.toml --all-targets
```

## Release expectations

Before tagging a public release:

1. Keep versioned surfaces in sync
2. Run the Rust gates
3. Run the security regression matrix
4. Build the app bundle with `./scripts/build-native-app.sh`

Related docs:

- [INSTALLATION.md](INSTALLATION.md)
- [SECURITY_POSTURE_AND_TESTING.md](SECURITY_POSTURE_AND_TESTING.md)
- [NATIVE_MIGRATION.md](NATIVE_MIGRATION.md)
