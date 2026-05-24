# In-app update contract

APW.app will use Sparkle 2 for in-app updates. The release channel is security
sensitive because the app broker mediates credential access, so APW uses the
standard macOS updater instead of a custom downloader and swapper.

Issue: #57

## Decision

Use Sparkle 2 as the updater framework for APW.app.

Rationale:

- Sparkle is the established macOS updater for Developer ID distributed apps.
- Sparkle supports EdDSA-signed update archives and Apple code signing checks.
- Sparkle can mark critical updates distinctly from ordinary maintenance
  updates.
- Sparkle keeps the update installer and relaunch behavior out of APW broker
  code, reducing the amount of security-sensitive custom code.

APW should not add a custom updater unless Sparkle cannot satisfy a release
blocker that is documented with a replacement threat model.

## Stable feed

The production appcast URL is:

```text
https://github.com/OMT-Global/apw-cli/releases/latest/download/appcast.xml
```

This URL is controlled by the project repository and resolves to the appcast
asset attached to the latest GitHub release. APW.app should set this URL in
`Info.plist` with `SUFeedURL` once Sparkle is linked into the native app.

The appcast contract is represented by
`packaging/sparkle/appcast.template.xml`. The template is not a production
appcast and must not be uploaded with placeholder signatures or lengths.

## Required Sparkle settings

When the runtime integration lands, APW.app must set these keys:

```text
SUFeedURL=https://github.com/OMT-Global/apw-cli/releases/latest/download/appcast.xml
SUPublicEDKey=<release Sparkle EdDSA public key>
SUVerifyUpdateBeforeExtraction=true
SURequireSignedFeed=true
SUEnableAutomaticChecks=true
SUAllowsAutomaticUpdates=false
SUAutomaticallyUpdate=false
```

`SUVerifyUpdateBeforeExtraction` requires EdDSA signing and validates the update
archive before extraction. `SURequireSignedFeed` requires Sparkle 2.9 or newer
and ensures the appcast and release notes are signed before update metadata is
trusted.

## Release signing requirements

Every APW.app update must be published as a Developer ID signed and notarized
archive. Before publishing the appcast, the release job must verify:

```bash
codesign --deep --strict --verify APW.app
spctl --assess --type execute --verbose APW.app
xcrun stapler validate APW.app
```

The release archive, release notes, and appcast must be signed with Sparkle's
EdDSA key. The private EdDSA key must stay in release automation secrets or a
release keychain and must never be committed to this repository.

Sparkle appcast preparation should use the checked helper:

```bash
./scripts/prepare-sparkle-appcast.sh \
  --archive dist/APW.app.zip \
  --release-notes dist/APW.app.release.md \
  --updates-dir dist/sparkle-updates \
  --generate-appcast /path/to/Sparkle/bin/generate_appcast
```

The helper copies the signed/notarized archive and release notes into the
updates directory, runs Sparkle's `generate_appcast`, and fails if the resulting
appcast does not contain EdDSA signatures or does not reference the release
archive. Private EdDSA key material stays with Sparkle's configured signing
environment, such as Keychain-backed release automation.

## Managed update control

Enterprise administrators can disable user-driven update checks with this
managed preference:

```text
Domain: dev.omt.apw
Key: com.omt.apw.updatesDisabled
Type: Boolean
```

When this key is `true`, APW.app must not start Sparkle automatic checks or
manual user-driven update checks. The broker should still report its installed
version through `apw status --json` and `apw doctor --json` so fleet tooling can
inventory stale installations.

This managed key is part of the APW configuration contract and should be wired
through the managed-config roadmap work before the updater runtime is enabled
by default.

## Security update surfacing

Security updates must be distinct from cosmetic or maintenance updates.

Use all of the following for security releases:

- title starts with `APW <version> Security Update`
- appcast item contains top-level `sparkle:criticalUpdate`
- release notes contain a `Security` section before other changes
- appcast item links to the GitHub release notes for the exact tag

Critical update status is reserved for credential-broker security fixes,
signing/notarization failures, or vulnerabilities that can affect credential
confidentiality, integrity, or update trust.

## Validation

Run the contract check with:

```bash
./scripts/ci/validate-appcast-contract.sh
./scripts/test-prepare-sparkle-appcast.sh
```

The fast PR check runs the same validator so changes to the appcast template,
security-update wording, MDM key, Sparkle security settings, or appcast
preparation helper fail before release automation drifts.
