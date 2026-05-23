# Native-only APW redesign

Status: bootstrap implementation in progress

Target release line: `v2.0.0`

## Decision summary

APW will no longer pursue "browser-free parity by launching Apple's private
browser helper from a native companion host." That path is not a stable product
direction on macOS 26.x.

The native-only direction is now:

- Rust CLI remains first-class as `apw`
- a signed macOS app becomes the supported native runtime surface
- the app uses public Apple APIs such as `AuthenticationServices`
- APW becomes an app-assisted credential broker, not a general iCloud Passwords
  vault reader

## Why this redesign exists

The current parity implementation depends on Apple's browser-managed helper path.
The prototype native host can attach to the Rust daemon, but end-to-end helper
launch is not reliable when Apple's private helper is launched outside its
intended browser-managed context.

The redesign therefore optimizes for:

- native macOS support
- supported Apple APIs
- explicit product boundaries
- long-term maintainability

It does not optimize for preserving every legacy CLI behavior.

## Goals

- Remove Chrome and browser-extension dependency from the supported product
  direction.
- Keep `apw` as the installed command-line entrypoint.
- Replace private-helper coupling with public Apple frameworks.
- Build a signed, notarizable macOS application that mediates credential access.
- Preserve secure local-only IPC and typed JSON/human CLI outputs.

## Non-goals

- Do not preserve arbitrary vault-wide password listing.
- Do not preserve arbitrary OTP listing or retrieval unless proven by supported
  Apple APIs.
- Do not keep the current UDP daemon plus private-helper launch stack as the
  long-term product shape.
- Do not claim full parity with the archived Deno project once the native-only
  cutover begins.

## Product contract

### Old contract

Legacy APW behaves like a vault reader:

- `apw auth`
- `apw pw list`
- `apw pw get <domain> <username>`
- `apw otp list`
- `apw otp get <domain> <username>`

### New contract

Native-only APW behaves like a credential broker:

- `apw login <url>`
- `apw fill <url>`
- `apw status`
- `apw doctor`
- `apw app install`
- `apw app launch`

Returned credentials are:

- domain-scoped
- app-mediated
- user-approved
- subject to Apple entitlement and associated-domain rules

## Architecture

### 1. Swift macOS app

Create `native-app/` as the new primary native runtime project.

Recommended shape:

- `APW.app`
- `LSUIElement` background app or menu bar app
- signed and notarized
- owns `AuthenticationServices` integration
- owns associated-domain configuration and diagnostics
- exposes a local XPC service to the CLI

Responsibilities:

- present native credential selection UI
- request credentials for approved domains
- map Apple framework errors into APW status/error envelopes
- never expose a network listener outside the local machine
- persist only the minimal local state required for app health and diagnostics

### 2. Rust CLI

Keep `rust/` as the maintained CLI codebase.

Responsibilities:

- parse commands
- connect to the local XPC service
- return human or `--json` output
- preserve typed status and error surfaces
- stop trying to launch Apple's private helper directly

### 3. IPC

Preferred IPC mechanism:

- `NSXPCConnection`

Fallback if needed:

- LaunchAgent-backed Mach service

Requirements:

- same-user only
- typed request/response envelopes
- bounded request timeout
- deterministic error mapping
- no remote bind surface

## Proposed command migration

| Legacy command | Native-only status | Notes |
| --- | --- | --- |
| `apw auth` | remove | replaced by app-mediated sign-in flow |
| `apw auth request` | remove | private-helper flow goes away |
| `apw auth response` | remove | private-helper flow goes away |
| `apw pw list` | remove | unsupported as a vault-wide native API contract |
| `apw pw get <domain> <username>` | deprecate then replace | map to `apw login https://<domain>` |
| `apw otp list` | remove | unsupported until proven otherwise |
| `apw otp get` | likely remove | only keep if supported native verification-code path is proven |
| `apw status` | keep | report app/XPC/entitlement readiness |
| `apw start` | remove or repurpose | native app launch replaces daemon start |

## Implementation phases

### Phase 0: branch and preserve parity line

- Freeze the current parity-oriented line as the legacy compatibility branch.
- Keep `legacy/deno/` archived.
- Keep the current Rust parity work available for historical comparison.
- Start the redesign on a new milestone branch for `v2.0.0`.

Exit criteria:

- historical parity line remains reproducible
- native-only work no longer blocks parity fixes

### Phase 1: app skeleton and diagnostics

- Create `native-app/` Swift project
- Add signing-ready bundle metadata
- Add `AuthenticationServices` linkage
- Implement `apw app install`
- Implement `apw app launch`
- Implement `apw status`
- Implement `apw doctor`

Deliverables:

- `APW.app` launches locally
- CLI can detect app presence and app version
- CLI reports entitlement/associated-domain readiness

### Phase 2: XPC contract

