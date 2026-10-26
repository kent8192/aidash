"""Adopt Email Service metadata and provision a Sending token without Terraform secrets.

No domain onboarding, DNS updates, verification mail or email sends exist here.
Token creation requires apply=True; its value goes directly from memory to gh's
stdin, never a CLI argument, output, journal, plan, state or local file.
"""

import argparse
import json
import os
import re
import ssl
import stat
import subprocess
import urllib.error
import urllib.parse
import urllib.request
from datetime import UTC, datetime, timedelta
from pathlib import Path

from setup import Refused, private_input, private_json, verify_environment

REPOSITORY = "kent8192/aidash"
ENVIRONMENT = "deployment-notifications"
SECRET = "CLOUDFLARE_EMAIL_SENDING_TOKEN"
TOKEN_NAME = "aidash-deployment-notifications"
PERMISSION_NAMES = {"Email Sending Write", "Email Sending Edit", "Email Sending: Edit"}
ALIASES = {
    "stg": "stg.deployment@aidash.run",
    "prod": "prod.deployment@aidash.run",
    "test": "test.deployment@aidash.run",
}


class SetupError(RuntimeError):
    def __init__(self, message, receipt=None):
        super().__init__(message)
        self.receipt = receipt or {}


def identifier(value):
    if not isinstance(value, str) or not re.fullmatch(r"[a-f0-9]{32}", value):
        raise SetupError("Supply an exact 32-character Cloudflare resource identifier.")
    return value


def bootstrap_token(private_file=None):
    if private_file is None:
        token = os.environ.get("CLOUDFLARE_API_TOKEN", "")
    else:
        descriptor = os.open(Path(private_file), os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(descriptor) as source:
            metadata = os.fstat(source.fileno())
            if (
                not stat.S_ISREG(metadata.st_mode)
                or metadata.st_uid != os.getuid()
                or stat.S_IMODE(metadata.st_mode) & 0o077
            ):
                raise SetupError(
                    "Bootstrap credential file must be private and owned by the current user."
                )
            token = source.read(4097).strip()
    if not re.fullmatch(r"[A-Za-z0-9_-]{20,256}", token):
        raise SetupError(
            "Provide a scoped Cloudflare bootstrap API token through the environment or a private file."
        )
    return token


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, newurl):
        return None


class CloudflareAPI:
    def __init__(self, token):
        if not isinstance(token, str) or not re.fullmatch(
            r"[A-Za-z0-9_-]{20,256}", token
        ):
            raise SetupError("A private scoped Cloudflare bootstrap token is required.")
        self.token = token
        self.opener = urllib.request.build_opener(
            urllib.request.HTTPSHandler(context=ssl.create_default_context()),
            NoRedirect(),
        )

    def call(self, method, path, data=None):
        if method not in {"GET", "POST", "DELETE"} or not re.fullmatch(
            r"(?:accounts|zones)/[a-f0-9]{32}(?:/[a-z0-9_/]+)?(?:\?[A-Za-z0-9%_=&.+-]+)?",
            path,
        ):
            raise SetupError("Invalid fixed Cloudflare API operation.")
        request = urllib.request.Request(
            "https://api.cloudflare.com/client/v4/" + path,
            method=method,
            data=None if data is None else json.dumps(data).encode(),
            headers={
                "Authorization": "Bearer " + self.token,
                "Content-Type": "application/json",
            },
        )
        try:
            with self.opener.open(request, timeout=30) as response:
                raw = response.read(1048577)
            if len(raw) > 1048576:
                raise SetupError(
                    "Cloudflare response exceeded the protected size limit."
                )
            value = json.loads(raw)
            if not isinstance(value, dict) or value.get("success") is not True:
                raise SetupError(
                    "Cloudflare did not acknowledge the protected operation."
                )
            return value
        except (urllib.error.URLError, OSError, ValueError):
            raise SetupError(
                "Cloudflare operation failed; no private provider output was logged."
            ) from None

    def inventory(self, path, max_pages=20):
        """Read every page of a Cloudflare list; refuse a partial or inconsistent one."""
        base, _, query = path.partition("?")
        parameters = dict(urllib.parse.parse_qsl(query))
        items, seen, page_path = [], set(), path
        for page in range(1, max_pages + 1):
            value = self.call("GET", page_path)
            result, info = value.get("result"), value.get("result_info") or {}
            if not isinstance(result, list) or not isinstance(info, dict):
                raise SetupError(
                    "Cloudflare inventory is incomplete; refuse partial adoption."
                )
            for item in result:
                key = json.dumps(item, sort_keys=True)
                if key in seen:
                    raise SetupError(
                        "Cloudflare inventory did not advance; refuse partial adoption."
                    )
                seen.add(key)
            items.extend(result)
            total = info.get("total_count", len(items))
            if not isinstance(total, int) or isinstance(total, bool) or total < len(items):
                raise SetupError(
                    "Cloudflare inventory is inconsistent; refuse partial adoption."
                )
            if total == len(items):
                return items
            per_page = info.get("per_page")
            if not result or not isinstance(per_page, int) or isinstance(per_page, bool) or per_page < 1:
                raise SetupError(
                    "Cloudflare inventory is incomplete; refuse partial adoption."
                )
            # Endpoints such as Email Routing addresses/rules cap per_page at 50.
            parameters.update(page=str(page + 1), per_page=str(per_page))
            page_path = base + "?" + urllib.parse.urlencode(parameters)
        raise SetupError("Cloudflare inventory exceeded its protected page limit.")


