#!/usr/bin/env python3
"""Deliver trusted immutable deployment receipts through Cloudflare Email Sending.

Receipts contain no user-authored text. This process cannot deploy, read runtime
secrets, or change recipients. Its only writes are CAS delivery acknowledgements
in a dedicated bucket's notification-delivery/ prefix, never lifecycle state.

Cloudflare assigns Message-ID and does not document a send idempotency key. CAS
prevents concurrent sends and repeats after acknowledgement. A lost acceptance
response or crash before acknowledgement can still duplicate mail after backoff;
X-Aidash-Event-ID remains stable for tracing that case. Accepted/queued does not
prove delivery to the forwarding destination.
"""

import argparse
import json
import os
import re
import ssl
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import UTC, datetime
from pathlib import Path

from cloud import Store, github

PROTOCOL = "aidash-deployment-notification/1"
REPOSITORY = "kent8192/aidash"
SENDER = "deployment.noreply@aidash.run"
RECIPIENTS = {
    "develop": "stg.deployment@aidash.run",
    "production": "prod.deployment@aidash.run",
    "test": "test.deployment@aidash.run",
    "pr": "test.deployment@aidash.run",
}
WORKFLOWS = {
    ".github/workflows/gcp-environments.yml": "GCP environments",
    ".github/workflows/gcp-production.yml": "GCP production",
}
EVENT_ID = re.compile(r"[a-f0-9]{64}")
IMAGE_KINDS = {
    "app",
    "postgres",
    "sandbox",
    "observer",
    "nats",
    "control",
    "edge",
    "caddy",
}
LEASE_SECONDS = 600
MAX_ATTEMPTS = 8
MAX_BATCH = 20
MAX_RESPONSE = 65536


class Refused(ValueError):
    """A safe explanation that cannot include attacker-supplied content."""


class SendFailure(Exception):
    def __init__(self, retryable, delay=300):
        super().__init__("email transport did not acknowledge the fixed recipient")
        self.retryable = retryable
        self.delay = delay


def matches(pattern, value):
    return isinstance(value, str) and re.fullmatch(pattern, value) is not None


def validate_receipt(receipt, configuration, event_id=None):
    required = {
        "protocol",
        "event_id",
        "environment",
        "incarnation",
        "generation",
        "source_sha",
        "trusted_sha",
        "status",
        "hostname",
        "run_id",
        "occurred_at",
    }
    if (
        not isinstance(receipt, dict)
        or not required <= receipt.keys()
        or receipt.keys() - required - {"images"}
    ):
        raise Refused("invalid notification receipt fields")
    if receipt["protocol"] != PROTOCOL or not matches(EVENT_ID, receipt["event_id"]):
        raise Refused("invalid notification receipt protocol or identity")
    if event_id is not None and receipt["event_id"] != event_id:
        raise Refused("notification receipt key does not match its identity")
    identity = receipt["environment"]
    if not matches(r"develop|production|test|pr-[1-9][0-9]{0,9}", identity):
        raise Refused("invalid notification environment")
    # A production reader must not route nonproduction receipts (or vice versa).
    scope = configuration.get("deployment_tier", "nonproduction")
    if scope not in {"nonproduction", "production"} or (identity == "production") != (
        scope == "production"
    ):
        raise Refused("notification environment is outside this bucket's scope")
    hostname = receipt["hostname"]
    if identity == "production":
        origin = configuration.get("production_origin")
        if (
            not isinstance(origin, str)
            or not matches(
                r"https://(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)*aidash\.run",
                origin,
            )
            or hostname != origin.removeprefix("https://")
        ):
            raise Refused(
                "production notification hostname is not explicitly configured"
            )
    elif identity == "test":
        if hostname != "test.aidash.run":
            raise Refused("invalid test notification hostname")
    elif not matches(r"[a-f0-9]{32}\.preview\.aidash\.run", hostname):
        raise Refused("invalid controller-issued preview hostname")
    for field, pattern in (
        ("incarnation", r"[a-f0-9]{12}"),
        ("source_sha", r"[a-f0-9]{40}"),
        ("trusted_sha", r"[a-f0-9]{40}"),
        ("run_id", r"[1-9][0-9]{0,19}"),
    ):
        if not matches(pattern, receipt[field]):
            raise Refused("invalid deployment receipt revision")
    if type(receipt["generation"]) is not int or not 0 < receipt["generation"] < 2**63:
        raise Refused("invalid deployment receipt generation")
    if (
        type(receipt["occurred_at"]) is not int
        or not 0 < receipt["occurred_at"] <= int(time.time()) + 300
    ):
        raise Refused("invalid deployment receipt timestamp")
    if receipt["status"] not in {"ready", "failed"}:
        raise Refused("only confirmed deployment results may be notified")
    images = receipt.get("images", {})
    if not isinstance(images, dict) or images.keys() - IMAGE_KINDS:
        raise Refused("invalid deployment image inventory")
    for kind, reference in images.items():
        if not matches(
            r"us-central1-docker\.pkg\.dev/[a-z][a-z0-9-]{4,61}[a-z0-9]/aidash/"
            + kind
            + r"@sha256:[a-f0-9]{64}",
            reference,
        ):
            raise Refused("deployment notification requires immutable image references")
    return receipt


