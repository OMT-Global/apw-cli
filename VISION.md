# APW CLI Vision

Version: 0.2

APW is a macOS-first command-line entrypoint plus local app broker for user-mediated credential access. The active v2 product is not a vault scraper and not a browser-helper parity project. It is a signed native macOS broker, controlled by the `apw` CLI, that uses supported Apple APIs and explicit local IPC to help a user fill or log in to a site.

The installed command remains `apw`, but the product contract has changed: `apw login`, `apw fill`, `apw status`, `apw doctor`, `apw app install`, and `apw app launch` are the supported direction. Legacy `auth`, `pw`, `otp`, daemon, and browser-helper behavior remains archived for migration and audit reference.

## Who It Serves

- macOS users who want a scriptable way to request Apple Passwords-backed sign-in without exposing a general vault reader.
- Operators building and validating universal CLI plus `APW.app` release artifacts.
- Contributors hardening the broker, diagnostics, packaging, and migration boundary.

## Current Product Boundary

- Supported platform: macOS.
- Supported runtime: Rust CLI plus local native app broker over same-user UNIX socket.
- Supported credential path: app-mediated `AuthenticationServices` flows and explicitly configured reduced-security external fallback.
- Unsupported direction: arbitrary password listing, arbitrary OTP retrieval, private Apple browser-helper coupling, and non-macOS degraded operation.

## Product Principles

- Prefer public Apple frameworks over private helper paths, even when that narrows parity.
- User mediation is a security feature; APW should not silently become a background vault extraction tool.
- Every broker request and response should use typed, bounded JSON envelopes with stable machine-readable errors.
- Diagnostics should fail closed, redact aggressively, and be safe to attach to support requests.
- Release trust belongs in standard macOS mechanisms: signed universal binaries, notarization, Sparkle appcasts, and Homebrew formula validation.

## Near-Term Direction

- Finish the v2 native-only cutover and keep the command migration matrix honest.
- Harden `apw doctor`, diagnostic bundles, broker timeout behavior, and native app readiness reporting.
- Wire Sparkle update metadata only with real signing material and release automation secrets.
- Keep legacy code available for parity audits while making the active CLI/app path smaller and clearer.

## Non-Goals

- Do not promise full iCloud Keychain vault or OTP listing through the CLI.
- Do not preserve browser-extension or private-helper behavior as the long-term product shape.
- Do not cache credentials returned through external fallback providers.
- Do not ship placeholder update trust material or unsigned release paths.