def check_routing_dns(records, proposed):
    """Never replace another mail provider or publish a second apex SPF policy."""
    required_mx = {
        item.get("content", "").lower().rstrip(".")
        for item in proposed
        if item.get("type") == "MX" and item.get("name") in {"@", "aidash.run"}
    }
    required_spf = [
        item.get("content", "").strip('"')
        for item in proposed
        if item.get("type") == "TXT"
        and item.get("name") in {"@", "aidash.run"}
        and item.get("content", "").strip('"').startswith("v=spf1")
    ]
    if not required_mx or len(required_spf) != 1:
        raise SetupError(
            "Cloudflare must provide a complete Routing DNS proposal before setup."
        )
    existing_spf = []
    for record in records:
        if record.get("name") != "aidash.run":
            raise SetupError("DNS inventory must be scoped to the exact zone apex.")
        if (
            record.get("type") == "MX"
            and record.get("content", "").lower().rstrip(".") not in required_mx
        ):
            raise SetupError(
                "Existing root MX belongs to another mail provider; review a mail migration instead of overwriting it."
            )
        if record.get("type") == "TXT" and record.get("content", "").strip(
            '"'
        ).startswith("v=spf1"):
            existing_spf.append(record["content"].strip('"'))
    if len(existing_spf) > 1 or any(item != required_spf[0] for item in existing_spf):
        raise SetupError(
            "Existing root SPF conflicts with Routing; review a merged policy instead of overwriting it or adding a second SPF."
        )


