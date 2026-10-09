"""Small trusted CLI/API boundary used by the lifecycle controller."""

from contextlib import contextmanager
from contextvars import ContextVar
import json
import os
from pathlib import Path
import signal
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request

DEADLINE = ContextVar("controller_deadline", default=None)


class OperationDeadline(BaseException):
    """Stop retry loops and release the lock before the runner's hard timeout."""


@contextmanager
def operation_budget(seconds):
    token = DEADLINE.set(time.monotonic() + seconds)
    try:
        yield
    finally:
        DEADLINE.reset(token)


def bounded_timeout(seconds):
    deadline = DEADLINE.get()
    if deadline is None:
        return seconds
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise OperationDeadline("controller operation deadline reached")
    return min(seconds, remaining)


def run(*args, data=None, timeout=900):
    timeout = bounded_timeout(timeout)
    with subprocess.Popen(
        [str(arg) for arg in args],
        stdin=subprocess.PIPE if data is not None else subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    ) as process:
        try:
            output, _ = process.communicate(data, timeout=timeout)
        except BaseException:
            # Terraform needs an opportunity to persist state/unlock. Stop the
            # whole CLI process group before releasing the lifecycle lock.
            try:
                os.killpg(process.pid, signal.SIGINT)
            except ProcessLookupError:
                pass
            try:
                process.communicate(timeout=15)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.communicate(timeout=5)
            bounded_timeout(1)
            raise
    if process.returncode:
        raise RuntimeError(
            f"{args[0]} failed (exit {process.returncode}); no private command output was logged"
        )
    return output


def github(path):
    return json.loads(run("gh", "api", path))


class Store:
    def __init__(self, bucket):
        self.bucket = bucket

    def call(self, method, path, data=None):
        token = run("gcloud", "auth", "print-access-token", timeout=30).decode().strip()
        request = urllib.request.Request(
            "https://storage.googleapis.com/" + path,
            data=data,
            method=method,
            headers={
                "Authorization": "Bearer " + token,
                "Content-Type": "application/json",
            },
        )
        with urllib.request.urlopen(request, timeout=bounded_timeout(60)) as response:
            return response.read()

    def read(self, key):
        encoded = urllib.parse.quote(key, safe="")
        try:
            meta = json.loads(
                self.call("GET", f"storage/v1/b/{self.bucket}/o/{encoded}")
            )
            data = self.call(
                "GET",
                f"storage/v1/b/{self.bucket}/o/{encoded}?alt=media&generation={meta['generation']}",
            )
            return json.loads(data), meta["generation"]
        except urllib.error.HTTPError as error:
            if error.code != 404:
                raise
            return None, "0"

    def write(self, key, value, generation):
        query = urllib.parse.urlencode(
            {"uploadType": "media", "name": key, "ifGenerationMatch": generation}
        )
        return json.loads(
            self.call(
                "POST",
                f"upload/storage/v1/b/{self.bucket}/o?{query}",
                json.dumps(value, sort_keys=True).encode(),
            )
        )["generation"]

    def mutate(self, callback):
        for _ in range(10):
            state, generation = self.read("lifecycle/state.json")
            new, result = callback(state or {"environments": {}})
            try:
                self.write("lifecycle/state.json", new, generation)
                return result
            except urllib.error.HTTPError as error:
                if error.code != 412:
                    raise
        raise RuntimeError("lifecycle state changed repeatedly; retry the operation")

    @contextmanager
    def lock(self, wait_seconds=0):
        key = "lifecycle/apply.lock"
        deadline = time.monotonic() + wait_seconds
        while True:
            try:
                generation = self.write(
                    key, {"run_id": os.environ.get("GITHUB_RUN_ID", "local")}, "0"
                )
                break
            except urllib.error.HTTPError as error:
                if error.code != 412:
                    raise
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise RuntimeError(
                        "lifecycle lock is busy; retry after its owner finishes"
                    ) from error
                time.sleep(min(5, remaining))
        try:
            yield
        finally:
            encoded = urllib.parse.quote(key, safe="")
            # Reserve a fresh bounded budget for authenticated release, even
            # after the operation exhausted its own wall-clock allowance.
            with operation_budget(90):
                self.call(
                    "DELETE",
                    f"storage/v1/b/{self.bucket}/o/{encoded}?ifGenerationMatch={generation}",
                )


def private_json(path, value):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "w") as file:
        json.dump(value, file)


class Terraform:
    def __init__(self, root, configuration):
        self.root = Path(root)
        self.configuration = configuration
        run(
            "terraform",
            f"-chdir={self.root}",
            "init",
            "-input=false",
            "-no-color",
            f"-backend-config=bucket={configuration['state_bucket']}",
            "-backend-config=prefix=terraform/environments",
        )

    def configuration_in_state(self, store):
        state, _ = store.read("terraform/environments/default.tfstate")
        if state is None:
            return {}
        value = state.get("outputs", {}).get("managed_configuration", {}).get("value")
        if value is None and state.get("resources"):
            raise RuntimeError(
                "Terraform state lacks managed_configuration; inspect partial apply before continuing"
            )
        return value or {}

    def broker_configuration(self, environments):
        # Match Terraform's optional enabled=false default in persisted intent.
        return {
            key: dict(value, enabled=value.get("enabled") or False)
            for key, value in self.configuration.get("credential_brokers", {}).items()
            if key in environments
        }

    def broker_configuration_changed(self, store, environments):
        state, _ = store.read("terraform/environments/default.tfstate")
        if state is None:
            return False
        previous = state.get("outputs", {}).get("managed_credential_brokers", {}).get("value")
        # Legacy state has no broker-intent output. Apply once to reconcile any
        # old resources and establish the durable comparison for future ticks.
        return previous != self.broker_configuration(environments)

    def apply(self, environments, retiring=(), starting=()):
        variables = {
            key: self.configuration[key]
            for key in (
                "project_id",
                "cloudflare_zone_id",
                "release_bucket",
                "deploy_service_account",
                "domain",
            )
        }
        variables["byok_project_id"] = self.configuration.get("byok_project_id", "")
        variables["environments"] = environments
        variables["credential_brokers"] = self.broker_configuration(environments)
        path = self.root / "controller.auto.tfvars.json"
        plan = self.root / "controller.tfplan"
        private_json(path, variables)
        try:
            run(
                "terraform",
                f"-chdir={self.root}",
                "plan",
                "-input=false",
                "-no-color",
                "-lock-timeout=120s",
                "-out=" + str(plan),
            )
            value = json.loads(
                run("terraform", f"-chdir={self.root}", "show", "-json", plan)
            )
            for change in value.get("resource_changes", []):
                if (
                    change["type"] == "google_compute_instance"
                    and "create" in change["change"]["actions"]
                ):
                    labels = (change["change"].get("after") or {}).get("labels", {})
                    if labels.get("environment") not in starting:
                        raise RuntimeError(
                            "plan would create/recreate a VM without a current explicit power authorization"
                        )
                if (
                    change["type"] == "google_compute_disk"
                    and "delete" in change["change"]["actions"]
                ):
                    labels = (change["change"].get("before") or {}).get("labels", {})
                    if labels.get("environment") not in retiring:
                        raise RuntimeError(
                            "plan would delete a retained disk without explicit retirement"
                        )
            run(
                "terraform",
                f"-chdir={self.root}",
                "apply",
                "-input=false",
                "-no-color",
                plan,
                timeout=1800,
            )
        finally:
            path.unlink(missing_ok=True)
            plan.unlink(missing_ok=True)

    def outputs(self):
        return json.loads(
            run("terraform", f"-chdir={self.root}", "output", "-json", "environments")
        )
