#!/usr/bin/env python3
"""Run one of two disjoint libtest shards through Cargo's target runner."""

import os
from pathlib import Path
import subprocess
import sys


def main():
    shard = os.environ["AIDASH_RUST_TEST_SHARD"]
    if shard not in ("1", "2"):
        raise ValueError("Rust test shard must be 1 or 2")
    binary, *arguments = sys.argv[1:]
    listing = subprocess.check_output(
        [binary, *arguments, "--list", "--format=terse", "--color=never"], text=True,
    )
    names = sorted(
        line.rsplit(": ", 1)[0] for line in listing.splitlines()
        if line.endswith((": test", ": benchmark"))
    )
    if len(names) != len(set(names)):
        raise ValueError("libtest listed duplicate test identities")
    selected = names[int(shard) - 1::2]
    print(f"Rust shard {shard}/2: {len(selected)}/{len(names)} tests in {Path(binary).name}", flush=True)
    if not selected:
        return 0
    # Multiple exact libtest filters are ORed; parameterized cases retain their
    # full names. Tests still share the normal binary-owned service fixtures.
    os.execv(binary, [binary, *arguments, "--exact", *selected])


if __name__ == "__main__":
    sys.exit(main())
