# Native Migration Guide

Release target: `v2.0.0`

APW v2 is a product-contract break. The primary interface is now an app-assisted
credential broker backed by the local APW macOS app, not the historical
browser-helper vault reader flow.

## Command Migration

| Legacy command | v2 status | Replacement |
| --- | --- | --- |
| `apw auth` | legacy-only | `apw app launch` then `apw login <url>` |
| `apw auth request` | legacy-only | no direct replacement |
| `apw auth response` | legacy-only | no direct replacement |
| `apw pw list` | legacy-only | no replacement in v2 |
| `apw pw get <url> <username>` | legacy-only | `apw login <url>` |
| `apw otp list` | removed | no replacement in v2 |
| `apw otp get <url>` | removed | no replacement in v2 |
| `apw status` | supported | `apw status --json` now reports app/broker readiness |
| `apw host doctor` | legacy-only | `apw doctor` |
| `apw start` | legacy-only | `apw app launch` |

## Bootstrap Flow

1. Build the app bundle with `./scripts/build-native-app.sh`
2. Install it with `apw app install`
3. Launch the local broker with `apw app launch`
4. Inspect readiness with `apw doctor --json`
5. Exercise the first supported flow with `apw login https://example.com`

## Notes

- `v1.x` remains the historical parity line for browser-helper behavior.
- The v2 bootstrap currently supports one demo associated domain:
  `https://example.com`
- Legacy `auth` and `pw` commands remain in the repo for migration and
  reference, but they are no longer the primary contract.
- `apw otp` is removed from the Rust CLI. Apple's public
  AuthenticationServices path supports app-mediated password credentials for
  sign-in and OTP AutoFill provider extensions that supply codes to the system;
  it does not provide an APW-style API for a CLI to retrieve arbitrary
  iCloud Keychain verification codes. See
  [MIGRATION_AND_PARITY.md](MIGRATION_AND_PARITY.md) for the linked framework
  references.
