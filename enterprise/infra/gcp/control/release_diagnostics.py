"""Allowlisted release evidence; never print command output or arbitrary API text."""

from contextvars import ContextVar
from datetime import datetime, timezone
from functools import wraps
import json
import re
import time
from urllib.parse import urlsplit

PHASE = ContextVar("release_phase", default="startup")
ROUTES = [
    (r"repos/\{owner\}/\{repo\}", "repos/{owner}/{repo}"),
    (r"branches/[^/]+", "branches/{branch}"),
    (r"compare/[^/]+", "compare/{before}...{after}"),
    (r"actions/workflows/[^/]+/runs", "actions/workflows/{workflow}/runs"),
    (r"actions/workflows/[^/]+", "actions/workflows/{workflow}"),
    (r"actions/runs/[0-9]+/artifacts", "actions/runs/{run_id}/artifacts"),
    (r"actions/runs/[0-9]+", "actions/runs/{run_id}"),
    (r"actions/artifacts/[0-9]+/zip", "actions/artifacts/{artifact_id}/zip"),
    (r"actions/artifacts", "actions/artifacts"),
    (r"git/matching-refs/.+", "git/matching-refs/{ref}"),
    (r"git/refs", "git/refs"),
    (r"pulls/[0-9]+", "pulls/{pull_number}"),
    (r"pulls", "pulls"),
    (r"commits/[^/]+/check-runs", "commits/{sha}/check-runs"),
    (r"check-runs/[0-9]+", "check-runs/{check_id}"),
    (r"check-runs", "check-runs"),
]
VALIDATION = {
    "resource": {"PullRequest", "Reference", "CheckRun", "Repository", "GraphQL"},
    "field": {"head", "base", "title", "body", "draft", "ref", "sha", "node_id"},
    "code": {"invalid", "missing", "missing_field", "already_exists", "custom", "unprocessable", "too_large"},
    "type": {"FORBIDDEN", "NOT_FOUND", "UNPROCESSABLE", "RATE_LIMITED", "INTERNAL"},
}
EXACT_ERRORS = {"Validation Failed": "validation_failed", "Bad credentials": "bad_credentials", "Not Found": "not_found",
                "Resource not accessible by integration": "integration_permission_denied", "Internal Server Error": "server_error",
                "Service Unavailable": "service_unavailable", "Resource not accessible by personal access token": "token_permission_denied"}
PREFIX_ERRORS = {"No commits between ": "no_commits_between", "A pull request already exists": "pull_request_already_exists",
                 "API rate limit exceeded": "rate_limit_exceeded", "You have exceeded a secondary rate limit": "secondary_rate_limit",
                 "Pull request is too big": "pull_request_too_large", "Head sha can't be blank": "missing_head_sha"}
TRANSPORT_ERRORS = [(b"connection reset", "connection_reset"), (b"connection refused", "connection_refused"),
                    (b"no such host", "dns_failure"), (b"could not resolve", "dns_failure"),
                    (b"timeout", "transport_timeout"), (b"timed out", "transport_timeout"),
                    (b"certificate", "tls_failure"), (b"tls handshake", "tls_failure")]
ERROR_CATEGORIES = set(EXACT_ERRORS.values()) | set(PREFIX_ERRORS.values()) | {label for _, label in TRANSPORT_ERRORS} | {"unclassified", "invalid_response"}
INTEGER_FIELDS = {"elapsed_ms", "failure_line", "request_number", "http_status", "rate_limit_remaining", "rate_limit_reset",
                  "retry_after_seconds", "exit_code", "ci_run_id", "ci_attempt", "pull_number", "run_id", "run_attempt"}
STRING_FIELDS = {
    "method": {"GET", "POST", "PATCH", "DELETE", "other"},
    "endpoint": {label for _, label in ROUTES} | {"repos/{owner}/{repo}/" + label for _, label in ROUTES} | {"graphql", "unrecognized_route"},
    "action": {"context", "candidate", "staging", "reconcile", "attempt"},
    "reason": {"different_development_branch", "evidence_expired", "no_matching_receipt", "no_staging_receipt"},
    "error_type": {"Refused", "ValueError", "KeyError", "TypeError", "AttributeError", "BadZipFile", "RuntimeError", "CommandError",
                   "JSONDecodeError", "UnicodeDecodeError", "TimeoutExpired", "OperationDeadline", "FileNotFoundError", "PermissionError",
                   "OSError", "KeyboardInterrupt", "SystemExit"},
}


