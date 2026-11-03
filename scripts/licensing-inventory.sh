#!/usr/bin/env bash
# Generate the third-party licensing inventory for every shipped workspace.
#
# Usage: scripts/licensing-inventory.sh <outdir>
#
# The output directory receives:
#   inventory-rust-{root,enterprise,desktop}.json
#   inventory-npm-{web,desktop,enterprise-web}.json
#   notices-rust-{root,enterprise,desktop}.md
#   cargo-deny-{root,enterprise,desktop}.txt
#   metadata.json and commands.txt
#
# The JSON inventories are normalised from `cargo metadata` and the npm
# package-lock files. They need no committed dependency beyond Python 3, and
# they feed `scripts/check-licensing.py attribution --inventory <outdir>`.
#
# The notice files come from cargo-about with the checked-in `about.toml` and
# `about.hbs`. The policy checks come from cargo-deny with `deny.toml` (root)
# and `enterprise/deny.toml` (Enterprise). Install them with:
#
#   cargo install --locked cargo-deny cargo-about --features cargo-about/cli
set -u

ROOT=$(cd "$(dirname "$0")/.." && pwd)
if [ "$#" -ne 1 ]; then
  echo "usage: scripts/licensing-inventory.sh <outdir>" >&2
  exit 2
fi
OUT=$1
case "$OUT" in
  /*) ;;
  *) OUT="$PWD/$OUT" ;;
esac
mkdir -p "$OUT"
cd "$ROOT" || exit 2

for tool in cargo python3 cargo-deny cargo-about; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: $tool is not installed" >&2
    echo "hint: cargo install --locked cargo-deny cargo-about --features cargo-about/cli" >&2
    exit 2
  fi
done
if ! python3 -c 'import tomllib' >/dev/null 2>&1; then
  echo "error: Python 3.11 or later is required" >&2
  exit 2
fi

: > "$OUT/commands.txt"
failures=0

record() {
  printf '%s\n' "$*" >> "$OUT/commands.txt"
}

run() {
  local log=$1
  shift
  record "$*"
  if "$@" >"$log" 2>&1; then
    echo "ok   ${log#"$OUT"/}"
  else
    echo "FAIL ${log#"$OUT"/} (see the file)" >&2
    failures=$((failures + 1))
  fi
}

# Normalise the dependency inventory. `cargo metadata` resolves workspace
# inheritance and gives every registry package a license expression. The npm
# lockfile v3 records the licence beside every installed package; reading it
# avoids a network install and a committed checker dependency. `dev` packages
# are skipped, which is what `license-checker-rseidelsohn --production` does:
# the published dashboard and desktop bundles do not ship test tooling. A
# production package without a licence is kept with `"license": null` so the
# attribution check reports it instead of silently dropping it.
if record "python3: cargo metadata and package-lock inventories" \
  && python3 - "$ROOT" "$OUT" <<'PY'
import importlib.util
import json
from datetime import datetime, timezone
from pathlib import Path
import subprocess
import sys

root = Path(sys.argv[1])
out = Path(sys.argv[2])

# One reviewed table for the inventory and the check. The checker rejects an
# exception that is not in this table or that changes its SPDX id or source.
spec = importlib.util.spec_from_file_location(
    "aidash_check_licensing", str(root / "scripts" / "check-licensing.py")
)
checker = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = checker
spec.loader.exec_module(checker)
exceptions = checker.NPM_LICENSE_EXCEPTIONS


def rust_inventory(name: str, manifest: str) -> None:
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1",
         "--manifest-path", manifest],
        cwd=root, text=True,
    ))
    packages = [
        {
            "name": package["name"],
            "version": package["version"],
            "license": package.get("license"),
            "license_file": package.get("license_file"),
            "source": package.get("source"),
        }
        for package in metadata["packages"]
    ]
    packages.sort(key=lambda package: (package["name"], package["version"]))
    write(f"inventory-rust-{name}.json", {
        "workspace": name,
        "ecosystem": "cargo",
        "manifest": manifest,
        "packages": packages,
    })


def npm_inventory(name: str, lockfile: str, note: str | None = None) -> None:
    path = root / lockfile
    if not path.is_file():
        raise SystemExit(f"error: {lockfile} is missing")
    data = json.loads(path.read_text())
    packages = []
    for key, entry in data.get("packages", {}).items():
        if not key or entry.get("link") or entry.get("dev"):
            continue
        name_part = key.rsplit("node_modules/", 1)[-1]
        if not name_part:
            continue
        license = entry.get("license")
        package = {
            "name": name_part,
            "version": entry.get("version"),
            "license": license,
            "license_source": "package-lock",
            "dev": bool(entry.get("dev")),
        }
        if not isinstance(license, str) or not license.strip():
            exception = exceptions.get((name_part, str(entry.get("version"))))
            if exception:
                package.update({
                    "license": exception["license"],
                    "license_source": "exception",
                    "license_exception": {
                        "license": exception["license"],
                        "source": exception["source"],
                        "reason": exception["reason"],
                    },
                })
        packages.append(package)
    packages.sort(key=lambda package: (package["name"], str(package["version"])))
    used_exceptions = [
        {
            "package": package["name"],
            "version": package["version"],
            **package["license_exception"],
        }
        for package in packages
        if package.get("license_source") == "exception"
    ]
    inventory = {
        "workspace": name,
        "ecosystem": "npm",
        "lockfile": lockfile,
        "license_exceptions": used_exceptions,
        "packages": packages,
    }
    if note:
        inventory["note"] = note
    write(f"inventory-npm-{name}.json", inventory)


def write(filename: str, data: dict) -> None:
    (out / filename).write_text(json.dumps(data, indent=2, sort_keys=False) + "\n")


rust_inventory("root", "Cargo.toml")
rust_inventory("enterprise", "enterprise/Cargo.toml")
rust_inventory("desktop", "desktop/src-tauri/Cargo.toml")
npm_inventory("web", "web/package-lock.json")
npm_inventory("desktop", "desktop/package-lock.json")
# enterprise/web declares no dependencies and reuses web/node_modules, so it
# shares the Community lockfile. Recording that keeps the inventory complete.
npm_inventory("enterprise-web", "web/package-lock.json",
              "enterprise/web reuses web/node_modules; no separate lockfile")

revision = subprocess.run(["git", "rev-parse", "HEAD"], cwd=root, capture_output=True,
                          text=True).stdout.strip()
dirty = bool(subprocess.run(["git", "status", "--porcelain"], cwd=root,
                            capture_output=True, text=True).stdout.strip())
tools = {}
for command in (["cargo", "--version"], ["cargo-deny", "--version"], ["cargo-about", "--version"]):
    tools[command[0]] = subprocess.run(command, cwd=root, capture_output=True,
                                       text=True).stdout.strip()
write("metadata.json", {
    "revision": revision or None,
    "dirty": dirty,
    "generated_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
    "root": str(root),
    "tools": tools,
    "workspaces": {
        "root": "Cargo.toml",
        "enterprise": "enterprise/Cargo.toml",
        "desktop": "desktop/src-tauri/Cargo.toml",
        "web": "web/package-lock.json",
        "desktop-npm": "desktop/package-lock.json",
        "enterprise-web": "web/package-lock.json",
    },
})
PY
then
  echo "ok   inventories"
else
  echo "FAIL inventories" >&2
  failures=$((failures + 1))
fi

run "$OUT/cargo-deny-root.txt" \
  cargo deny --manifest-path Cargo.toml check --config deny.toml licenses \
  --hide-inclusion-graph
run "$OUT/cargo-deny-enterprise.txt" \
  cargo deny --manifest-path enterprise/Cargo.toml check --config enterprise/deny.toml licenses \
  --hide-inclusion-graph
run "$OUT/cargo-deny-desktop.txt" \
  cargo deny --manifest-path desktop/src-tauri/Cargo.toml check --config deny.toml licenses \
  --hide-inclusion-graph

run "$OUT/cargo-about-root.txt" \
  cargo about generate --workspace --locked --manifest-path Cargo.toml \
  --config about.toml --output-file "$OUT/notices-rust-root.md" about.hbs
run "$OUT/cargo-about-enterprise.txt" \
  cargo about generate --workspace --locked --manifest-path enterprise/Cargo.toml \
  --config about.toml --output-file "$OUT/notices-rust-enterprise.md" about.hbs
run "$OUT/cargo-about-desktop.txt" \
  cargo about generate --workspace --locked --manifest-path desktop/src-tauri/Cargo.toml \
  --config about.toml --output-file "$OUT/notices-rust-desktop.md" about.hbs

if [ "$failures" -ne 0 ]; then
  echo "licensing inventory: $failures step(s) failed" >&2
  exit 1
fi
echo "licensing inventory written to $OUT"
