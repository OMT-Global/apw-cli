# APW Native

Rust-first, macOS-first CLI and daemon for reading Apple Passwords and one-time
codes from the command line.

Release reference version: `v1.2.0`

`apw` is the installed executable name.

This project is not affiliated with Apple. It interoperates with Apple-provided
password infrastructure on supported macOS versions.

## Project status

- Rust in [`rust/`](/Users/johnteneyckjr./src/apw/rust) is the only supported implementation.
- The historical Deno code is archived under [`legacy/deno/`](/Users/johnteneyckjr./src/apw/legacy/deno) for parity audits and rollback reference only.
- The project is macOS-first. Non-macOS execution fails fast with an explicit error.

Archive policy: [docs/ARCHIVE_POLICY.md](/Users/johnteneyckjr./src/apw/docs/ARCHIVE_POLICY.md)

## What APW does

- Starts a local daemon with `apw start`
- Authenticates an Apple Passwords session with `apw auth`
- Supports explicit request/response auth flows with `apw auth request` and `apw auth response`
- Lists and retrieves passwords with `apw pw`
- Lists and retrieves one-time codes with `apw otp`
- Reports daemon, host, bridge-alias, and session health with `apw status` and `apw status --json`
- Clears persisted session material with `apw auth logout`

## Support model

- Supported target: macOS
- Preferred runtime on macOS 26.x: native companion host mode
- Direct helper launch remains available as a diagnostic mode with `--runtime-mode direct` or `--runtime-mode launchd`
- Unsupported target: non-macOS platforms

On macOS 26.x, `auto` resolves to the native companion host instead of trying
to launch Apple’s helper directly from the CLI. That is an OS/runtime
compatibility choice, not a feature reduction.

## Install

Detailed instructions: [docs/INSTALLATION.md](/Users/johnteneyckjr./src/apw/docs/INSTALLATION.md)

### Build from source

```bash
cargo build --manifest-path rust/Cargo.toml --release
./scripts/build-native-host.sh
```

### Install with Cargo

```bash
cargo install --path rust --locked
./scripts/build-native-host.sh
apw host install
```

### Homebrew

For local formula validation from this checkout:

```bash
./packaging/homebrew/install-from-source.sh
```

For a public tap/release flow, use the formula template in
[`packaging/homebrew/apw.rb`](/Users/johnteneyckjr./src/apw/packaging/homebrew/apw.rb)
and publish a tagged release tarball. After a Homebrew install, run
`apw host install` once per user to install the LaunchAgent-backed native host.

## Quick start

### macOS 26.x native host path

1. Install the native host bundle and LaunchAgent:

```bash
apw host install
```

2. Start the daemon:

```bash
apw start
```

3. Confirm host readiness:

```bash
apw status --json
```

Wait until `host.status` becomes `"attached"` before expecting auth or
password/OTP commands to work through the native-host path.

4. Authenticate:

```bash
apw auth
```

5. Query data:

```bash
apw pw
apw otp
```

### Direct helper diagnostics

If you need to diagnose native host launch behavior directly:

```bash
apw start --runtime-mode direct --dry-run
apw status --json
```

The JSON status output now includes `daemon.preflight`, which reports:

- resolved runtime mode
- candidate launch strategies
- native host readiness
- LaunchAgent and app bundle state
- helper binary path and executability
- machine-readable failure reason when the native host is not viable

## Common commands

```bash
apw --help
apw host install
apw host doctor --json
apw host uninstall
apw start
apw start --bind 127.0.0.1 --port 10000
apw status
apw status --json
apw auth
apw auth request
apw auth response --pin 123456 --salt <salt> --server_key <server_key> --client_key <client_key> --username <username>
apw auth logout
apw pw
apw otp
```

## Security and storage

- APW stores config in `~/.apw/config.json`
- `~/.apw` is created with mode `0700`
- `config.json` is written atomically with mode `0600`
- On supported macOS paths, session secret material is stored in the user keychain and config keeps metadata such as `secretSource`
- Invalid, malformed, or stale config is cleared and requires re-authentication
- Transport, parser, and status errors are returned as typed failures instead of silent partial output

Security and release validation guidance:
[docs/SECURITY_POSTURE_AND_TESTING.md](/Users/johnteneyckjr./src/apw/docs/SECURITY_POSTURE_AND_TESTING.md)

## Repository layout

- [`rust/`](/Users/johnteneyckjr./src/apw/rust): supported CLI, daemon, transport, SRP, and packaging target
- `native-host/`: packaged macOS companion host used on modern macOS native mode
- [`browser-bridge/`](/Users/johnteneyckjr./src/apw/browser-bridge): legacy bridge retained only during native-host transition
- [`legacy/deno/`](/Users/johnteneyckjr./src/apw/legacy/deno): archived compatibility reference
- [`packaging/homebrew/`](/Users/johnteneyckjr./src/apw/packaging/homebrew): Homebrew formula and local install helpers
- [`docs/`](/Users/johnteneyckjr./src/apw/docs): installation, migration, archive, security, and breakout docs

## Parity and migration

Rust is the maintained path. The Deno implementation remains only for audit and
behavior comparison.

Parity and archive details:
[docs/MIGRATION_AND_PARITY.md](/Users/johnteneyckjr./src/apw/docs/MIGRATION_AND_PARITY.md)

## License

This project is licensed under `GPL-3.0-only`. See
[LICENSE](/Users/johnteneyckjr./src/apw/LICENSE).
