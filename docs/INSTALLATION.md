# Installation and operation

APW ships as a Rust-native CLI and daemon. The supported executable name is
`apw`.

Release reference version: `v1.2.0`

## Platform support

- Supported: macOS
- Preferred runtime on macOS 26.x: native companion host mode
- Unsupported: non-macOS platforms

When run on an unsupported platform, APW fails fast with an explicit error
instead of attempting degraded behavior.

## Install from source

From the repository root:

```bash
cargo build --manifest-path rust/Cargo.toml --release
./scripts/build-native-host.sh
```

The resulting binary is:

```text
rust/target/release/apw
```

### Install manually

```bash
install -m 0755 rust/target/release/apw /usr/local/bin/apw
# run from the source checkout so apw can find native-host/dist/APWNativeHost.app
apw host install
```

### Install with Cargo

```bash
cargo install --path rust --locked
./scripts/build-native-host.sh
apw host install
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
- native host bundle packaging
- `apw --version`
- `apw status --json`

### Publish your own tap

Use [`packaging/homebrew/apw.rb`](/Users/johnteneyckjr./src/apw/packaging/homebrew/apw.rb)
as the formula template.

Typical public flow:

1. Publish a tagged release
2. Update formula `url`, `sha256`, and `version`
3. Push the formula to your tap
4. Install with:

```bash
brew tap <owner>/apw-native
brew install <owner>/apw-native/apw
```

After installing with Homebrew, install the per-user native host bundle:

```bash
apw host install
```

If you want Homebrew to manage the daemon lifecycle after host install:

```bash
brew services start apw
```

## Native host setup on macOS 26.x

On macOS 26.x, the default runtime is the native companion host. Install the
per-user app bundle and LaunchAgent before expecting end-to-end auth or
data-plane commands to work.

```bash
apw host install
apw host doctor --json
```

## Start and authenticate

### Start the daemon

```bash
apw start
```

Optional bind override:

```bash
apw start --bind 127.0.0.1 --port 10000
```

### Check health

```bash
apw status
apw status --json
```

Healthy native-host state usually looks like:

1. `daemon.runtimeMode = "native"`
2. `host.status = "attached"`
3. `daemon.preflight.status = "ready"`

### Authenticate interactively

```bash
apw auth
```

### Authenticate non-interactively

```bash
apw auth --pin 123456
```

### Explicit request/response auth flow

```bash
apw auth request
apw auth response --pin 123456 --salt <salt> --server_key <server_key> --client_key <client_key> --username <username>
```

## Diagnostics

### Machine-readable status

```bash
apw status --json
```

Important fields:

- `daemon.host`
- `daemon.port`
- `daemon.runtimeMode`
- `daemon.lastLaunchStatus`
- `daemon.lastLaunchError`
- `daemon.lastLaunchStrategy`
- `daemon.preflight`
- `host.status`
- `host.connectedAt`
- `host.bundleVersion`
- `host.lastError`
- `bridge.status`
- `bridge.browser`
- `bridge.connectedAt`
- `bridge.lastError`
- `session.username`
- `session.createdAt`
- `session.expired`
- `session.authenticated`

`daemon.preflight` is the public diagnostics block for runtime decisions. It
includes resolved mode, candidate launch strategies, native host socket and
LaunchAgent checks, app bundle checks, helper binary checks, and a structured
failure reason when the native host cannot work on the current host.

### Direct helper launch diagnostics

Use this to confirm whether the host allows direct native helper launch:

```bash
apw start --runtime-mode direct --dry-run
apw status --json
```

If the host reports a direct helper failure such as a code-signature or parent
process constraint, stay on the native-host path and use `apw host doctor`.

## Development and release checks

Recommended local gates before publishing:

```bash
cargo fmt --manifest-path rust/Cargo.toml -- --check
cargo clippy --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path rust/Cargo.toml --all-targets
cargo build --manifest-path rust/Cargo.toml --release
```

Optional parity and release helpers:

```bash
cargo test --manifest-path rust/Cargo.toml --test legacy_parity
./scripts/release-bootstrap.sh
./scripts/release-bootstrap.sh --host-smoke --pw-domain example.com
```
