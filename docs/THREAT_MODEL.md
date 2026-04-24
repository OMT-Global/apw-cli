# APW threat model

This document describes the APW v2 credential-broker security boundary.

Release reference version: `v2.0.0`

## Assets

- iCloud Keychain password credentials for app-associated domains.
- Same-user broker requests and responses over the local APW socket.
- APW runtime config under `~/.apw/config.json`.
- Optional external password-manager fallback output.

APW is scoped to credentials the signed macOS app is entitled to request through
Apple associated-domain and AuthenticationServices policy. Domains outside the
embedded entitlements are out of scope for credential access.

## Trust boundaries

- CLI to native app: same-user local IPC under `~/.apw/native-app/`.
- Native app to Apple Passwords: public Apple frameworks and system-mediated
  credential UI.
- CLI to external fallback provider: opt-in subprocess execution from an
  absolute configured path.
- Repository to release artifact: build, signing, notarization, and package
  publication gates.

## Attacker models

- Local unprivileged user on the same machine attempting cross-user credential
  access.
- Malicious or corrupted `~/.apw/config.json` attempting to redirect fallback
  provider execution.
- Rogue local process attempting to impersonate the broker socket or replay a
  credential response.
- Operator error during domain expansion, signing, notarization, or release
  packaging.

## In scope

- Rejecting unsupported domains before a credential is returned.
- Returning typed failures for malformed IPC, unsupported URLs, denied requests,
  and broker timeouts.
- Keeping broker and runtime files same-user scoped.
- Requiring explicit user mediation before credential release.
- Validating external fallback provider configuration before execution.

## Out of scope

- Cross-user access to another macOS account's keychain.
- Remote exploitation of Apple Passwords or AuthenticationServices.
- Credentials for domains not present in the app's associated-domain
  entitlement.
- Bypassing Apple's domain-verification or system credential-selection policy.

## Domain expansion

Adding a production domain requires all of these steps:

1. Add the domain to the app's Associated Domains entitlement.
2. Serve a valid `apple-app-site-association` file from that domain.
3. Rebuild and re-sign the app with the updated entitlement.
4. Re-notarize and staple the app bundle before release.
5. Validate the domain with `apw doctor` or the extended validation suite before
   publishing.

Config may narrow the active domain set, but it must not claim domains beyond
the embedded entitlement.
