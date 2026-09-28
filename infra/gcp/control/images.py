#!/usr/bin/env python3
"""Build without cloud credentials; publish archives in a separate trusted job."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[3]
KINDS = ("app", "postgres", "sandbox", "observer")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["build", "publish"])
    parser.add_argument("--kind", choices=KINDS)
    parser.add_argument("--source", type=Path)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"[a-f0-9]{40}", args.sha):
        parser.error("full source SHA required")
    args.directory.mkdir(parents=True, exist_ok=True)
    if args.action == "build":
        current = subprocess.check_output(
            ["git", "-C", str(args.source), "rev-parse", "HEAD"], text=True
        ).strip()
        if current != args.sha:
            raise SystemExit("build checkout does not match the authorized SHA")
        if (
            args.kind == "app"
            and "GOOGLE_OIDC_ISSUER" not in (args.source / "src/config.rs").read_text()
        ):
            raise SystemExit(
                "Selected source does not contain the required Google login integration"
            )
        file, context, target = {
            "app": (ROOT / "Dockerfile", args.source, ["--target", "runtime"]),
            "postgres": (ROOT / "deploy/postgres/Dockerfile", args.source, []),
            "sandbox": (ROOT / "runner/Dockerfile", args.source / "runner", []),
            "observer": (
                ROOT / "infra/gcp/observer/Dockerfile",
                ROOT / "infra/gcp/observer",
                [],
            ),
        }[args.kind]
        tag = f"aidash-{args.kind}:{args.sha}"
        subprocess.run(
            [
                "docker",
                "build",
                "--platform",
                "linux/amd64",
                "--file",
                str(file),
                "--tag",
                tag,
                *target,
                str(context),
            ],
            check=True,
        )
        subprocess.run(
            [
                "docker",
                "save",
                "--output",
                str(args.directory / f"{args.kind}.tar"),
                tag,
            ],
            check=True,
        )
    else:
        config = json.loads(os.environ["AIDASH_GCP_CONFIG"])
        registry = "us-central1-docker.pkg.dev/" + config["project_id"] + "/aidash"
        release = {"source_sha": args.sha, "images": {}}
        for kind in KINDS:
            target = f"{registry}/{kind}:{args.sha}-{os.environ['GITHUB_RUN_ID']}"
            digest_file = args.directory / f"{kind}.digest"
            subprocess.run(
                [
                    "skopeo",
                    "copy",
                    "--digestfile",
                    str(digest_file),
                    "docker-archive:" + str(args.directory / f"{kind}.tar"),
                    "docker://" + target,
                ],
                check=True,
            )
            digest = digest_file.read_text().strip()
            if not re.fullmatch(r"sha256:[a-f0-9]{64}", digest):
                raise SystemExit("publisher returned an invalid digest")
            release["images"][kind] = f"{registry}/{kind}@{digest}"
        (args.directory / "release.json").write_text(
            json.dumps(release, indent=2) + "\n"
        )


if __name__ == "__main__":
    main()
