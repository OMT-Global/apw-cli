# Adding production domains to the APW v2 native app

Issue: [#8](https://github.com/OMT-Global/apw-cli/issues/8)

The `v2.0.0` native app supports `https://example.com` as the bundled
demo associated domain. Operators who want APW to broker credentials
for additional domains must extend the macOS `Associated Domains`
entitlement, host an `apple-app-site-association` (AASA) file at each
target domain, and re-sign / re-notarize the rebuilt bundle.

This document is the operator playbook for that work.

## Prerequisites

- macOS with Xcode and a valid `Developer ID Application` certificate
  (run `apw doctor` to confirm — issue #12).
- Apple Notary credentials wired into release CI (issue #7).
- Write access to the DNS / `/.well-known` path of every target domain.

## Step 1: list the domains in `~/.apw/config.json`

Add (or update) the `supportedDomains` array in the user config. The
field is validated against the bundle's `Associated Domains` entitlement
at runtime, so it cannot claim more domains than the app is entitled to.

```json
{
  "schema": 1,
  "supportedDomains": [
    "example.com",
    "vault.acme.example",
    "internal.acme.example"
  ]
}
```

## Step 2: extend the app entitlement

Edit `native-app/Sources/NativeApp/APW.entitlements` and add one
`webcredentials:<domain>` entry per target domain inside the
`com.apple.developer.associated-domains` array. Example:

```xml
<key>com.apple.developer.associated-domains</key>
<array>
  <string>webcredentials:example.com</string>
  <string>webcredentials:vault.acme.example</string>
  <string>webcredentials:internal.acme.example</string>
</array>
```

Wildcards (`webcredentials:*.acme.example`) are allowed but each base
domain must still serve a valid AASA file.

## Step 3: serve a valid AASA file at each domain

Each target domain must serve a publicly-reachable AASA file at:

```
https://<domain>/.well-known/apple-app-site-association
```

The file must be served as `application/json`, must not redirect, and
must include the `webcredentials.apps` array with the APW bundle id:

```json
{
  "webcredentials": {
    "apps": ["<TEAM_ID>.dev.omt.apw"]
  }
}
```

`<TEAM_ID>` is the 10-character Apple Developer Team ID that signs the
APW.app bundle.

Apple's CDN caches AASA files aggressively; allow up to 24h between an
AASA update and end-user broker behavior.

## Step 4: rebuild, re-sign, re-notarize

```bash
./scripts/build-native-app.sh
# Sign with the Developer ID Application certificate (release.yml will
# automate this once issue #7 lands).
xcrun notarytool submit native-app/dist/APW.app.zip --wait \
    --key "$APPLE_NOTARY_PRIVATE_KEY" \
    --key-id "$APPLE_NOTARY_KEY_ID" \
    --issuer "$APPLE_NOTARY_KEY_ISSUER"
xcrun stapler staple native-app/dist/APW.app
apw app install
```

## Step 5: verify with `apw doctor`

Run `apw doctor --json` after install. The `app.frameworks` block
reports the entitlement domains the bundle was signed with, and the
`environment` array (issue #12) probes reachability of each AASA file
under `app.aasa[]`. Any check that fails surfaces a remediation hint.

## Long-term plan

A multi-tenant entitlement (wildcard subdomain or managed capability)
would remove the per-domain rebuild requirement. That investigation is
captured under issue #8 and is not yet scheduled.
