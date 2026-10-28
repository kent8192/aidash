#!/usr/bin/env python3
"""Create a binary Git diff and prove its applied tree equals the supplied HEAD."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile


def git(*args, cwd=None):
    return subprocess.check_output(["git", *args], cwd=cwd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True)
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--base-branch", required=True)
    parser.add_argument("--output-dir", type=Path, default=Path("private"))
    args = parser.parse_args()
    root = Path(git("rev-parse", "--show-toplevel").decode().strip())
    base = git("rev-parse", f"{args.base}^{{commit}}", cwd=root).decode().strip()
    head = git("rev-parse", f"{args.head}^{{commit}}", cwd=root).decode().strip()
    expected_tree = git("rev-parse", f"{head}^{{tree}}", cwd=root).decode().strip()
    git("merge-base", "--is-ancestor", base, head, cwd=root)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    output = args.output_dir.resolve()
    patch = output / "aidash-cli-auth.patch"
    command = ["git", "diff", "--binary", "--full-index", base, head]
    data = subprocess.check_output(command, cwd=root)
    if not data:
        raise SystemExit("Refusing an empty patch")
    paths = git("diff", "--name-only", "-z", base, head, cwd=root).decode().split("\0")
    for path in filter(None, paths):
        parts = Path(path).parts
        if parts[0] in ("private", "secrets", "notes", ".ignore") or Path(path).name == ".env":
            raise SystemExit("Refusing private paths in patch")
    patch.write_bytes(data)
    with tempfile.TemporaryDirectory(prefix="aidash-patch-verify-") as parent:
        checkout = Path(parent) / "checkout"
        git("worktree", "add", "--detach", str(checkout), base, cwd=root)
        try:
            git("apply", "--check", str(patch), cwd=checkout)
            git("apply", "--index", str(patch), cwd=checkout)
            actual_tree = git("write-tree", cwd=checkout).decode().strip()
            if actual_tree != expected_tree:
                raise SystemExit("Applied Git tree does not match final HEAD")
        finally:
            # Only this script's disposable checkout, including its applied diff.
            git("worktree", "remove", "--force", str(checkout), cwd=root)
    manifest = {
        "repository": "https://github.com/kent8192/aidash",
        "pr_base_branch": args.base_branch,
        "patch_base_sha": base,
        "final_head_sha": head,
        "final_tree_sha": expected_tree,
        "patch_filename": patch.name,
        "patch_sha256": hashlib.sha256(data).hexdigest(),
        "generation_command": command,
        "application": [f"git switch --detach {base}", "git apply --check /absolute/path/aidash-cli-auth.patch", "git apply --index /absolute/path/aidash-cli-auth.patch", "git write-tree"],
        "verification": {"git_apply_check": "passed", "git_apply_index": "passed", "applied_tree_sha": actual_tree, "matches_final_head_tree": True, "temporary_worktree_removed": True},
    }
    destination = output / "aidash-cli-auth-patch-manifest.json"
    destination.write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps({"patch": str(patch), "manifest": str(destination), "sha256": manifest["patch_sha256"], "tree_verified": True}))


if __name__ == "__main__":
    main()
