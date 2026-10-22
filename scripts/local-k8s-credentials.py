"""Initialize a local kind Secret Store without replacing existing keys."""

import argparse
import base64
import json
import re
import secrets
import subprocess

KEYS = ("AIDASH_PROVIDER_FINGERPRINT_KEY", "AIDASH_PROVIDER_STORE_MASTER_KEY")


def initialize(kubectl):
    """Atomically add both keys to the app Secret, or validate the existing pair."""
    result = subprocess.run(
        [*kubectl, "get", "secret", "aidash-local-app", "-o", "json"],
        capture_output=True,
        check=False,
    )
    if result.returncode:
        raise RuntimeError("Cannot read the local app Secret; apply the local configuration first")
    secret = json.loads(result.stdout)
    data = secret.get("data", {})
    present = [key in data for key in KEYS]
    if any(present):
        if not all(present):
            raise RuntimeError("The local Secret Store key pair is incomplete; recover its original keys")
        for key in KEYS:
            try:
                value = base64.b64decode(data[key], validate=True).decode("ascii")
            except (ValueError, UnicodeError) as error:
                raise RuntimeError("The existing local Secret Store keys are invalid") from error
            if not re.fullmatch(r"[0-9a-fA-F]{64}", value):
                raise RuntimeError("The existing local Secret Store keys are invalid")
        return
    # The resource-version test prevents two initializers from replacing one
    # another's pair. A failed precondition is safe to retry from the beginning.
    patch = [{"op": "test", "path": "/metadata/resourceVersion", "value": secret["metadata"]["resourceVersion"]}]
    keys = {key: base64.b64encode(secrets.token_hex(32).encode()).decode() for key in KEYS}
    if "data" not in secret:
        patch.append({"op": "add", "path": "/data", "value": keys})
    else:
        patch.extend({"op": "add", "path": "/data/" + key, "value": value} for key, value in keys.items())
    result = subprocess.run(
        [*kubectl, "patch", "secret", "aidash-local-app", "--type=json", "--patch-file", "-"],
        input=json.dumps(patch).encode(),
        capture_output=True,
        check=False,
    )
    if result.returncode:
        raise RuntimeError("Cannot initialize the local Secret Store keys; retry k8s-up")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kubeconfig", required=True)
    args = parser.parse_args()
    initialize(["kubectl", "--kubeconfig", args.kubeconfig, "--namespace", "aidash-local"])


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, ValueError, KeyError) as error:
        raise SystemExit(str(error)) from None
