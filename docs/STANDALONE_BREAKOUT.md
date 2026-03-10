# Standalone project breakout

This document describes how to take APW from an upstream fork to a standalone
public project while keeping the Rust implementation as the supported path.

Release reference version: `v1.2.0`

## Goal

Create a new standalone repository, preserve the archived Deno history for audit
purposes, and publish the Rust implementation as the only maintained runtime.

## Recommended breakout steps

1. Create the new repository, for example `omt-global/apw-native`
2. Copy the working tree into the new repository without the old `.git` history
3. Preserve `legacy/deno/` as a read-only archive
4. Keep `rust/` as the only release, packaging, and CI target
5. Update repository metadata, Homebrew publishing metadata, and documentation

## Example bootstrap flow

From the existing checkout:

```bash
./scripts/bootstrap-apw-native-standalone.sh
```

## Manual breakout flow

```bash
mkdir -p ../apw-native
rsync -a --exclude='.git' --exclude='target' --exclude='dist' --exclude='build' ./ ../apw-native/
cd ../apw-native
git init -b main
git remote add origin git@github.com:omt-global/apw-native.git
```

Then:

```bash
git add -A
git commit -m "chore: initialize standalone apw-native project"
git push -u origin main
```

## Metadata to update after breakout

- `README.md`
- `packaging/homebrew/apw.rb`
- release workflow repository references
- issue templates, funding, and repository settings if you use them

## Release posture after breakout

- keep `apw` as the installed executable name
- publish binaries from the Rust path only
- treat `legacy/deno/` as archived reference material only
- run parity and security gates before the first public release

Related docs:

- [README.md](/Users/johnteneyckjr./src/apw/README.md)
- [docs/MIGRATION_AND_PARITY.md](/Users/johnteneyckjr./src/apw/docs/MIGRATION_AND_PARITY.md)
- [docs/SECURITY_POSTURE_AND_TESTING.md](/Users/johnteneyckjr./src/apw/docs/SECURITY_POSTURE_AND_TESTING.md)
