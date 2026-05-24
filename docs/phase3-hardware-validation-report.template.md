# Phase 3 hardware validation report

Issue: #43

Status: not yet validated

## Host

- Date:
- macOS version:
- Hardware model:
- Architecture:
- APW.app version:
- APW CLI version:
- Test associated domain:
- Release tag or commit:

## Automated checks

- [ ] `codesign --deep --strict --verify APW.app`
- [ ] `spctl --assess --type execute --verbose APW.app`
- [ ] `xcrun stapler validate APW.app`
- [ ] Associated-domain entitlement contains `webcredentials:`
- [ ] `apw app install`
- [ ] `apw app launch`
- [ ] `apw status --json` reports installed app and running broker
- [ ] `apw login <url>` exits successfully

## Operator-observed flow

- [ ] Native iCloud Keychain credential picker appeared
- [ ] Operator selected the expected test credential
- [ ] APW returned a credential response without saving it to disk

## Error paths

| Path | Expected result | Observed result |
| --- | --- | --- |
| Success | credential response with `userMediated: true` | |
| Cancel | stable canceled/denied broker error | |
| Denied | stable denied broker error | |
| Timeout | communication timeout error | |
| Unsupported domain | no-results or unsupported-domain error | |

## Notes

- Do not paste real usernames, passwords, session tokens, or credential payloads
  into this report.
