# Installation and operation

APW ships as a Rust-native CLI plus a local macOS app broker. The supported
executable name is `apw`.

Release reference version: `v2.0.0`

## Platform support

- Supported: macOS
- Preferred runtime on macOS: APW local app broker mode
- Unsupported: non-macOS platforms

When run on an unsupported platform, APW fails fast with an explicit error
instead of attempting degraded behavior.

## Install from source

From the repository root:

```bash
cargo build --manifest-path rust/Cargo.toml --release
./scripts/build-native-app.sh
```

The resulting binary is:

```text
rust/target/release/apw
```

### Install manually

```bash
install -m 0755 rust/target/release/apw /usr/local/bin/apw
# run from the source checkout so apw can find native-app/dist/APW.app
apw app install
```

### Install with Cargo

```bash
cargo install --path rust --locked
./scripts/build-native-app.sh
apw app install
```

## Homebrew

### Local formula smoke test

To validate the bundled formula from this checkout:

```bash
./packaging/homebrew/install-from-source.sh
```

This validates:

- source archive creation
- formula install path
- native app bundle packaging
- `apw --version`
- `apw version --json`
- `apw status --json`

### Publish your own tap

Use [`packaging/homebrew/apw.rb`](../packaging/homebrew/apw.rb)
as the formula template.

After installing with Homebrew, install the per-user APW app bundle:

```bash
apw app install
```

## APW app setup

Install the app bundle and launch the local broker before expecting the v2
credential flow to work.

```bash
apw app install
apw app launch
apw doctor --json
```

## Request a credential

### Check health

```bash
apw status
apw status --json
```

Healthy v2 bootstrap state usually looks like:

1. `app.installed = true`
2. `app.service.running = true`
3. `app.service.live.serviceStatus = "running"`

### Request a credential

```bash
apw login https://example.com
```

The default install does not materialize a demo credential. To exercise the
bundled bootstrap credential for `https://example.com`, opt in explicitly:

```bash
APW_DEMO=1 apw app install
APW_DEMO=1 apw app launch
APW_DEMO=1 apw login https://example.com
```

Without `APW_DEMO=1`, `apw login` returns a typed `no_credential_source`
error for the demo domain. The real
`AuthenticationServices` broker is tracked in issue #13.

### External fallback providers

When a fallback provider is configured in `~/.apw/config.json`
(`fallbackProvider` + `fallbackProviderPath`), the CLI validates the
provider binary before each invocation:

- the path must be absolute and must not start with `~`
- the resolved file (after `realpath`) must be a regular file, owned by the
  current user, with the execute bit set, and not world-writable

Each invocation is bounded by:

- `APW_FALLBACK_TIMEOUT_MS` (default 15000) — wall-clock timeout per exec;
  the child is killed if it exceeds it
- `APW_FALLBACK_INVOCATION_LIMIT` (default 5) — maximum invocations per
  `apw` process before further calls are refused

## Diagnostics

### Machine-readable status

```bash
apw status --json
```

Important v2 fields:

- `releaseLine`
- `app.installed`
- `app.bundleVersion`
- `app.service.running`
- `app.service.transport`
- `app.service.live`
- `session`
- `daemon`
- `host`
- `bridge`

The legacy daemon/host/bridge sections remain in the payload for migration and
diagnostics, but the new primary health model is app-first.

## Development and release checks

Recommended local gates before publishing:

```bash
cargo fmt --manifest-path rust/Cargo.toml -- --check
cargo clippy --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path rust/Cargo.toml --all-targets
cargo test --manifest-path rust/Cargo.toml --test native_app_e2e
cargo build --manifest-path rust/Cargo.toml --release
./scripts/build-native-app.sh
```

Optional parity and release helpers:

```bash
cargo test --manifest-path rust/Cargo.toml --test legacy_parity
./scripts/release-bootstrap.sh
```
