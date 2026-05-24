# Security Policy

## Supported Versions

Security fixes are prepared for the maintained Rust-first `v2` line. The
historical Deno implementation under `legacy/deno/` and the archived browser
bridge are retained only for compatibility audits and do not receive new
security fixes.

| Version line | Supported |
| --- | --- |
| `v2.x` | Yes |
| `v1.x` compatibility paths | Best-effort migration support only |
| `legacy/deno/` | No |
| `browser-bridge/` archive | No |

## Reporting a Vulnerability

Report suspected vulnerabilities privately through GitHub Security Advisories
for `OMT-Global/apw-cli`. Include:

- affected APW version or commit,
- operating system and architecture,
- whether `APW_DEMO=1` or `--external-fallback` was used,
- exact command or broker request shape,
- impact and any known credential exposure path,
- safe reproduction steps that do not include real secrets.

Do not open a public issue with exploit details, credentials, tokens, private
domains, or machine-local paths.

## Security Scope

In scope:

- the Rust CLI in `rust/`,
- the native macOS app broker in `native-app/`,
- local broker IPC, request/response envelopes, and timeout behavior,
- associated-domain and AASA validation paths,
- release packaging, signing, notarization, and install surfaces,
- explicit external password-manager fallback execution.

Out of scope:

- cross-user access to another macOS account's keychain,
- vulnerabilities in Apple Passwords or AuthenticationServices,
- secrets intentionally returned by a configured external fallback provider,
- archived legacy implementations except where they affect the maintained `v2`
  line.

## Disclosure Expectations

Maintainers will acknowledge valid reports as soon as practical, assess the
affected surface, and coordinate a fix before public disclosure. Security
release checks should include the gates in
[`docs/SECURITY_POSTURE_AND_TESTING.md`](docs/SECURITY_POSTURE_AND_TESTING.md)
and the current threat model in [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md).
