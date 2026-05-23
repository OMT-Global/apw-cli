# Native Migration Guide

Release target: `v2.0.0`

APW v2 is a product-contract break. The primary interface is now an app-assisted
credential broker backed by the local APW macOS app, not the historical
browser-helper vault reader flow.

## Command Migration

| Legacy command | v2 status | Replacement |
| --- | --- | --- |
| `apw auth` | removed in v2.1.0 | `apw app launch` then `apw login <url>` |
| `apw auth request` | removed in v2.1.0 | no direct replacement |
| `apw auth response` | removed in v2.1.0 | no direct replacement |
| `apw pw list` | removed in v2.1.0 | no replacement in v2 |
| `apw pw get <url> <username>` | removed in v2.1.0 | `apw login <url>` |
| `apw otp list` | removed in v2.1.0 | no replacement in v2 |
| `apw otp get <url>` | removed in v2.1.0 | no replacement in v2 |
| `apw status` | supported | `apw status --json` now reports app/broker readiness |
| `apw host doctor` | legacy-only | `apw doctor` |
| `apw start` | removed in v2.1.0 | `apw app launch` |

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
- Legacy `auth`, `pw`, `otp`, and `start` commands are no longer available in
  the active CLI. The archived parity line remains under `legacy/`.
