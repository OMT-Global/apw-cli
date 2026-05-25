# APW CLI Vision

Version: 0.1

APW CLI is a focused command-line tool for agent and operator workflows that need reliable local execution, native-host awareness, and clear security boundaries.

It should make common APW operations scriptable, inspectable, and safe to run repeatedly.

## Who It Serves

- Operators managing local APW workflows.
- Agents repairing issues, checking state, and producing PRs.
- Contributors who need a small CLI surface with strong tests and explicit host assumptions.

## Product Principles

- CLI behavior should be deterministic and easy to test.
- Native-host limitations should be reported plainly.
- Security review findings should become durable checks where possible.
- Shell portability matters on macOS.
- Every command should explain failure well enough for an agent to take the next step.

## Near-Term Direction

- Keep open PRs moving by resolving requested changes and stale labels.
- Strengthen local checks around security-sensitive paths.
- Reduce native-host ambiguity in command output and docs.
- Maintain concise issue-backed slices.

## Non-Goals

- Do not grow the CLI into an unbounded workflow framework.
- Do not rely on GNU-only shell behavior.
- Do not treat read-only diagnosis as completed issue work.
