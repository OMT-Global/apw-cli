# APW Native

Rust-first, macOS-first CLI and local app broker for mediated credential access
from the command line.

Release reference version: `v2.0.0`

`apw` remains the installed executable name.

This project is not affiliated with Apple. It interoperates with Apple-provided
password infrastructure on supported macOS versions.

> Note: APW CLI was historically forked from `bendews/apw`, but the current
> `v2` line is a substantially rewritten Rust-first implementation.

## Project status

- `main` now tracks the `v2.0.0` native-only redesign line.
- Rust in [`rust/`](rust/) remains the maintained CLI.
- `native-app/` is the new primary runtime surface for the app-assisted broker.
- The historical Deno code is archived under [`legacy/deno/`](legacy/deno/) for parity audits and rollback reference only.
- Legacy daemon/browser-helper code remains in-tree only to preserve the historical `v1.x` parity line during migration.
- The command migration matrix is tracked in [`docs/NATIVE_MIGRATION.md`](docs/NATIVE_MIGRATION.md).

Archive policy: [`docs/ARCHIVE_POLICY.md`](docs/ARCHIVE_POLICY.md)

## What APW does

- Installs the APW macOS app bundle with `apw app install`
- Launches the local APW broker with `apw app launch`
- Reports app, broker, and legacy runtime health with `apw status` and `apw status --json`
- Supports structured stderr logging via `--log-level` or `APW_LOG`
- Reports machine-readable build metadata with `apw version --json`
- Reports bootstrap diagnostics with `apw doctor`
- Returns a fill-intent credential envelope with `apw fill <url>`
- Returns an app-mediated credential for a supported domain with `apw login <url>`

## Support model

- Supported target: macOS
- Current primary runtime on macOS: the APW local app broker
- Historical parity runtime: legacy daemon/browser-helper code retained for migration only
- Legacy direct/native/browser runtime modes remain available only for the `v1.x` compatibility path
- Unsupported target: non-macOS platforms

Detailed migration and redesign notes:

- [`docs/NATIVE_MIGRATION.md`](docs/NATIVE_MIGRATION.md)
- [`docs/NATIVE_ONLY_REDESIGN.md`](docs/NATIVE_ONLY_REDESIGN.md)

## Install

Detailed instructions: [`docs/INSTALLATION.md`](docs/INSTALLATION.md)

### Build from source

```bash
cargo build --manifest-path rust/Cargo.toml --release
./scripts/build-native-app.sh
```

### Install with Cargo

```bash
cargo install --path rust --locked
./scripts/build-native-app.sh
apw app install
```

### Homebrew

For local formula validation from this checkout:

```bash
./packaging/homebrew/install-from-source.sh
```

The formula template is kept in
[`packaging/homebrew/apw.rb.template`](packaging/homebrew/apw.rb.template) and can be rendered with
`scripts/render-homebrew-formula.sh <version> <sha256>`.

## Quick start

The supported `v2.0.0` bootstrap flow is app-first:

```bash
./scripts/build-native-app.sh
apw app install
apw app launch
apw doctor --json
apw login https://example.com
```

The current bootstrap domain is `https://example.com`. The APW app uses a
same-user local broker socket and explicit approval UI for the returned
credential flow.

Optional reduced-security mode for external password managers can be configured
in `~/.apw/config.json` with an absolute provider path:

```json
{
  "fallbackProvider": "1password",
  "fallbackProviderPath": "/usr/local/bin/op"
}
```

Supported fallback providers are `1password` and `bitwarden`. APW does not
cache external-provider credentials.

## Common commands

```bash
apw --help
apw app install
apw app launch
apw doctor
APW_LOG=debug apw status --json
apw status
apw status --json
apw version
apw version --json
apw fill https://example.com
apw login https://example.com
```

Machine-readable build metadata is available via `apw version` and
`apw version --json`.

Legacy migration commands remain available in the repo:

```bash
apw start
apw auth
apw pw
apw otp
apw host doctor --json
```

## Security and storage

- APW stores legacy runtime config in `~/.apw/config.json`
- The v2 app broker stores bootstrap runtime state under `~/.apw/native-app/`
- `~/.apw` is created with mode `0700`
- config and status files are written with mode `0600`
- Legacy session secret material is stored in the user keychain when the `v1.x` compatibility path is used
- Transport, parser, and status errors are returned as typed failures instead of silent partial output

Security and release validation guidance:
[`docs/SECURITY_POSTURE_AND_TESTING.md`](docs/SECURITY_POSTURE_AND_TESTING.md)

## Repository layout

- [`rust/`](rust/): supported CLI, legacy daemon, migration scaffolding, and packaging target
- `native-app/`: v2 bootstrap macOS app bundle and local broker service
- `native-host/`: legacy macOS companion host from the parity line
- [`browser-bridge/`](browser-bridge/): legacy bridge retained only during migration
- [`legacy/deno/`](legacy/deno/): archived compatibility reference
- [`packaging/homebrew/`](packaging/homebrew/): Homebrew formula and local install helpers
- [`docs/`](docs/): installation, migration, archive, security, and breakout docs

## Parity and migration

Rust is still the maintained CLI path, but the active product contract is now
the native app broker. The Deno implementation remains only for audit and
behavior comparison.

Parity and archive details:
[`docs/MIGRATION_AND_PARITY.md`](docs/MIGRATION_AND_PARITY.md)

Migration details:
[`docs/NATIVE_MIGRATION.md`](docs/NATIVE_MIGRATION.md)

## License

This project is licensed under `GPL-3.0-only`. See
[`LICENSE`](LICENSE).
