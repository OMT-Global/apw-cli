# APW CLI Vision

Version: 0.3

APW is a macOS-first command-line entrypoint for user-mediated credential access. The active v2 product is not a vault scraper and not a browser-helper parity project. It is a CLI-native, mediated **orchestrator** across whatever password backend the user actually has: the password-manager CLIs they already trust for arbitrary-site credentials, and Apple's supported sign-in surfaces for the narrow flows those APIs actually permit. A signed native macOS broker (`APW.app`), controlled by the `apw` CLI, brokers the Apple-side flows and local IPC.

The installed command remains `apw`, but the product contract has changed: `apw login`, `apw fill`, `apw status`, `apw doctor`, `apw app install`, and `apw app launch` are the supported direction. Legacy `auth`, `pw`, `otp`, daemon, and browser-helper behavior remains archived for migration and audit reference.

## The Platform Constraint That Shapes This Product

Apple does not provide a supported way for a third-party CLI to read a user's saved website passwords from iCloud Keychain or the Passwords app. There is no entitlement, no `security` subcommand, and no `AuthenticationServices` call that returns the password for an arbitrary site. This is a deliberate platform boundary, and APW is designed around it rather than against it.

What `AuthenticationServices` actually offers is AutoFill-style sign-in, and only for domains the app has an **associated-domain relationship** with — which means APW could only ever return credentials for domains its own team controls and lists in an AASA file. That makes a general-purpose "read my Apple passwords from the terminal" feature impossible on supported APIs. APW treats that capability as permanently out of scope, and does not imply the broker can read the user's vault.

Consequently, the only path that delivers CLI credential access for arbitrary sites on supported, vendor-blessed APIs is integration with the password managers' own CLIs. APW makes that the primary product, not a fallback.

## Who It Serves

- macOS users who want a single scriptable, CLI-native front end over the password backend they already use (1Password, Bitwarden, KeePassXC, `pass`), with consistent mediation and machine-readable output.
- macOS users who want a scriptable way to invoke Apple-backed sign-in for the narrow associated-domain flows those APIs support — without expecting a general vault reader.
- Operators building and validating universal CLI plus `APW.app` release artifacts.
- Contributors hardening the broker, diagnostics, packaging, and migration boundary.

## Current Product Boundary

- Supported platform: macOS.
- Supported runtime: Rust CLI orchestrating configured password-manager backends, plus a local native app broker over a same-user UNIX socket for Apple-side flows.
- Primary credential path: explicit, user-configured password-manager CLI backends (1Password `op`, Bitwarden `bw`, KeePassXC, `pass`), each invocation mediated and bounded.
- Secondary credential path: app-mediated `AuthenticationServices` sign-in, scoped to its real capability (associated domains the project controls, passkey/sign-in flows) — not general vault retrieval.
- Unsupported direction: reading arbitrary iCloud Keychain / Passwords-app entries, arbitrary OTP retrieval, private Apple browser-helper coupling, and non-macOS degraded operation.

## Product Principles

- Design around Apple's platform boundary, not against it. Where a capability is not available through public APIs, treat it as out of scope and say so plainly rather than implying it works.
- Meet users where their credentials already live. A consistent CLI surface over the password-manager backends they already trust is more valuable than a partial reimplementation of any one vault.
- Prefer public Apple frameworks over private helper paths, even when that narrows parity.
- User mediation is a security feature; APW should not silently become a background vault extraction tool.
- Every broker request and response should use typed, bounded JSON envelopes with stable machine-readable errors.
- Diagnostics should fail closed, redact aggressively, and be safe to attach to support requests.
- Release trust belongs in standard macOS mechanisms: signed universal binaries, notarization, Sparkle appcasts, and Homebrew formula validation.

## Near-Term Direction

- Promote password-manager backend integration from "reduced-security fallback" to the first-class, documented primary path: clear backend selection, consistent output, and mediated, bounded invocations.
- Scope the `AuthenticationServices` broker to its real capability and stop framing it as a vault reader; reflect that boundary in `README.md` and `docs/THREAT_MODEL.md`.
- Finish the v2 native-only cutover and keep the command migration matrix honest.
- Harden `apw doctor`, diagnostic bundles, broker timeout behavior, and native app readiness reporting.
- Wire Sparkle update metadata only with real signing material and release automation secrets.
- Keep legacy code available for parity audits while making the active CLI/app path smaller and clearer.

## Non-Goals

- Do not promise iCloud Keychain / Passwords-app vault reading or OTP listing through the CLI; it is not possible on supported APIs and is permanently out of scope.
- Do not drive the Passwords app through Accessibility/UI scripting as a backend; it is fragile, breaks across OS releases, and works against the platform.
- Do not preserve browser-extension or private-helper behavior as the long-term product shape.
- Do not cache credentials returned through password-manager backends.
- Do not ship placeholder update trust material or unsigned release paths.
