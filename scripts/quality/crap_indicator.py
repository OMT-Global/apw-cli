#!/usr/bin/env python3
"""Compute a lightweight CRAP-style risk indicator for Rust functions.

The real CRAP score is:

    complexity^2 * (1 - coverage)^3 + complexity

This script intentionally keeps dependencies at zero. It estimates cyclomatic
complexity from Rust source and optionally reads line coverage from an LCOV
file produced by tools such as `cargo llvm-cov --lcov`. Without LCOV input it
uses 0% coverage, which makes the report a conservative hotspot indicator.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


FN_RE = re.compile(
    r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:(?:async|const|unsafe)\s+)*(?:extern\s+"[^"]+"\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\('
)
COMPLEXITY_RE = re.compile(r"\b(if|match|for|while)\b|&&|\|\||=>")


@dataclass
class FunctionMetric:
    path: Path
    name: str
    start: int
    end: int
    complexity: int
    coverage: float

    @property
    def crap(self) -> float:
        return (self.complexity**2) * ((1.0 - self.coverage) ** 3) + self.complexity


def rust_files(paths: Iterable[Path]) -> list[Path]:
    files: list[Path] = []
    for path in paths:
        if path.is_file() and path.suffix == ".rs":
            files.append(path)
        elif path.is_dir():
            files.extend(sorted(p for p in path.rglob("*.rs") if "target" not in p.parts))
    return sorted(files)


def line_coverage(lcov: Path | None) -> dict[Path, dict[int, int]]:
    if lcov is None:
        return {}
    coverage: dict[Path, dict[int, int]] = {}
    current: Path | None = None
    for raw in lcov.read_text(encoding="utf-8").splitlines():
        if raw.startswith("SF:"):
            current = Path(raw[3:]).resolve()
            coverage.setdefault(current, {})
        elif raw.startswith("DA:") and current is not None:
            line_s, hits_s = raw[3:].split(",", 1)
            coverage[current][int(line_s)] = int(hits_s)
        elif raw == "end_of_record":
            current = None
    return coverage


def function_coverage(path: Path, start: int, end: int, coverage: dict[Path, dict[int, int]]) -> float:
    line_hits = coverage.get(path.resolve())
    if not line_hits:
        return 0.0
    executable = [line for line in range(start, end + 1) if line in line_hits]
    if not executable:
        return 0.0
    covered = sum(1 for line in executable if line_hits[line] > 0)
    return covered / len(executable)


def count_complexity(lines: list[str]) -> int:
    complexity = 1
    for line in lines:
        stripped = line.split("//", 1)[0]
        complexity += len(COMPLEXITY_RE.findall(stripped))
    return complexity


def parse_functions(path: Path, coverage: dict[Path, dict[int, int]]) -> list[FunctionMetric]:
    lines = path.read_text(encoding="utf-8").splitlines()
    metrics: list[FunctionMetric] = []
    index = 0
    while index < len(lines):
        match = FN_RE.match(lines[index])
        if not match:
            index += 1
            continue
        name = match.group(1)
        start_index = index
        body_lines: list[str] = []
        depth = 0
        seen_body = False
        while index < len(lines):
            line = lines[index]
            body_lines.append(line)
            depth += line.count("{")
            if "{" in line:
                seen_body = True
            depth -= line.count("}")
            if seen_body and depth <= 0:
                break
            index += 1
        end_index = index
        metrics.append(
            FunctionMetric(
                path=path,
                name=name,
                start=start_index + 1,
                end=end_index + 1,
                complexity=count_complexity(body_lines),
                coverage=function_coverage(path, start_index + 1, end_index + 1, coverage),
            )
        )
        index += 1
    return metrics


def render_markdown(metrics: list[FunctionMetric], limit: int) -> str:
    rows = ["| CRAP | Complexity | Coverage | Function |", "| ---: | ---: | ---: | --- |"]
    for metric in metrics[:limit]:
        rows.append(
            "| "
            f"{metric.crap:.1f} | {metric.complexity} | {metric.coverage * 100:.1f}% | "
            f"`{metric.path}:{metric.start}` `{metric.name}` |"
        )
    return "\n".join(rows)


def run(paths: list[Path], lcov: Path | None, limit: int) -> list[FunctionMetric]:
    coverage = line_coverage(lcov)
    metrics: list[FunctionMetric] = []
    for path in rust_files(paths):
        metrics.extend(parse_functions(path, coverage))
    metrics.sort(key=lambda metric: metric.crap, reverse=True)
    return metrics[:limit]


def self_test() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        source = root / "sample.rs"
        source.write_text(
            "\n".join(
                [
                    "fn simple() {",
                    "    println!(\"ok\");",
                    "}",
                    "pub(crate) const fn const_helper() -> bool {",
                    "    true",
                    "}",
                    "unsafe fn unsafe_helper(value: bool) {",
                    "    if value { println!(\"unsafe\"); }",
                    "}",
                    "pub extern \"C\" fn ffi_helper(value: bool) {",
                    "    if value { println!(\"ffi\"); }",
                    "}",
                    "fn branchy(value: bool) {",
                    "    if value && true {",
                    "        match value { true => (), false => () }",
                    "    }",
                    "}",
                ]
            ),
            encoding="utf-8",
        )
        lcov = root / "lcov.info"
        lcov.write_text(
            "\n".join(
                [
                    f"SF:{source}",
                    "DA:1,1",
                    "DA:2,1",
                    "DA:3,1",
                    "DA:4,1",
                    "DA:5,1",
                    "DA:6,1",
                    "DA:7,0",
                    "DA:8,0",
                    "DA:9,0",
                    "DA:10,0",
                    "DA:11,0",
                    "DA:12,0",
                    "DA:13,0",
                    "DA:14,0",
                    "DA:15,1",
                    "DA:16,0",
                    "DA:17,0",
                    "DA:18,1",
                    "end_of_record",
                ]
            ),
            encoding="utf-8",
        )
        metrics = run([source], lcov, 10)
        names = {metric.name for metric in metrics}
        assert {"const_helper", "unsafe_helper", "ffi_helper"}.issubset(names)
        assert metrics[0].name == "branchy"
        simple = next(metric for metric in metrics if metric.name == "simple")
        assert metrics[0].complexity > simple.complexity
        assert simple.coverage == 1.0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="*", type=Path, default=[Path("rust/src")])
    parser.add_argument("--lcov", type=Path, help="Optional LCOV file for line coverage.")
    parser.add_argument("--limit", type=int, default=20)
    parser.add_argument("--threshold", type=float, default=30.0)
    parser.add_argument("--fail-threshold", action="store_true")
    parser.add_argument("--json", action="store_true", dest="json_output")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return 0

    metrics = run(args.paths, args.lcov, args.limit)
    if args.json_output:
        print(
            json.dumps(
                [
                    {
                        "path": str(metric.path),
                        "line": metric.start,
                        "name": metric.name,
                        "complexity": metric.complexity,
                        "coverage": round(metric.coverage, 4),
                        "crap": round(metric.crap, 2),
                    }
                    for metric in metrics
                ],
                indent=2,
            )
        )
    else:
        print(render_markdown(metrics, args.limit))

    if args.fail_threshold and metrics and metrics[0].crap > args.threshold:
        print(
            f"CRAP threshold exceeded: {metrics[0].crap:.1f} > {args.threshold:.1f}",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