def message(receipt, configuration):
    receipt = validate_receipt(receipt, configuration)
    identity = receipt["environment"]
    recipient = RECIPIENTS["pr" if identity.startswith("pr-") else identity]
    label = "staging" if identity == "develop" else identity
    status = (
        "deployment complete" if receipt["status"] == "ready" else "deployment failed"
    )
    occurred = datetime.fromtimestamp(receipt["occurred_at"], UTC).isoformat()
    lines = [
        f"Aidash {label}: {status}",
        f"Environment: {identity}",
        f"Source SHA: {receipt['source_sha']}",
        f"Trusted controller SHA: {receipt['trusted_sha']}",
        f"Incarnation / generation: {receipt['incarnation']} / {receipt['generation']}",
        f"Occurred at: {occurred}",
    ]
    if receipt["status"] == "ready":
        lines.append(f"URL: https://{receipt['hostname']}")
    lines.extend(
        [
            f"Actions: https://github.com/{REPOSITORY}/actions/runs/{receipt['run_id']}",
            f"Event ID: {receipt['event_id']}",
        ]
    )
    for kind, reference in sorted(receipt.get("images", {}).items()):
        lines.append(f"Image {kind}: {reference}")
    return {
        "from": SENDER,
        # Cloudflare Email Sending defines `to` as an array of addresses.
        "to": [recipient],
        "subject": f"[Aidash {label}] {status} ({receipt['source_sha'][:12]})",
        "text": "\n".join(lines) + "\n",
        "headers": {
            "Auto-Submitted": "auto-generated",
            "X-Aidash-Event-ID": receipt["event_id"],
        },
    }


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, newurl):
        # Never forward the API credential through an unexpected redirect.
        return None


