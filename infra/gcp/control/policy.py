"""Pure lifecycle policy. No cloud calls, shell interpretation, or application writes."""

from copy import deepcopy
import re

SHA = re.compile(r"[0-9a-f]{40}")
ENVIRONMENT = re.compile(r"(?:develop|test|pr-[1-9][0-9]*)")
DEVELOP = re.compile(r"develop/[0-9]+\.[0-9]+\.[0-9]+")
PREVIEW = re.compile(
    r"/preview (up|stop|destroy)(?: (spot|normal))?(?: ([0-9a-f]{40}))?"
)


class Refused(ValueError):
    """Invalid, stale, or unauthorized lifecycle request."""


def preview_command(body):
    match = PREVIEW.fullmatch(body.strip())
    if not match:
        return None
    action, mode, sha = match.groups()
    if action != "up" and (mode or sha):
        raise Refused("Only /preview up accepts a VM mode or an approved SHA")
    return {"action": action, "mode": mode or "", "approved_sha": sha or ""}


def eligible_base(branch):
    return branch == "main" or bool(DEVELOP.fullmatch(branch))


def validate_request(request):
    if not ENVIRONMENT.fullmatch(request["environment"]):
        raise Refused(
            "Only develop, test, or pr-N environments exist; production is deferred"
        )
    if request["action"] not in {"create", "up", "resume", "stop", "destroy", "update"}:
        raise Refused("Unknown lifecycle action")
    if request.get("sha") and not SHA.fullmatch(request["sha"]):
        raise Refused("Source must resolve to an immutable full SHA")
    if request.get("mode", "") not in {"", "spot", "normal"}:
        raise Refused("Unknown provisioning model")
    if request["environment"] == "develop" and request.get("mode") == "spot":
        raise Refused("Shared development staging uses a normal VM")
    if request["action"] in {"create", "up", "resume", "update"} and not request.get(
        "sha"
    ):
        raise Refused("An authorized, CI-successful source is required")
    if request.get("fork") and request.get("approved_sha") != request.get("sha"):
        raise Refused("Fork deployment requires an explicit approval of this exact SHA")


def transition(state, request, incarnation, now):
    """Accept intent under the lifecycle lock, before building a release.

    Sequence is a trusted GitHub run ID, not an issue-comment field. Tombstones
    retain it so old reruns cannot resurrect a retired environment.
    """
    validate_request(request)
    result = deepcopy(state)
    envs = result.setdefault("environments", {})
    identity = request["environment"]
    previous = envs.get(identity)
    sequence = int(request["sequence"])
    if previous and sequence <= previous["sequence"]:
        return result, False
    action = request["action"]
    exists = previous and previous["desired"] != "destroyed"
    if action == "stop" and not exists:
        # Fence an older create even if stop arrived first. An absent stop must
        # not invent an incomplete environment eligible for automatic updates.
        action = "destroy"
    if (
        action in {"create", "up"}
        and previous
        and not exists
        and previous.get("status") != "destroyed"
    ):
        raise Refused(
            "Retirement is still in progress; retry after owned resources are removed"
        )
    if action == "update":
        if not exists or identity == "test":
            return result, False
        if previous["source_ref"] != request["source_ref"]:
            return result, False
        if previous.get("fork") or request.get("fork"):
            return result, False
        if previous["sha"] == request["sha"]:
            return result, False
    if action == "resume" and not exists:
        raise Refused("The environment does not exist; use create")
    if action == "create" and exists:
        if (
            previous["sha"] != request["sha"]
            or previous["source_ref"] != request["source_ref"]
        ):
            raise Refused(
                "Create cannot replace retained data; use an explicit resume with a source or destroy"
            )
        return result, False
    kind = "pr" if identity.startswith("pr-") else identity
    entry = (
        deepcopy(previous)
        if exists
        else {
            "kind": kind,
            "incarnation": incarnation,
            "desired": "stopped",
            "spot": kind != "develop",
            "applied": None,
            "release": None,
            "status": "absent",
            "published": False,
        }
    )
    entry.update(
        sequence=sequence,
        generation=(previous or {}).get("generation", 0) + 1,
        force=bool(request.get("force")),
        requested_at=now,
        last_action=action,
    )
    if action in {"create", "up", "resume", "update"}:
        if kind == "develop" and not DEVELOP.fullmatch(request["source_ref"]):
            raise Refused("Development staging must follow develop/x.y.z")
        if exists and kind != "test" and entry["source_ref"] != request["source_ref"]:
            raise Refused(
                "Retargeting a retained staging environment requires destroy/create"
            )
        if entry.get("sha") != request["sha"]:
            entry["release"] = None
        entry.update(
            {
                key: request.get(key, "")
                for key in ("sha", "source_ref", "source_repo", "fork", "approved_sha")
            }
        )
        if action != "update":
            entry["desired"] = "running"
            entry["start_pending"] = True
            entry["keepalive_at"] = now
            if request.get("mode"):
                entry["spot"] = request["mode"] == "spot"
        # An update records a pending source even while stopped, but never wakes it.
    else:
        entry["desired"] = "destroyed" if action == "destroy" else "stopped"
        entry["start_pending"] = False
    entry["status"] = "requested"
    envs[identity] = entry
    # A second PR waits. Do not replace another PR or erase its retained data.
    if kind == "pr" and entry["desired"] == "running":
        owners = [
            key
            for key, value in envs.items()
            if key != identity
            and value["kind"] == "pr"
            and (value.get("applied") or {}).get("running")
        ]
        if owners:
            entry["status"] = "waiting_for_pr_slot"
    return result, True


