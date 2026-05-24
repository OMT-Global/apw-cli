# APW v2.1.0 Release Notes

## Breaking changes

- `apw otp`, including `apw otp list` and `apw otp get`, has been removed.
  There is no v2 replacement. The native APW broker uses public Apple
  AuthenticationServices APIs, which support app-mediated password credential
  requests and OTP AutoFill provider extensions, but do not expose arbitrary
  iCloud Keychain verification-code retrieval to CLI tools.

## Migration

- Scripts that called `apw otp` should remove that step or use a dedicated
  authenticator/provider workflow outside APW.
- Password credential flows should use `apw login <url>` or `apw fill <url>`.
