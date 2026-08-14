# Security Policy

## Supported Surface

This repository follows the bootstrap-managed security baseline for OMT-Global/apw-cli.

## Reporting

Report suspected vulnerabilities through [GitHub private vulnerability reporting](https://github.com/OMT-Global/apw-cli/security/advisories/new). If that form is unavailable, open a public issue titled `Private security contact requested` without vulnerability details; maintainers will establish a confidential channel before accepting the report. Never include exploit details in public issues or discussions.

## Response Targets

- Acknowledge a complete report within 3 business days.
- Provide a status update within 10 business days, even when investigation is ongoing.
- Target remediation within 7 days for critical findings, 30 days for high findings, and 90 days for moderate findings. Low-severity findings are scheduled by maintainers.
- Coordinate disclosure timing with the reporter after a fix or documented mitigation is available.

## Baseline

- Dependabot policy: disabled by manifest
- Secret scanning hints: enabled
- Generated hooks and CI helpers must not require committed secrets or machine-local environment files.