def build_needed(entry):
    return entry["desired"] == "running" and not entry.get("release")


def attach_release(state, identity, generation, release):
    result = deepcopy(state)
    entry = result["environments"].get(identity)
    if not entry or entry["generation"] != generation or entry["desired"] != "running":
        return result, False
    if release["source_sha"] != entry["sha"]:
        raise Refused("Build source does not match current authorized intent")
    for name in ("app", "postgres", "sandbox", "observer"):
        if not re.fullmatch(
            r"us-central1-docker\.pkg\.dev/[a-z0-9-]+/aidash/[a-z0-9-]+@sha256:[a-f0-9]{64}",
            release["images"].get(name, ""),
        ):
            raise Refused(
                "Every deployed image must have a digest in the private registry"
            )
    entry["release"] = release
    return result, True


def meaningful_request(method, path, status):
    # The access log contains $uri, never a query string, cookie, or request body.
    if not 200 <= status < 400:
        return False
    if method == "GET":
        return path in {"/auth/login", "/auth/callback"}
    if method not in {"POST", "PUT", "PATCH", "DELETE"} or not path.startswith(
        ("/api/", "/federation/v0.1/")
    ):
        return False
    if re.fullmatch(r"/api/runs/[^/]+/(shell|python)/poll", path):
        return False
    if path.startswith(("/api/peer/", "/api/metrics", "/api/state", "/api/session")):
        return False
    # The peer protocol also uses POST for discovery, verification and polling.
    if re.fullmatch(
        r"/federation/v0\.1/(discover|workspace|scoped/discover|scoped/registry/verify|"
        r"scoped/execution/(inspect|status|grants/(verify|snapshot|describe|activation)|"
        r"admissions/[^/]+/verify)|scoped/files/(negotiate|describe|status|recipients))",
        path,
    ):
        return False
    return True


def idle_due(observation, now, keepalive_at=0):
    if observation.get("protocol") != "aidash-infra-activity/1":
        raise Refused("Unknown activity protocol")
    if not 0 <= now - observation["observed_at"] <= 120:
        raise Refused("Activity observation is stale")
    return (
        not observation["busy"]
        and now - max(observation["last_active"], keepalive_at) >= 3600
    )