def discover_existing(
    api, account_id, zone_id, destination_email, *, configure_routing_dns=False
):
    """Return private tfvars after independently checking existing ownership."""
    account_id, zone_id = identifier(account_id), identifier(zone_id)
    if (
        not isinstance(destination_email, str)
        or not re.fullmatch(r"[^\s<>@,]+@[^\s<>@,]+", destination_email)
        or len(destination_email) > 254
    ):
        raise SetupError("Supply one private destination mailbox.")
    zone = api.call("GET", f"zones/{zone_id}")["result"]
    if (
        zone.get("name") != "aidash.run"
        or zone.get("status") != "active"
        or zone.get("account", {}).get("id") != account_id
    ):
        raise SetupError(
            "The exact zone must be active aidash.run in the supplied account."
        )
    routing = api.call("GET", f"zones/{zone_id}/email/routing")["result"]
    if routing.get("name") != "aidash.run" or (
        not configure_routing_dns
        and (routing.get("enabled") is not True or routing.get("status") != "ready")
    ):
        raise SetupError(
            "Existing Email Routing is not ready; inspect MX/SPF conflicts before changing DNS."
        )
    if configure_routing_dns:
        query = urllib.parse.urlencode({"name": "aidash.run", "per_page": 100})
        apex = api.inventory(f"zones/{zone_id}/dns_records?{query}")
        proposed = api.call("GET", f"zones/{zone_id}/email/routing/dns")["result"].get(
            "dns", []
        )
        check_routing_dns(apex, proposed)
    sending = [
        item
        for item in api.inventory(f"zones/{zone_id}/email/sending/subdomains")
        if item.get("name") == "aidash.run"
    ]
    if len(sending) != 1 or sending[0].get("enabled") is not True:
        raise SetupError(
            "Adopt one already configured aidash.run Sending registration; do not onboard it again."
        )
    addresses = [
        item
        for item in api.inventory(f"accounts/{account_id}/email/routing/addresses")
        if item.get("email") == destination_email
    ]
    if (
        len(addresses) != 1
        or not isinstance(addresses[0].get("verified"), str)
        or not addresses[0]["verified"]
    ):
        raise SetupError(
            "The private destination must already exist and be verified; no verification mail was sent."
        )
    rules = api.inventory(f"zones/{zone_id}/email/routing/rules")
    for address in ALIASES.values():
        matches = [
            rule
            for rule in rules
            if any(
                item.get("type") == "literal"
                and item.get("field") == "to"
                and item.get("value") == address
                for item in rule.get("matchers", [])
            )
        ]
        if len(matches) > 1:
            raise SetupError(
                "Existing deployment alias ownership is ambiguous; refuse duplicate adoption."
            )
        if matches:
            rule = matches[0]
            if (
                len(rule.get("matchers", [])) != 1
                or rule.get("source") not in {None, "api"}
                or len(rule.get("actions", [])) != 1
                or rule["actions"][0].get("type") != "forward"
            ):
                raise SetupError(
                    "Do not take over Worker, drop, or multi-matcher alias rules."
                )
            identifier(rule.get("id"))
    return {
        "account_id": account_id,
        "zone_id": zone_id,
        "domain": "aidash.run",
        # Cloudflare identifies a Sending subdomain by `tag`; the list has no `id`.
        "sending_subdomain_id": identifier(sending[0].get("tag")),
        "destination_address_id": identifier(addresses[0].get("id")),
        "destination_email": destination_email,
        "configure_routing_dns": configure_routing_dns,
    }


