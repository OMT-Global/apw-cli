# Enterprise Deployment

APW supports managed macOS preferences for MDM deployments. Managed values are read from the `dev.omt.apw` preferences domain before `~/.apw/config.json`, so an organization can pin enterprise settings while still allowing per-user auth material to remain local.

Managed keys:

- `fallbackProvider`: external provider id, currently `1password` or `bitwarden`.
- `fallbackProviderPath`: absolute path to the provider executable. APW still validates ownership and executable permissions before use.
- `fallbackProviderTimeoutMs`: per-call provider timeout in milliseconds.
- `fallbackProviderMaxInvocations`: maximum provider invocations per broker request.
- `supportedDomains`: associated domains that the native app should treat as managed.
- `disableDemo`: disables demo affordances for managed deployments when `true`.

`apw doctor --json` includes a `managed-config` check with per-setting provenance. Each managed key reports `"source": "managed"`; otherwise settings report `"user"` when that specific setting is present in `~/.apw/config.json` or `"default"` when APW is using the built-in default.

Sample `.mobileconfig` payload:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>PayloadContent</key>
  <array>
    <dict>
      <key>PayloadType</key>
      <string>com.apple.ManagedClient.preferences</string>
      <key>PayloadIdentifier</key>
      <string>dev.omt.apw.managed</string>
      <key>PayloadUUID</key>
      <string>00000000-0000-4000-8000-000000000051</string>
      <key>PayloadVersion</key>
      <integer>1</integer>
      <key>PayloadEnabled</key>
      <true/>
      <key>PayloadContent</key>
      <dict>
        <key>dev.omt.apw</key>
        <dict>
          <key>Forced</key>
          <array>
            <dict>
              <key>mcx_preference_settings</key>
              <dict>
                <key>fallbackProvider</key>
                <string>1password</string>
                <key>fallbackProviderPath</key>
                <string>/Applications/1Password.app/Contents/MacOS/op</string>
                <key>fallbackProviderTimeoutMs</key>
                <integer>2500</integer>
                <key>fallbackProviderMaxInvocations</key>
                <integer>2</integer>
                <key>supportedDomains</key>
                <array>
                  <string>example.com</string>
                  <string>login.example.com</string>
                </array>
                <key>disableDemo</key>
                <true/>
              </dict>
            </dict>
          </array>
        </dict>
      </dict>
    </dict>
  </array>
  <key>PayloadDisplayName</key>
  <string>APW Managed Settings</string>
  <key>PayloadIdentifier</key>
  <string>dev.omt.apw.profile</string>
  <key>PayloadOrganization</key>
  <string>Example Org</string>
  <key>PayloadRemovalDisallowed</key>
  <false/>
  <key>PayloadType</key>
  <string>Configuration</string>
  <key>PayloadUUID</key>
  <string>00000000-0000-4000-8000-000000000052</string>
  <key>PayloadVersion</key>
  <integer>1</integer>
</dict>
</plist>
```
