#!/usr/bin/env python3
"""Reject Cloud-only names from the Community OpenAPI description (#254).

The Community build runs only the self-hosted Offering, so its description must
not name a Cloud-only feature. Match the denylist below against every path and
every tag, including the tags attached to operations.
"""
import json
from pathlib import Path
import re
import sys

# GCIP, the credential broker, billing, Secret Manager and Cloud KMS exist only
# in the Enterprise build (ADR 0032).
CLOUD_ONLY = re.compile(r"gcip|broker|billing|secret[-_]manager|kms", re.IGNORECASE)


def tags(document):
    """Yield every tag name in the description."""
    for tag in document.get("tags", []):
        if isinstance(tag, dict):
            yield tag.get("name", "")
        else:
            yield str(tag)
    for operations in document.get("paths", {}).values():
        for operation in operations.values():
            if isinstance(operation, dict):
                for tag in operation.get("tags", []):
                    yield str(tag)


def violations(document):
    """Return a message for each Cloud-only path or tag."""
    problems = [
        f"Cloud-only path: {path}"
        for path in document.get("paths", {})
        if CLOUD_ONLY.search(path)
    ]
    problems.extend(
        f"Cloud-only tag: {tag}" for tag in tags(document) if CLOUD_ONLY.search(tag)
    )
    return problems


def main(arguments):
    if len(arguments) != 1:
        print("usage: check-community-openapi.py DESCRIPTION", file=sys.stderr)
        return 2
    try:
        document = json.loads(Path(arguments[0]).read_text())
    except (OSError, ValueError) as error:
        print(f"{arguments[0]}: unreadable OpenAPI description ({error})", file=sys.stderr)
        return 2
    problems = violations(document)
    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
