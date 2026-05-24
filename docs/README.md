# APW documentation

Start here for the maintained APW v2 documentation set.

- [Installation and operation](INSTALLATION.md)
- [Security posture and testing](SECURITY_POSTURE_AND_TESTING.md)
- [Threat model](THREAT_MODEL.md)
- [Native migration matrix](NATIVE_MIGRATION.md)
- [Native-only redesign notes](NATIVE_ONLY_REDESIGN.md)
- [Rust migration and parity](MIGRATION_AND_PARITY.md)
- [Archive policy](ARCHIVE_POLICY.md)
- [Standalone breakout notes](STANDALONE_BREAKOUT.md)

Bootstrap and contributor environment notes live under
[`bootstrap/`](bootstrap/).

## Publication decision

APW keeps the maintained documentation as repository Markdown under `docs/`.
There is no GitHub Pages or external rendered docs site for the current
`v2.0.0` line. `project.bootstrap.yaml` intentionally keeps
`ci.workflows.pagesDeploy`, `capabilities.pages.enabled`, and
`capabilities.docsPublish.enabled` set to `false`.

This avoids duplicating normative content while the native-app broker,
notarization, distribution, and enterprise rollout docs are still changing.
When those surfaces stabilize, a future release-docs issue can revisit a
rendered site that builds from the same Markdown source.