class CloudflareSender:
    def __init__(self, account_id, token):
        if not matches(r"[a-f0-9]{32}", account_id) or not matches(
            r"[A-Za-z0-9_-]{20,256}", token
        ):
            raise Refused(
                "configure a scoped Cloudflare Email Sending account and token"
            )
        self.endpoint = f"https://api.cloudflare.com/client/v4/accounts/{account_id}/email/sending/send"
        self.token = token
        self.opener = urllib.request.build_opener(
            urllib.request.HTTPSHandler(context=ssl.create_default_context()),
            NoRedirect(),
        )

    def send(self, payload):
        # Keep routing fixed even if a future caller bypasses message().
        if not isinstance(payload, dict) or payload.keys() != {
            "from",
            "to",
            "subject",
            "text",
            "headers",
        }:
            raise Refused("invalid deployment mail fields")
        recipients = payload["to"]
        if (
            payload["from"] != SENDER
            or not isinstance(recipients, list)
            or len(recipients) != 1
            or recipients[0] not in RECIPIENTS.values()
        ):
            raise Refused("deployment mail requires the fixed sender and recipient")
        if not matches(
            r"\[Aidash (?:staging|production|test|pr-[1-9][0-9]{0,9})\] deployment (?:complete|failed) \([a-f0-9]{12}\)",
            payload["subject"],
        ):
            raise Refused("invalid deployment mail subject")
        if (
            not isinstance(payload["text"], str)
            or not 0 < len(payload["text"]) <= 16000
            or "\r" in payload["text"]
            or "\x00" in payload["text"]
        ):
            raise Refused("invalid deployment mail body")
        headers = payload["headers"]
        if (
            not isinstance(headers, dict)
            or headers.keys() != {"Auto-Submitted", "X-Aidash-Event-ID"}
            or headers["Auto-Submitted"] != "auto-generated"
            or not matches(EVENT_ID, headers["X-Aidash-Event-ID"])
        ):
            raise Refused("invalid deployment mail tracking headers")
        request = urllib.request.Request(
            self.endpoint,
            data=json.dumps(payload).encode(),
            method="POST",
            headers={
                "Authorization": "Bearer " + self.token,
                "Content-Type": "application/json",
            },
        )
        try:
            with self.opener.open(request, timeout=30) as response:
                raw = response.read(MAX_RESPONSE + 1)
            if len(raw) > MAX_RESPONSE:
                raise SendFailure(True)
            value = json.loads(raw)
        except urllib.error.HTTPError as error:
            retry_after = error.headers.get("Retry-After", "") if error.headers else ""
            delay = (
                max(300, min(int(retry_after), 86400))
                if matches(r"[0-9]{1,8}", retry_after)
                else 300
            )
            raise SendFailure(error.code == 429 or error.code >= 500, delay) from None
        except (urllib.error.URLError, TimeoutError, OSError, ValueError):
            # Acceptance may have happened before a lost response. Delay, do not
            # loop within a run; a later notifier attempt may duplicate mail.
            raise SendFailure(True) from None
        if (
            not isinstance(value, dict)
            or value.get("success") is not True
            or not isinstance(value.get("result"), dict)
        ):
            raise SendFailure(True)
        result, recipient = value["result"], recipients[0]
        groups = {}
        for field in (
            "delivered",
            "queued",
            "permanent_bounces",
            "suppressed_recipients",
        ):
            group = result.get(field, [])
            if not isinstance(group, list) or any(item != recipient for item in group):
                raise SendFailure(True)
            groups[field] = group
        if groups["permanent_bounces"] or groups["suppressed_recipients"]:
            raise SendFailure(False)
        if not groups["delivered"] and not groups["queued"]:
            raise SendFailure(True)
        return "delivered" if groups["delivered"] else "accepted"


def receipt_keys(store):
    page, seen, keys = "", set(), set()
    while True:
        query = urllib.parse.urlencode(
            {"prefix": "notifications/", "maxResults": 1000, "pageToken": page}
        )
        value = json.loads(store.call("GET", f"storage/v1/b/{store.bucket}/o?{query}"))
        for item in value.get("items", []):
            key = item.get("name", "")
            if not matches(r"notifications/[a-f0-9]{64}\.json", key):
                raise Refused("invalid object in the notification receipt prefix")
            keys.add(key)
        page = value.get("nextPageToken", "")
        if not page:
            return sorted(keys)
        if not isinstance(page, str) or page in seen or len(seen) >= 100:
            raise Refused("notification receipt inventory did not advance")
        seen.add(page)


def claim(store, event_id, now, retry_blocked=False):
    key = f"notification-delivery/{event_id}.json"
    for _ in range(3):
        previous, generation = store.read(key)
        previous = previous or {}
        if previous.get("status") in {"accepted", "delivered"}:
            return None
        if previous.get("status") == "blocked" and not retry_blocked:
            return None
        if (
            previous.get("status") == "inflight"
            and previous.get("lease_until", 0) > now
        ):
            return None
        if previous.get("next_attempt_at", 0) > now:
            return None
        attempts = (
            1
            if retry_blocked and previous.get("status") == "blocked"
            else previous.get("attempts", 0) + 1
        )
        value = {
            "status": "inflight",
            "lease_until": now + LEASE_SECONDS,
            "attempts": attempts,
        }
        try:
            return store.write(key, value, generation), value
        except urllib.error.HTTPError as error:
            if error.code != 412:
                raise
    return None