def emit(event, **fields):
    # A mistaken caller cannot turn a log field into a configuration/body dump.
    safe = {}
    for key, value in fields.items():
        if key in INTEGER_FIELDS and type(value) is int and 0 <= value < 10**20:
            safe[key] = value
        elif key == "http_status" and value is None:
            safe[key] = None
        elif key in {"token_present", "raw_error_details_withheld"} and type(value) is bool:
            safe[key] = value
        elif key in STRING_FIELDS and isinstance(value, str) and value in STRING_FIELDS[key]:
            safe[key] = value
        elif key in {"source_sha", "controller_sha"} and isinstance(value, str) and re.fullmatch(r"[a-f0-9]{40}", value):
            safe[key] = value
        elif key == "request_id" and isinstance(value, str) and re.fullmatch(r"[A-Za-z0-9:-]{1,128}", value):
            safe[key] = value
        elif key == "error_categories" and isinstance(value, list):
            safe[key] = [item for item in value[:20] if isinstance(item, str) and item in ERROR_CATEGORIES]
        elif key == "validation_errors" and isinstance(value, list):
            safe[key] = [{name: item for name, item in entry.items() if name in VALIDATION and isinstance(item, str) and item in VALIDATION[name]}
                         for entry in value[:20] if isinstance(entry, dict)]
    print(json.dumps({"component": "release", "time": datetime.now(timezone.utc).isoformat(timespec="milliseconds"),
                      "event": event, "phase": PHASE.get(), **safe}, sort_keys=True), flush=True)


def phase(function):
    @wraps(function)
    def wrapped(*args, **kwargs):
        token = PHASE.set(function.__name__)
        started = time.monotonic()
        emit("phase.started")
        try:
            result = function(*args, **kwargs)
        except BaseException as error:
            origin = error.__traceback__
            while origin is not None and origin.tb_next is not None:
                origin = origin.tb_next
            emit("phase.failed", error_type=type(error).__name__, failure_line=origin.tb_lineno if origin else None,
                 elapsed_ms=round((time.monotonic() - started) * 1000))
            raise
        else:
            emit("phase.completed", elapsed_ms=round((time.monotonic() - started) * 1000))
            return result
        finally:
            PHASE.reset(token)
    return wrapped


def endpoint(path):
    """Log route templates, never branch names, query values or signed URLs."""
    if path == "graphql":
        return "graphql"
    value = re.sub(r"^repos/[^/]+/[^/]+", "repos/{owner}/{repo}", urlsplit(path).path)
    for pattern, label in ROUTES:
        if re.fullmatch(pattern, value):
            return label
        if re.fullmatch("repos/\\{owner\\}/\\{repo\\}/" + pattern, value):
            return "repos/{owner}/{repo}/" + label
    return "unrecognized_route"


def response(output):
    """Separate gh --include headers without decoding a binary artifact body."""
    status, headers, body = None, {}, output
    while body.startswith(b"HTTP/"):
        separator = b"\r\n\r\n" if b"\r\n\r\n" in body[:65536] else b"\n\n"
        head, found, remaining = body.partition(separator)
        if not found or len(head) > 65536:
            break
        lines = head.decode("ascii", errors="replace").splitlines()
        match = re.fullmatch(r"HTTP/\S+ ([1-5][0-9]{2})(?: .*)?", lines[0])
        if not match:
            break
        status, headers, body = int(match[1]), {}, remaining
        for line in lines[1:]:
            key, _, value = line.partition(":")
            headers[key.lower()] = value.strip()
    evidence = {"http_status": status}
    request_id = headers.get("x-github-request-id", "")
    if re.fullmatch(r"[A-Za-z0-9:-]{1,128}", request_id):
        evidence["request_id"] = request_id
    for header, name in [("x-ratelimit-remaining", "rate_limit_remaining"), ("x-ratelimit-reset", "rate_limit_reset"), ("retry-after", "retry_after_seconds")]:
        if re.fullmatch(r"[0-9]{1,12}", headers.get(header, "")):
            evidence[name] = int(headers[header])
    return evidence, body


def errors(body, stderr=b""):
    """Classify known messages instead of echoing reflected request/secret values."""
    categories, validation = set(), []
    try:
        payload = json.loads(body)
    except (ValueError, UnicodeError):
        payload = None
    if isinstance(payload, dict):
        messages = [payload.get("message")]
        entries = payload.get("errors")
        for entry in entries[:20] if isinstance(entries, list) else []:
            if isinstance(entry, dict):
                messages.append(entry.get("message"))
                safe = {key: value for key, value in entry.items() if key in VALIDATION and isinstance(value, str) and value in VALIDATION[key]}
                if safe:
                    validation.append(safe)
            elif isinstance(entry, str):
                messages.append(entry)
        for message in messages:
            if not isinstance(message, str):
                continue
            if message in EXACT_ERRORS:
                categories.add(EXACT_ERRORS[message])
            for prefix, label in PREFIX_ERRORS.items():
                if message.startswith(prefix):
                    categories.add(label)
    text = stderr.lower()
    for marker, label in TRANSPORT_ERRORS:
        if marker in text:
            categories.add(label)
    return {"error_categories": sorted(categories or {"unclassified"}), "validation_errors": validation,
            "raw_error_details_withheld": True}
