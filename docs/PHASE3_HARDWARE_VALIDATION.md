# Phase 3 notarized hardware validation

Issue: #43

Phase 3 is not complete until APW.app has been exercised on real macOS
hardware as a Developer ID signed, notarized, stapled bundle with associated
domain entitlements. CI can build and unit test the broker, but it cannot prove
that Apple's credential picker appears for a notarized app on a user's machine.

## Validation command

Run this on the real validation host:

```bash
./scripts/validate-phase3-hardware.sh \
  --app /path/to/APW.app \
  --apw /path/to/apw \
  --url https://example.com \
  --unsupported-url https://unsupported.invalid \
  --report docs/phase3-hardware-validation-report.md
```

Use a test associated domain that has a valid AASA file and an iCloud Keychain
credential already saved for that domain. The script intentionally does not
persist returned usernames or passwords.

## What the script proves

The script fails closed unless all of these checks pass:

- host is macOS
- `APW.app` exists and contains `Contents/MacOS/APW`
- the app bundle passes `codesign --deep --strict --verify`
- the app bundle passes `spctl --assess --type execute`
- the app bundle passes `xcrun stapler validate`
- bundle entitlements include at least one `webcredentials:` associated domain
- `apw app install` succeeds
- `apw app launch` succeeds
- `apw status --json` reports the app installed and the broker running
- `apw login <url>` exits successfully
- the operator confirms the native iCloud Keychain picker appeared
- the operator confirms the selected credential was returned by APW
- the operator records cancel, denied, and timeout observations
- an unsupported-domain credential request fails with a domain/no-credential
  error

The operator confirmations are required because the picker is a user-mediated
OS UI flow and the script must not scrape or save credential values.

## Error paths to record

During a successful run, the script requires observations for the documented
error paths before it writes the generated report:

- cancel: dismiss the credential picker and record the broker error code
- denied: deny the APW approval prompt, when that prompt is present
- timeout: stop or block the broker and record the CLI timeout code
- unsupported domain: request a domain outside the app entitlement set
  (automated by `--unsupported-url`)

Do not remove the Phase 3 exit blocker in `docs/NATIVE_ONLY_REDESIGN.md` until
the report captures success plus the required error paths on a notarized host.
