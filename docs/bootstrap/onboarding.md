    # Bootstrap Onboarding

    ## Local environment check

    - Run `apw doctor` from a fresh checkout — the first-step diagnostic for new
      contributors. It probes `xcodebuild`, `rustc`, `detect-secrets`, the
      Apple `Developer ID Application` keychain identity, and the APW.app
      bundle install state, and prints a `[OK]/[WARN]/[FAIL]` line per check
      with a remediation hint.
    - For CI consumers and runner inventory work, `apw doctor --ci` emits the
      same checks as a structured JSON array (also honors the global `--json`
      flag). When `CI=true`, set `RUNNER_LABELS` so the doctor can sanity-check
      the runner pool selection (issue #12).

    ## Repo Governance

    - Confirm the repository exists at `OMT-Global/apw-cli`.
    - Confirm branch protection or rulesets on `main` require one approval and code owner review.
    - Confirm branch protection points at the `CI Gate` status.
    - Confirm `delete branch on merge` and `allow auto-merge` are enabled.

    ## Environments

    - `dev`: open by default for rapid iteration.
    - `stage`: one reviewer required and self-review blocked.
    - `prod`: one reviewer required, self-review blocked, deployments limited to `main`.

    ## Runner Policy

    - Shell-safe jobs may use `[self-hosted, synology, shell-only, public]`.
    - Docker, service-container, browser, and `container:` workloads stay on GitHub-hosted runners.
    - Keep PR checks cheap. Add heavy validation to `scripts/ci/run-extended-validation.sh` instead of the PR lane.
    - APW extended validation requires both Rust (`cargo`) and the macOS Swift toolchain, so the `extended-checks` job must run on the org macOS self-hosted pool (`[self-hosted, private, macOS, ARM64, xcode]`) rather than the Synology shell-only pool.

    ## Release Prep

    - Run `scripts/bump-version.sh <version>` from the repository root to update all version-bearing release surfaces.
    - Run `bash scripts/ci/run-fast-checks.sh` after version bumps before opening a release PR.

    ### Release secrets

    The following repository secrets are consumed by `.github/workflows/release.yml`:

    | Secret                       | Purpose                                                       |
    | ---------------------------- | ------------------------------------------------------------- |
    | `APPLE_DEVELOPER_CERT_P12`   | base64-encoded Developer ID Application .p12 (issue #7)        |
    | `APPLE_CERT_PASSWORD`        | passphrase for the .p12 above                                  |
    | `APPLE_TEAM_ID`              | 10-character Apple Developer Team ID                           |
    | `APPLE_NOTARY_KEY_ID`        | App Store Connect API key id used by `notarytool`              |
    | `APPLE_NOTARY_KEY_ISSUER`    | App Store Connect issuer UUID                                  |
    | `APPLE_NOTARY_PRIVATE_KEY`   | base64-encoded `.p8` private key for `notarytool`              |
    | `HOMEBREW_TAP_TOKEN`         | scoped `contents:write` token on the tap repo (issue #6)       |

    All Apple credentials are optional — when absent, the workflow emits
    a `::warning::` and continues without notarization. The Homebrew tap
    job is `continue-on-error` so a missing or rejected token does not
    block the release.

    ## Home Profiles

    - Run `project-bootstrap apply home --manifest ./project.bootstrap.yaml` after reviewing the bundled profile content.
    - The bootstrap manages portable Codex and Claude assets only. Auth, sessions, caches, and machine-local state stay unmanaged.

    ## Claude Setup

    - First-party Claude web sessions should use `bash scripts/claude-cloud/setup.sh` in `claude.ai/code`.
- Interactive Claude work is prepared through `.devcontainer/devcontainer.json`.
- GitHub-hosted Claude automation lives in `.github/workflows/claude.yml` and is intentionally separate from the required PR checks.
- Finish GitHub-side auth by running `/install-github-app` in Claude Code or adding `ANTHROPIC_API_KEY` as a repo secret.