def gh(*arguments, data=None):
    # gh needs its own auth only. Never pass a Cloudflare bootstrap credential to
    # the child, and never put the newly generated token into its arguments.
    environment = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith("CLOUDFLARE_")
    }
    try:
        result = subprocess.run(
            ["gh", *arguments],
            input=data,
            capture_output=True,
            env=environment,
            timeout=60,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        raise SetupError(
            "GitHub secret operation failed without exposing private command output."
        ) from None
    if result.returncode:
        raise SetupError(
            "GitHub secret operation was not acknowledged; private command output was discarded."
        )
    return result.stdout


def sending_permission(api, account_id):
    query = urllib.parse.urlencode({"scope": "com.cloudflare.api.account"})
    groups = api.inventory(f"accounts/{account_id}/tokens/permission_groups?{query}")
    candidates = [
        group
        for group in groups
        if group.get("name") in PERMISSION_NAMES
        and group.get("scopes") == ["com.cloudflare.api.account"]
        and group.get("is_selectable") is True
    ]
    if len(candidates) != 1:
        raise SetupError(
            "The account API must expose one selectable account-scoped Email Sending permission group."
        )
    return identifier(candidates[0].get("id"))


def account_tokens(api, account_id):
    values, seen = [], set()
    for page in range(1, 101):
        value = api.call(
            "GET",
            f"accounts/{account_id}/tokens?page={page}&per_page=50&include_expired=true",
        )
        entries = value.get("result")
        if not isinstance(entries, list):
            raise SetupError("The account token inventory is unavailable.")
        for entry in entries:
            token_id = identifier(entry.get("id"))
            if token_id in seen:
                raise SetupError("Account token inventory did not advance.")
            seen.add(token_id)
        values.extend(entries)
        if len(entries) < 50:
            return values
    raise SetupError("Account token inventory exceeded its protected limit.")


def provision_sending_token(
    api, account_id, *, apply=False, rotate=False, journal=None
):
    """Create only when the protected secret is absent, or rotation is explicit.

    journal, when supplied by the orchestrator, receives nonsecret receipts only.
    An orphaned matching token requires reconciliation instead of creating another.
    Rotation preserves older tokens; revoke them separately after proving delivery.
    """
    account_id = identifier(account_id)
    secrets = json.loads(
        gh(
            "secret",
            "list",
            "--repo",
            REPOSITORY,
            "--env",
            ENVIRONMENT,
            "--json",
            "name",
        )
    )
    if not isinstance(secrets, list):
        raise SetupError("The protected GitHub secret inventory is unavailable.")
    exists = any(item.get("name") == SECRET for item in secrets)
    if exists and not rotate:
        return {"status": "existing_secret", "secret": SECRET}
    permission_id = sending_permission(api, account_id)
    existing = account_tokens(api, account_id)
    if not exists and any(
        item.get("name", "").startswith(TOKEN_NAME) and item.get("status") == "active"
        for item in existing
    ):
        raise SetupError(
            "An active Sending token exists without its protected secret; reconcile it before provisioning."
        )
    expiry = (
        (datetime.now(UTC) + timedelta(days=180))
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z")
    )
    token_name = TOKEN_NAME + (
        "-" + datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ") if rotate else ""
    )
    policy = {
        "name": token_name,
        "expires_on": expiry,
        "policies": [
            {
                "effect": "allow",
                "permission_groups": [{"id": permission_id}],
                "resources": {f"com.cloudflare.api.account.{account_id}": "*"},
            }
        ],
    }
    if not apply:
        return {
            "status": "planned",
            "secret": SECRET,
            "account_id": account_id,
            "permission_id": permission_id,
            "rotate": rotate,
        }
    verify_environment(ENVIRONMENT)
    result = api.call("POST", f"accounts/{account_id}/tokens", policy).get("result", {})
    token_id = identifier(result.get("id"))
    receipt = {
        "status": "created",
        "token_id": token_id,
        "account_id": account_id,
        "expires_on": expiry,
        "secret": SECRET,
    }
    try:
        if journal:
            journal(dict(receipt))
        token = result.get("value")
        if not isinstance(token, str) or not re.fullmatch(
            r"[A-Za-z0-9_-]{40,80}", token
        ):
            raise SetupError("Cloudflare returned no valid one-time token value.")
        verify_environment(ENVIRONMENT)
        gh(
            "secret",
            "set",
            SECRET,
            "--repo",
            REPOSITORY,
            "--env",
            ENVIRONMENT,
            data=token.encode(),
        )
    except Exception:  # noqa: BLE001 - cleanup token after any failed secret handoff, without logging private data
        try:
            api.call("DELETE", f"accounts/{account_id}/tokens/{token_id}")
            receipt["status"] = "revoked_after_handoff_failure"
        except Exception:  # noqa: BLE001 - receipt must identify cleanup work without private provider output
            receipt["status"] = "cleanup_required"
        if journal:
            journal(dict(receipt))
        raise SetupError(
            "Sending token handoff failed; inspect the nonsecret cleanup receipt before retrying.",
            receipt,
        ) from None
    receipt["status"] = "secret_installed"
    if journal:
        journal(dict(receipt))
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["discover", "sending-token"])
    parser.add_argument("--file", type=Path, required=True, help="Private JSON/tfvars.json inputs, including the scoped bootstrap token")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--rotate", action="store_true")
    args = parser.parse_args()
    try:
        values = private_input(args.file)
        api = CloudflareAPI(values["cloudflare_api_token"])
        if args.action == "discover":
            if args.apply or args.rotate:
                raise SetupError("Discovery is read only; apply routing through the reviewed Terraform stack")
            result = discover_existing(api, values["account_id"], values["zone_id"], values["destination_email"],
                                       configure_routing_dns=values.get("configure_routing_dns", False))
            result["cloudflare_api_token"] = values["cloudflare_api_token"]
            private_json(args.output, result)
            print(json.dumps({"private_tfvars_file": str(args.output), "discovery_only": True}))
        else:
            result = provision_sending_token(api, values["account_id"], apply=args.apply, rotate=args.rotate,
                                            journal=lambda receipt: private_json(args.output, receipt))
            private_json(args.output, result)
            print(json.dumps(result, sort_keys=True))
    except (SetupError, Refused, OSError, ValueError, KeyError, TypeError):
        print("Cloudflare setup incomplete; inspect private inputs, verified destination and protected GitHub settings")
        raise SystemExit(1) from None


if __name__ == "__main__":
    main()
