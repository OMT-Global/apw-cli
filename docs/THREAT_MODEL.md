# APW threat model

This document describes the supported APW v2 credential-broker security
boundary after the native-app cutover.

Release reference version: `v2.1.0` planning baseline

## Assets

- iCloud Keychain password credentials for app-associated domains.
- Same-user broker requests and responses between the Rust CLI and `APW.app`.
- AppleScript and Shortcuts/AppIntents automation requests for `APW.app`.
- APW runtime config under `~/.apw/config.json`.
- Associated-domain entitlement and AASA state used to decide which domains
  the app may request from Apple Passwords.
- Optional external password-manager fallback output.
- Release artifacts, signatures, notarization tickets, and install packages.

APW is scoped to credentials the signed macOS app is entitled to request through
Apple associated-domain and AuthenticationServices policy. Domains outside the
embedded entitlement are out of scope for credential access.

## Trust boundaries

- CLI to native app: same-user local UNIX socket under
  `~/.apw/native-app/`. The socket is local-only, permission checked, and
  bounded by request and response size limits. `NSXPCConnection` was considered
  in the redesign notes, but the current supported transport is the UNIX socket.
- Native app to Apple Passwords: public AuthenticationServices APIs and
  system-mediated credential UI. Credential release must remain user mediated.
- Local automation to native app: `APW.app` intentionally publishes an
  AppleScript dictionary and Shortcuts/AppIntents commands for `request login`
  and `request fill`. These entrypoints are same-user local automation surfaces
  and must preserve the same broker envelope, HTTPS validation, and
  user-mediated credential release semantics as the CLI.
- Associated-domain trust input: the app's signed entitlement and each
  domain's AASA file determine which hosts can be requested.
- CLI to external fallback provider: opt-in subprocess execution from an
  absolute configured path, enabled only by an explicit command flag.
- Runtime files: same-user `~/.apw` state stores config and status metadata but
  must not persist plaintext user credentials by default.
- Repository to release artifact: build, signing, notarization, and package
  publication gates.

## Retired surfaces

The UDP daemon listener, browser-extension bridge, and private native-host
helper launch path are legacy compatibility surfaces. They are not part of the
supported v2 credential-broker boundary and must not receive new feature work.
Archive/removal is tracked in issue #47, with command removal tracked in issue
#46. Until those changes land, tests may keep compatibility behavior
reproducible, but release security claims should be made against the native app
broker, not the legacy daemon.

## Attacker models

- Local unprivileged user on the same machine attempting cross-user credential
  access.
- Same-user malicious process attempting to impersonate the broker socket,
  replay a credential response, tamper with runtime files, or drive the
  scriptable automation surface repeatedly to induce prompt fatigue.
- Malicious or corrupted `~/.apw/config.json` attempting to redirect fallback
  provider execution or claim unsupported domains.
- Compromised external password-manager CLI or shim returning malformed,
  oversized, or unexpected output.
- Operator error during domain expansion, signing, notarization, diagnostic
  bundle export, or release packaging.

## Threat matrix

| Threat | Mitigation | Residual risk or follow-up |
| --- | --- | --- |
| Unsupported domain asks for a credential | URL host validation, associated-domain entitlement scoping, config allowed only to narrow the active domain set, and `apw doctor` AASA checks | Production-domain validation remains tracked by issue #8 and real-hardware validation by issue #43 |
| Broker socket spoofing or replay | Same-user runtime directory, socket type and permission checks before connect, bounded request/response sizes, typed request IDs, and timeouts | Same-user malware can still observe user-owned processes; this is out of scope for APW |
| Malformed or oversized broker response | JSON envelope validation, maximum response size, typed error mapping, and regression coverage | Keep new broker commands on the same envelope |
| User cancels, denies, or times out in AuthenticationServices | Stable broker error codes and messages, no credential persistence on failure, and `userMediated: true` on success | Real notarized host coverage is tracked by issue #43 |
| Same-user app drives AppleScript or Shortcuts requests repeatedly | Automation entrypoints route through the same broker request envelope and HTTPS validation as the CLI, the scripting dictionary documents user mediation, and credential release still depends on AuthenticationServices or APW-owned approval UI | `APW.app` is currently not sandboxed and has no automation rate limit/coalescing guard; issue #96 tracks whether to add App Sandbox entitlements and prompt-fatigue controls before broadening automation support |
| External fallback path hijack | Requires config plus explicit `--external-fallback`, rejects relative and `~` paths, validates executable permissions through symlink targets, uses bounded reads and process-group timeouts, and marks payloads `securityMode: reduced_external_cli` | External providers are an accepted reduced-security mode, not equivalent to Apple Passwords mediation |
| Runtime config tampering | `~/.apw` and generated files use owner-only permissions, config contains routing metadata rather than plaintext credentials, and managed config is tracked separately | Enterprise override policy is tracked by issue #51 |
| AASA or entitlement drift | Domain expansion playbook documents entitlement, AASA, signing, notarization, and doctor validation steps | Wildcard/multi-tenant entitlement strategy remains future work under issue #8 |
| Diagnostic bundle leaks secrets | Bundle export excludes credential/config/log files, audits every staged string for token-like data, aborts fail-closed, and writes archives mode `0600` | Keep new diagnostic fields behind the same redaction audit |
| Release artifact tampering | Release gates include build, test, signing, notarization, package verification, and universal-binary checks | Notarization automation is tracked by issue #7; universal checks by issue #53 |

## Security regression map

- `rust/tests/security_regressions.rs` covers invalid inputs, launch failure
  precedence, stable status shape, fallback provider path hardening, and this
  threat-model drift guard.
- `rust/src/native_app.rs` unit tests cover socket safety, stale socket
  fallback, timeout handling, direct-exec fallback, and external provider
  parsing.
- `rust/tests/native_app_e2e.rs` covers native app install, launch, doctor,
  login, diagnostic-bundle redaction, and fail-closed bundle export behavior.
- `native-app/Tests/NativeAppTests/BrokerCoreTests.swift` covers the
  AuthenticationServices broker routing contract with injected success and
  denial outcomes, plus automation envelope parity for AppleScript/Shortcuts
  requests.
- `scripts/test-native-automation-config.sh` checks that the scripting
  dictionary, AppIntents, Info.plist, Swift bridge, and documented automation
  risk posture stay aligned.
- `docs/SECURITY_POSTURE_AND_TESTING.md` lists the release gates that must stay
  aligned with this threat model.

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
the embedded entitlement. The operator playbook is
[DOMAIN_EXPANSION.md](DOMAIN_EXPANSION.md).
