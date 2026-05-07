#!/usr/bin/env python3
"""Check repo-local bootstrap governance invariants for issue #17.

This dependency-free check keeps the lightweight PR CI lane aligned with the
GitHub governance contract encoded in project.bootstrap.yaml until a maintainer
applies that contract with project-bootstrap.
"""
from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "project.bootstrap.yaml"
PR_FAST_CI = ROOT / ".github" / "workflows" / "pr-fast-ci.yml"
ONBOARDING = ROOT / "docs" / "bootstrap" / "onboarding.md"


def fail(message: str) -> None:
    print(f"FAIL bootstrap governance: {message}", file=sys.stderr)
    raise SystemExit(1)


def require(pattern: str, text: str, label: str, flags: int = re.MULTILINE) -> None:
    if not re.search(pattern, text, flags):
        fail(f"missing {label}")


def main() -> int:
    manifest = MANIFEST.read_text(encoding="utf-8")
    pr_fast_ci = PR_FAST_CI.read_text(encoding="utf-8")
    onboarding = ONBOARDING.read_text(encoding="utf-8")

    require(r"^\s*deleteBranchOnMerge:\s*true\s*$", manifest, "deleteBranchOnMerge: true")
    require(r"^\s*autoMerge:\s*true\s*$", manifest, "autoMerge: true")
    require(r"^\s*requiredApprovals:\s*1\s*$", manifest, "requiredApprovals: 1")
    require(r"^\s*dismissStaleReviews:\s*true\s*$", manifest, "dismissStaleReviews: true")
    require(r"^\s*requireCodeOwnerReviews:\s*true\s*$", manifest, "requireCodeOwnerReviews: true")
    require(r"requiredStatusChecks:\s*\n\s*- CI Gate\s*$", manifest, "required CI Gate status check")
    require(r"repoFeatures:[\s\S]*?hasProjects:\s*false[\s\S]*?hasWiki:\s*false", manifest, "projects/wiki disabled")
    require(r"security:[\s\S]*?secretScanningHints:\s*true", manifest, "security scanning hints enabled")

    for environment in ("dev", "stage", "prod"):
        require(rf"^\s{{2}}{environment}:\s*$", manifest, f"{environment} environment")

    require(r"^\s*name:\s*CI Gate\s*$", pr_fast_ci, "CI Gate job name in PR Fast CI")
    require(
        r"Treat issue #17 as complete only\s+after a maintainer runs `project-bootstrap apply github --manifest\s+\./project\.bootstrap\.yaml`",
        onboarding,
        "maintainer apply instruction for issue #17",
        re.MULTILINE | re.DOTALL,
    )

    print("Bootstrap governance invariants passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
