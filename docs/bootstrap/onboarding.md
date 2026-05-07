    # Bootstrap Onboarding

    ## Repo Governance

    This manifest update prepares the desired GitHub governance state, but it does
    not by itself mutate live repository settings. Treat issue #17 as complete only
    after a maintainer runs `project-bootstrap apply github --manifest
    ./project.bootstrap.yaml` or otherwise verifies the equivalent GitHub settings
    are live.

    - Confirm the repository exists at `OMT-Global/apw-cli`.
    - Confirm branch protection or rulesets on `main` require one approval and code owner review.
    - Confirm branch protection points at the `CI Gate` status.
    - Confirm `delete branch on merge` and `allow auto-merge` are enabled.
    - Confirm projects and wiki are disabled in live repository settings.
    - Confirm `dev`, `stage`, and `prod` GitHub environments exist with the reviewer gates described below.

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

    ## Home Profiles

    - Run `project-bootstrap apply home --manifest ./project.bootstrap.yaml` after reviewing the bundled profile content.
    - The bootstrap manages portable Codex and Claude assets only. Auth, sessions, caches, and machine-local state stay unmanaged.

    ## Claude Setup

    - First-party Claude web sessions should use `bash scripts/claude-cloud/setup.sh` in `claude.ai/code`.
- Interactive Claude work is prepared through `.devcontainer/devcontainer.json`.
- GitHub-hosted Claude automation lives in `.github/workflows/claude.yml` and is intentionally separate from the required PR checks.
- Finish GitHub-side auth by running `/install-github-app` in Claude Code or adding `ANTHROPIC_API_KEY` as a repo secret.
