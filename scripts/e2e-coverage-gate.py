"""Measure Rust line coverage from E2E test partitions alone and enforce a floor."""

import argparse
import collections
from fnmatch import fnmatch
import os
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
PARTITION_FILE = re.compile(r"rust-(?P<partition>.+)-(?P<shard>\d+)\.lcov$")
# Same scope as codecov.yml: production Rust only.
IGNORED = ("/tests/", "/migrations/")
# `#[cfg(test)]`, `#[cfg(all(test, ...))]` and `#[cfg(any(test, ...))]` items.
# Unit-test builds instrument them, but they are test code, not production code.
TEST_ATTRIBUTE = re.compile(r"^\s*#\[cfg\((test|all\(test\b.*|any\(test\b.*)\)\]\s*$")
OUTLINE_MODULE = re.compile(r"^\s*(pub(\([^)]*\))?\s+)?mod\s+\w+\s*;")


def relative(path, root):
    """Return the repository-relative path of an LCOV source file.

    CI paths belong to another machine, so this never touches the filesystem
    (resolving /home/... on macOS waits on the automounter).
    """
    text = os.path.normpath(path)
    prefix = os.path.normpath(root) + os.sep
    if text.startswith(prefix):
        return text[len(prefix):]
    for marker in ("/crates/", "/server/"):
        index = text.rfind(marker)
        if index >= 0:
            return text[index + 1:]
    return text


def in_scope(path, excludes):
    if not (path.startswith("crates/") or path.startswith("server/src/")):
        return False
    if any(part in f"/{path}" for part in IGNORED) or path.endswith(("/tests.rs", "_tests.rs")):
        return False
    return not any(fnmatch(path, rule["glob"]) for rule in excludes)


def code_only(line):
    """Drop string and char literals and comments, so their braces do not count."""
    line = re.sub(r'"(\\.|[^"\\])*"', '""', line)
    line = re.sub(r"'(\\.|[^'\\])'", "''", line)
    return line.split("//")[0]


def test_only_lines(text):
    """Return the 1-based line numbers of items gated on cfg(test).

    Brace counting is approximate (raw strings and block comments are not
    parsed); it errs toward the item's own braces, which rustfmt keeps simple.
    """
    lines = text.splitlines()
    found = set()
    index = 0
    while index < len(lines):
        if not TEST_ATTRIBUTE.match(lines[index]):
            index += 1
            continue
        item = index + 1
        while item < len(lines) and lines[item].strip().startswith("#["):
            item += 1
        if item < len(lines) and OUTLINE_MODULE.match(lines[item]):
            # `mod tests;` lives in its own file, which the path rules remove.
            index = item + 1
            continue
        depth, opened, end = 0, False, item
        while end < len(lines):
            code = code_only(lines[end])
            depth += code.count("{") - code.count("}")
            opened = opened or "{" in code
            if (opened and depth <= 0) or (not opened and code.rstrip().endswith(";")):
                break
            end += 1
        found.update(range(index + 1, end + 2))
        index = end + 1
    return found


def component(path):
    """Group a source into its crate or server app for the report."""
    segments = path.split("/")
    if path.startswith("crates/"):
        return "/".join(segments[:2])
    if path.startswith("server/src/apps/"):
        return "/".join(segments[:4])
    return "server/src"


def read_lcov(path, root, hits):
    """Merge one LCOV file into hits[source][line] = max(count)."""
    source = None
    with open(path) as lcov:
        for line in lcov:
            if line.startswith("SF:"):
                source = relative(line[3:].strip(), root)
            elif line.startswith("DA:") and source is not None:
                number, count = line[3:].split(",")[:2]
                lines = hits[source]
                lines[int(number)] = max(lines.get(int(number), 0), int(count))
            elif line.startswith("end_of_record"):
                source = None


def measure(parts, config, root=ROOT):
    """Return (covered, total, per-component rows) for E2E partitions."""
    unit = set(config["unit_partitions"])
    excludes = config.get("exclude", [])
    every = collections.defaultdict(dict)
    e2e = collections.defaultdict(dict)
    seen = set()
    for path in sorted(Path(parts).rglob("*.lcov")):
        match = PARTITION_FILE.search(path.name)
        if not match:
            raise ValueError(f"LCOV file is not named rust-<partition>-<shard>.lcov: {path}")
        seen.add(match["partition"])
        read_lcov(path, root, every)
        if match["partition"] not in unit:
            read_lcov(path, root, e2e)
    if not seen - unit:
        raise ValueError("No E2E partition LCOV files were found")
    components = collections.defaultdict(lambda: [0, 0])
    covered = total = 0
    for source, lines in every.items():
        if not in_scope(source, excludes):
            continue
        reached = e2e.get(source, {})
        file = Path(root) / source
        if file.is_file():
            skipped = test_only_lines(file.read_text())
            lines = {number: count for number, count in lines.items() if number not in skipped}
        hit = sum(1 for number in lines if reached.get(number, 0) > 0)
        key = component(source)
        components[key][0] += hit
        components[key][1] += len(lines)
        covered += hit
        total += len(lines)
    return covered, total, sorted(components.items(), key=lambda item: item[1][0] - item[1][1])


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("parts", help="directory holding rust-<partition>-<shard>.lcov files")
    parser.add_argument("--config", default=Path(__file__).with_name("e2e-coverage.toml"))
    args = parser.parse_args(argv)
    with open(args.config, "rb") as file:
        config = tomllib.load(file)
    covered, total, components = measure(args.parts, config)
    percent = 100 * covered / total if total else 0.0
    for name, (hit, lines) in components:
        print(f"{name:45s} {hit:7d}/{lines:7d} {100 * hit / lines:6.1f}%")
    print(f"E2E-only coverage {covered}/{total} = {percent:.2f}% "
          f"(threshold {config['threshold']}%, target {config['target']}%)")
    if percent < config["threshold"]:
        print(f"E2E-only coverage fell below the {config['threshold']}% floor", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
