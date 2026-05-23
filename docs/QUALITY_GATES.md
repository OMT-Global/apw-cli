# Quality gates

APW keeps PR checks cheap, but release and readiness work should include a
deeper quality signal when code changes affect credential handling, process
execution, or operator diagnostics.

## Fast checks

Run the normal local gate before opening a PR:

```bash
bash scripts/ci/run-fast-checks.sh
cargo fmt --manifest-path rust/Cargo.toml -- --check
cargo clippy --manifest-path rust/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path rust/Cargo.toml
```

## Mutation testing

Mutation testing is opt-in because it is intentionally slower than PR Fast CI.
Install `cargo-mutants`, then run:

```bash
APW_RUN_MUTATION=1 bash scripts/ci/run-quality-indicators.sh
```

The script writes mutation output under `rust/target/mutants` and keeps it out
of source control.

## CRAP indicator

The CRAP indicator highlights functions whose estimated complexity is high
relative to test coverage:

```text
complexity^2 * (1 - coverage)^3 + complexity
```

Run the source-only hotspot report:

```bash
bash scripts/ci/run-quality-indicators.sh
```

For coverage-aware scores, install `cargo-llvm-cov` and run:

```bash
APW_RUN_COVERAGE=1 bash scripts/ci/run-quality-indicators.sh
```

Without LCOV coverage input, the script assumes 0% coverage and produces a
conservative hotspot list. That mode is useful for deciding where to add tests
before enabling coverage tooling on a runner.