- Define a typed XPC protocol
- Implement request/response envelopes
- Add request timeout and failure taxonomy
- Add CLI adapter in Rust

Deliverables:

- CLI can call the app locally
- machine-readable JSON status is stable
- no UDP daemon is required for the native-only path

### Phase 3: first supported credential flow

- Add `apw login <url>`
- Use supported Apple native credential-selection APIs
- Present user-facing picker UI through the app
- Return selected username/password to the CLI only after user mediation

Deliverables:

- end-to-end sign-in flow for one associated domain
- stable error mapping for cancel, denial, timeout, and unsupported-domain cases

#### Phase 3 status (issue #13)

- `native-app/Sources/NativeAppLib/AuthenticationServicesBroker.swift`
  introduces a `CredentialBroker` protocol, the
  `AppleAuthenticationServicesBroker` implementation that drives
  `ASAuthorizationController` + `ASAuthorizationPasswordRequest` on the
  main thread and bridges results back to the worker thread via
  `DispatchSemaphore`, and a stable `BrokerErrorCode` mapping
  (`canceled` / `failed` / `invalidResponse` / `notHandled` / `unknown`).
- `BrokerCore.loginResponse` routes through the injected broker when
  `APW_DEMO` is unset, mapping outcomes onto the existing wire envelope
  (`transport: "authentication_services"`, `userMediated: true`, integer
  status codes that match the Rust `Status` enum).
- `BrokerCoreTests` exercises the broker outcome paths via
  `StubCredentialBroker` for `success` / `denied` / `canceled` /
  `invalidResponse`, and asserts the broker error code mapping.

**Phase 3 exit blockers still open**:

- The integration is unverified against a notarized build with
  associated-domain entitlements wired (the macOS build cannot be
  exercised from CI on Linux). A follow-up validation pass on a real
  macOS host is required before declaring Phase 3 complete.
- Domain expansion beyond `example.com` is tracked in issue #8.

### Phase 4: command migration and deprecation

- Add compatibility warnings to `pw` and `otp`
- Map `pw get <domain>` to the new login flow where appropriate
- Remove unsupported commands from primary docs
- Preserve a migration guide for operators moving from parity APW to native-only

Deliverables:

- clear CLI help text
- explicit migration notices
- browser/runtime code marked legacy

### Phase 5: packaging and release

- ship `APW.app`
- ship `apw` CLI
- add notarization/signing pipeline
- move Homebrew distribution to a cask or mixed formula-plus-app install
- add manual install path for non-Homebrew users

Deliverables:

- signed local build
- notarized release candidate
- install docs for app plus CLI

## Repository changes

### New top-level paths

- `native-app/`
- `docs/NATIVE_ONLY_REDESIGN.md`
- `docs/NATIVE_MIGRATION.md`

### Rust changes

- add an app/XPC client module
- deprecate daemon/helper-specific runtime modes in CLI help
- introduce a new command family:
  - `apw app install`
  - `apw app launch`
  - `apw doctor`
  - `apw login <url>`

### Code to archive after cutover

- `browser-bridge/`
- native-host private-helper bridge code
- helper manifest install scripts
- launchd/direct helper runtime modes

## Security requirements

- all local IPC must be same-user only
- no UDP listener for the native-only path
- timeouts and explicit status envelopes on every CLI request
- no secret persistence in app logs
- no silent credential fallback
- explicit user mediation for credential access
- signed app bundle for release builds

## Testing plan

### Rust

- CLI parser coverage for new commands
- XPC client error mapping tests
- JSON output contract tests
- migration tests for deprecated commands

### Swift

- unit tests for request handling
- unit tests for `AuthenticationServices` wrappers
- integration tests for XPC service lifecycle
- UI tests for credential selection and cancellation

### End-to-end

- install app
- launch app
- `apw status --json`
- `apw doctor --json`
- `apw login https://example.com`
- verify cancel, denied, timeout, unsupported-domain, and success cases

## Risks and open questions

- Associated domains may make APW practical only for explicitly configured sites.
- OTP parity may not survive the redesign.
- Homebrew-only installation may be insufficient once a signed app bundle is
  required.
- UI mediation means APW becomes less scriptable by design.

These are product decisions, not implementation bugs.

## Immediate next actions

1. Create `docs/NATIVE_MIGRATION.md` with a command-by-command migration matrix.
2. Create `native-app/` with the minimal signed app skeleton.
3. Prototype `apw status` against a local app presence check instead of the
   current native-host helper flow.
4. Spike one supported credential request flow for a single associated domain.
5. Decide whether `v2.0.0` is a hard product break or ships with a temporary
   compatibility shim for `pw get`.

## Release framing

Use a version boundary that is honest:

- `v1.x`: parity-oriented Rust line with browser-managed helper support
- `v2.0.0`: native-only redesign with a changed contract
