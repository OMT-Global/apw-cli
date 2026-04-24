# Legacy Archive Policy

## Canonical archive path

The full archived, non-maintained implementation is at:

- `legacy/deno/` (repo path)

The browser bridge is also archived in place:

- `browser-bridge/` (repo path)

## Purpose of archive

- Preserve the original Deno implementation as a compatibility and audit reference.
- Keep behavior and protocol history reproducible for manual verification.
- Enable one-time or periodic compatibility spot checks during major Rust milestones.

## Maintenance rules

- `legacy/deno/` is read-only by default.
- `browser-bridge/` is read-only by default and carries an in-directory
  `ARCHIVED` tombstone.
- No feature work or new behavior should be introduced there.
- CI, lint, build, and packaging should target `rust/` only.
- Use `legacy/deno/` or `browser-bridge/` only for:
  - behavior audits,
  - historical diffing,
  - explicit, manual compatibility re-runs.

## Safety guardrails

- Never treat archive behavior as a source of truth for releases.
- Do not apply dependency or security hardening changes only in the archive.
- All release gates, changelog updates, and bug fixes must land in Rust.
- `CODEOWNERS` explicitly owns `browser-bridge/` so archive changes require
  code-owner review even though the bridge has no active product CI.

## First-run policy (for maintainers)

- If you are running this project as an active codebase, use `rust/` CLI and daemon paths.
- Ignore `legacy/deno/` unless you are explicitly performing a compatibility audit.
- On first run of a new checkout, execute the normal Rust workflow first:
  - `cargo fmt --manifest-path rust/Cargo.toml -- --check`
  - `cargo clippy --manifest-path rust/Cargo.toml --all-targets -- -D warnings`
  - `cargo test --manifest-path rust/Cargo.toml`
