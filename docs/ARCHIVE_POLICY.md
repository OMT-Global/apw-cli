# Legacy Archive Policy

## Canonical archive path

The archived, non-maintained implementations are at:

- `legacy/deno/` (repo path)
- `legacy/browser-bridge/` (repo path)
- `legacy/native-host/` (repo path)
- `legacy/rust-src/` (repo path, old daemon/browser/native-host internals)
- `legacy/scripts/` (repo path, old browser/native-host helper scripts)

## Purpose of archive

- Preserve the original Deno implementation as a compatibility and audit reference.
- Keep behavior and protocol history reproducible for manual verification.
- Enable one-time or periodic compatibility spot checks during major Rust milestones.

## Maintenance rules

- `legacy/deno/` is read-only by default.
- `legacy/browser-bridge/` is read-only by default and carries an in-directory
  `ARCHIVED` tombstone.
- `legacy/native-host/`, `legacy/rust-src/`, and `legacy/scripts/` are read-only
  by default.
- No feature work or new behavior should be introduced there.
- CI, lint, build, and packaging should target `rust/` only.
- Use `legacy/` only for:
  - behavior audits,
  - historical diffing,
  - explicit, manual compatibility re-runs.

## Safety guardrails

- Never treat archive behavior as a source of truth for releases.
- Do not apply dependency or security hardening changes only in the archive.
- All release gates, changelog updates, and bug fixes must land in Rust.
- `CODEOWNERS` explicitly owns archive paths so changes require code-owner
  review even though they have no active product CI.

## First-run policy (for maintainers)

- If you are running this project as an active codebase, use `rust/` CLI and
  `native-app/` broker paths.
- Ignore `legacy/` unless you are explicitly performing a compatibility audit.
- On first run of a new checkout, execute the normal Rust workflow first:
  - `cargo fmt --manifest-path rust/Cargo.toml -- --check`
  - `cargo clippy --manifest-path rust/Cargo.toml --all-targets -- -D warnings`
  - `cargo test --manifest-path rust/Cargo.toml`
