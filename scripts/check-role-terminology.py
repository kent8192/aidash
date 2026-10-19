#!/usr/bin/env python3
"""Reject "operator" outside its allowed, unrelated meanings (ADR 0044, #277).

The deployment-wide authority role is the Deployment Administrator. Text that
means whoever runs the Deployment's machines names them or the Node instead.
"operator" remains only where it means something else: SQL operators, NATS
operators, Kubernetes selector operators, search query operators, and the
private infra/delivery input directory.
"""

import re
import subprocess
import sys

# Unrelated meanings, matched within a line and removed before the check.
ALLOWED = [
    # SQL and query-builder operators.
    r"operator_class",
    r"PgOperator",
    r"regoperator",
    r"sort_operator",
    # NATS operators and their credentials.
    r"NATS operator",
    r"operator NATS",
    r"operator_url",
    r"nats_operator",
    r'user: "operator", password: "fixture-operator"',
    r"nats://operator:fixture-operator@",
    # Kubernetes and Cilium.
    r"""["']?operator["']?: ["']?(?:Exists|In|NotIn|Equal|DoesNotExist)\b""",
    r"operator\.replicas",
    r"expression\['operator'\]",
    r"key, operator, values",
    r"\}\[operator\]",
    # Search query operators of the Brave API.
    r'"operators": true',
    r"search operators",
    # The private input directory of infra/delivery.
    r"--operator-directory",
    r"operator_directory",
    r"private/delivery/operator",
    r"operator\.tfvars\.json",
    # Until #243 removes untenanted Deployment Administrator execution.
    r"OperatorConversations",
    r"OperatorConversationTransaction",
    r"NativeOperator\w*",
    r"OperatorPhase",
    r"operator_conversations?",
    r"OperatorAuthority",
    r"OperatorMedia\w*",
    r"operator_media_repository",
    r"operator_human_message_media",
    r"A fully privileged operator has already authenticated",
    r"Preserve operator preflight ordering",
    r"Operator Runs carry no subject policy snapshot",
    r"Unscoped execution is the operator path",
    r"Operator conversations use the same Store",
]

# Files whose every "operator" is unrelated.
ALLOWED_FILES = {
    # This guard and its test.
    "scripts/check-role-terminology.py",
    "scripts/test_check_role_terminology.py",
}

PATTERN = re.compile("|".join(f"(?:{p})" for p in ALLOWED))
WORD = re.compile("operator", re.IGNORECASE)


def violations(lines):
    """Yield `path:line: text` hits that keep "operator" after removing allowed uses."""
    for hit in lines:
        path, _, rest = hit.partition(":")
        if path in ALLOWED_FILES:
            continue
        _, _, text = rest.partition(":")
        if WORD.search(PATTERN.sub("", text)):
            yield hit


def paths(names):
    """Yield tracked paths that name the role."""
    for name in names:
        if name not in ALLOWED_FILES and WORD.search(PATTERN.sub("", name)):
            yield name


def main():
    grep = subprocess.run(
        ["git", "grep", "-n", "-I", "-i", "operator", "--", ".", ":!*.lock"],
        capture_output=True,
        text=True,
        check=False,
    )
    if grep.returncode not in (0, 1):
        sys.exit(grep.stderr)
    files = subprocess.run(["git", "ls-files"], capture_output=True, text=True, check=True).stdout
    found = list(violations(grep.stdout.splitlines())) + list(paths(files.splitlines()))
    if found:
        print("Name the role Deployment Administrator (ADR 0044, #277). Where the text means")
        print("whoever runs the Deployment, say so or name the Node. Unexpected uses:")
        for hit in found:
            print(f"  {hit}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