def drain(store, configuration, sender, retry_blocked=False):
    counts = {"accepted": 0, "delivered": 0, "retry": 0, "blocked": 0, "skipped": 0}
    attempts = 0
    for key in receipt_keys(store):
        event_id = key.removeprefix("notifications/").removesuffix(".json")
        # Acknowledged historical entries need no receipt fetch or validation.
        status, _ = store.read(f"notification-delivery/{event_id}.json")
        if status and status.get("status") in {"accepted", "delivered"}:
            counts["skipped"] += 1
            continue
        receipt, _ = store.read(key)
        validate_receipt(receipt, configuration, event_id)
        now = int(time.time())
        owned = claim(store, event_id, now, retry_blocked)
        if owned is None:
            counts["skipped"] += 1
            continue
        generation, state = owned
        try:
            result = sender.send(message(receipt, configuration))
            value = {
                "status": result,
                "accepted_at": now,
                "attempts": state["attempts"],
            }
        except SendFailure as error:
            result = (
                "retry"
                if error.retryable and state["attempts"] < MAX_ATTEMPTS
                else "blocked"
            )
            value = {"status": result, "attempts": state["attempts"]}
            if result == "retry":
                value["next_attempt_at"] = now + max(
                    error.delay, min(300 * 2 ** (state["attempts"] - 1), 21600)
                )
        store.write(f"notification-delivery/{event_id}.json", value, generation)
        counts[result] += 1
        attempts += 1
        if attempts >= MAX_BATCH:
            break
    return counts


def preflight(event_name, event):
    if (
        os.environ.get("GITHUB_REPOSITORY") != REPOSITORY
        or event.get("repository", {}).get("full_name") != REPOSITORY
    ):
        raise Refused(
            "deployment notifications are restricted to the configured repository"
        )
    if event_name == "workflow_run":
        workflow_run = event.get("workflow_run", {})
        workflow_id = workflow_run.get("workflow_id")
        if (
            type(workflow_id) is not int
            or workflow_id <= 0
            or workflow_run.get("status") != "completed"
            or workflow_run.get("head_repository", {}).get("full_name") != REPOSITORY
        ):
            raise Refused(
                "notification trigger must be a completed trusted deployment workflow"
            )
        workflow = github(f"repos/{REPOSITORY}/actions/workflows/{workflow_id}")
        if WORKFLOWS.get(workflow.get("path")) != workflow.get("name"):
            raise Refused("notification trigger is not an approved deployment workflow")
    elif event_name == "workflow_dispatch":
        actor = os.environ.get("GITHUB_ACTOR", "")
        if not matches(r"[A-Za-z0-9][A-Za-z0-9-]{0,38}", actor):
            raise Refused("invalid notification operator")
        permission = github(f"repos/{REPOSITORY}/collaborators/{actor}/permission")
        if permission.get("permission") not in {"admin", "maintain", "write"}:
            raise Refused("notification retry requires repository write permission")
    elif event_name != "schedule":
        raise Refused("unsupported deployment notification trigger")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["preflight", "deliver"])
    parser.add_argument("--retry-blocked", action="store_true")
    args = parser.parse_args()
    if args.command == "preflight":
        preflight(
            os.environ["GITHUB_EVENT_NAME"],
            json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text()),
        )
        return 0
    configuration = json.loads(os.environ["AIDASH_GCP_CONFIG"])
    if configuration.get("repository") != REPOSITORY or not matches(
        r"[a-z0-9][a-z0-9._-]{1,220}[a-z0-9]", configuration.get("notification_bucket")
    ):
        raise Refused("configure the dedicated notification bucket")
    sender = CloudflareSender(
        os.environ.get("CLOUDFLARE_ACCOUNT_ID", ""),
        os.environ.get("CLOUDFLARE_EMAIL_SENDING_TOKEN", ""),
    )
    counts = drain(
        Store(configuration["notification_bucket"]),
        configuration,
        sender,
        args.retry_blocked,
    )
    # Counts only: no token, account, random URL, receipt contents or API error.
    print(json.dumps(counts, sort_keys=True))
    return 1 if counts["retry"] or counts["blocked"] else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception:  # noqa: BLE001 - redact all private provider/receipt data at the CLI boundary
        print(
            "Deployment notification processing failed; inspect its protected configuration or delivery acknowledgements.",
            file=sys.stderr,
        )
        sys.exit(1)
