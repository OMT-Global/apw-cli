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
apw login https://vault.example.com
```

In a notarized build with associated-domain entitlements wired,
`apw login` routes through the
[`AuthenticationServicesBroker`](native-app/Sources/NativeAppLib/AuthenticationServicesBroker.swift)
and returns an iCloud Keychain credential surfaced via the Apple
credential picker (issue #13).

A separate **demo bootstrap path** is available for first-run
validation. Setting `APW_DEMO=1` makes the broker materialize and
return the bundled placeholder credential for `https://example.com` —
nothing else. Without `APW_DEMO=1`, the demo path returns a typed
`no_credential_source` error rather than silently falling back to a
plaintext file (issue #14):

```bash
APW_DEMO=1 apw app install
APW_DEMO=1 apw app launch
APW_DEMO=1 apw login https://example.com
```

Optional reduced-security mode for external password managers can be configured
in `~/.apw/config.json` with an absolute provider path:

```json
{
  "fallbackProvider": "1password",
  "fallbackProviderPath": "/usr/local/bin/op"
}
```

Supported fallback providers are `1password`, `bitwarden`, `keepassxc`, and
`pass`. Configuration alone does not activate fallback for `apw login`; callers
must pass `apw login --external-fallback <url>` to explicitly choose this
reduced-security path when the native broker is unavailable or returns no
results. JSON fallback payloads use `transport: "external_cli"`,
`securityMode: "reduced_external_cli"`, and `externalFallbackExplicit: true` so
automation can distinguish them from native broker approvals. APW does not
cache external-provider credentials.

### Provider-specific setup

- **`1password`** — `fallbackProviderPath` points at the `op` CLI. The vault
  must already be unlocked (`op signin`).
- **`bitwarden`** — `fallbackProviderPath` points at the `bw` CLI. The vault
  must already be unlocked and `BW_SESSION` exported.
- **`keepassxc`** — `fallbackProviderPath` points at `keepassxc-cli` and
  `fallbackProviderDatabase` must be set to the absolute path of a `.kdbx`
  database. The master password is read from the `APW_KEEPASSXC_PASSWORD`
  environment variable and fed to the CLI over stdin; keep it out of
  persistent shell history.

  ```json
  {
    "fallbackProvider": "keepassxc",
    "fallbackProviderPath": "/opt/homebrew/bin/keepassxc-cli",
    "fallbackProviderDatabase": "/Users/example/Passwords.kdbx"
  }
  ```

- **`pass`** ([passwordstore.org](https://www.passwordstore.org/)) —
  `fallbackProviderPath` points at the `pass` CLI. `gpg-agent` handles the
  unlock, so APW never sees the master key. Entries are discovered with
  `pass find <host>`; an entry whose leaf name matches the host is preferred.
  The first line of `pass show` is treated as the password, and
  `user:` / `username:` / `login:` and `url:` / `website:` lines are parsed
  for the remaining fields.

Provider failure modes (locked vault, missing entry, no match) surface as typed
APW errors: a missing entry maps to `no_results`, malformed CLI output maps to
`proto_invalid_response`, and missing configuration maps to `invalid_config`.

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
