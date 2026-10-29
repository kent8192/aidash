#!/usr/bin/env python3
"""Render explicit Bruno contract scenarios and enforce complete route coverage.

The checked-in catalog comes from native showurls. Expected statuses and semantic
assertions are specified here, never learned from the server under test. Generated
requests remain ordinary Bruno files that can be inspected or run individually.
"""

from collections import Counter
import argparse
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
COLLECTION = ROOT / "server/tests/bruno"
CATALOG = COLLECTION / "contracts.json"
MANIFEST = COLLECTION / "scenarios.json"
MISSING = "11111111-1111-4111-8111-111111111111"
DEPLOYMENT_ADMINISTRATOR = {"Authorization": "Bearer {{deployment_administrator_token}}"}
# A Deployment Administrator holds no Tenant content or settings authority
# (#484, ADR 0102): the access boundary refuses it with 403 before a handler or
# an extractor runs. Only these routes admit it (`AccessBoundary::administered()`
# and `deployment_administrator()` in the apps' urls.rs, plus the session and
# run-management routes registered with the same boundary). Every other /api
# route is checked with the contract subject, which passes the boundary, so
# extractor errors such as 415 and 422 still reach the response.
ADMIN_ADMITTED = {
    "session", "state", "run-management-get", "run-management-control",
    "authorization-snapshot", "authorization-replace", "authorization-evaluate",
    "transaction_revocations", "transaction-participants",
    "tenant-administration", "tenant-registrations", "tenant-registration-approve",
    "tenant-registration-reject", "tenant-mappings", "tenant-mapping-disable",
    "tenant-invitations-issue", "tenant-invitations", "tenant-invitations-revoke",
    "tenants-label", "tenants-delete", "tenants-list", "tenants-purge",
    "openrouter-models",
    "deployment-status", "marketplace-administration",
    "sessions-admin-approve", "sessions-admin-disable-mapping",
    "sessions-admin-erase-display", "sessions-admin-identities", "sessions-admin-identity",
    "sessions-admin-mappings", "sessions-admin-registrations", "sessions-admin-reject",
    "sessions-admin-restore-identity", "sessions-deployment-administrator-grant",
    "sessions-deployment-administrator-grants",
}
SUBJECT = {"Authorization": "Bearer {{contract_token}}"}
# A Federation token (ADR 0064) is signed with a Deployment Key and bound to its
# request, so a Bruno request cannot carry a valid one: these scenarios only
# prove the boundary refuses what it cannot verify.
PEER = {
    "Authorization": "Bearer not-a-federation-token",
    "X-Aidash-Protocol": "0.5",
}
COOKIE = {
    "Cookie": "{{session_cookie}}",
    "Origin": "{{browser_origin}}",
    "X-Aidash-CSRF": "{{csrf}}",
}
# A built-in every Tenant's Registry holds from its creation (ADR 0033).
BUILTIN = {"id": "aidash.workspace_read", "version": "1.0.0"}
# A well-formed Node name no Tenant holds.
NODE = "brave-otter-k3m9q2xa"
APPROVAL = {"entry": BUILTIN, "enabled": True, "expected_revision": 0}
AGENT = {"id": "research", "version": "1.0.0"}
# Tenant routes: a Deployment Administrator reaches no Tenant's Registry,
# catalog or work (ADR 0029, ADR 0043). Each body passes its JSON extractor and
# input validation, so the 403 is the authority decision. A `refusal` is the
# contract subject's 403 for lacking the route's action; `path` aims the request
# at the journey's own workspace or task.
TENANT_ROUTES = {
    "POST /api/registry": {
        "body": {"id": "bruno.contract-skill", "version": "1.0.0", "kind": "skill",
                 "name": {"en": "Bruno"}, "description": {"en": "Contract probe"}},
        "refusal": "Subject without registry.create cannot register a definition",
    },
    "POST /api/agents/personal": {
        "body": {"entry": {"id": "bruno.personal", "version": "1.0.0", "kind": "agent",
                           "name": {"en": "Bruno"}, "description": {"en": "Contract probe"}},
                 "documents": []},
    },
    "POST /api/registry/mcp-tool-hint": {
        "body": {"endpoint": "https://bruno.example/mcp", "tool_name": "probe"},
        "refusal": "Subject without registry.create cannot read an MCP tool's hint",
    },
    "POST /api/skills/import": {
        "body": {"url": "https://github.com/bruno/skills"},
        "refusal": "Subject without registry.create cannot import a Skill",
    },
    "POST /api/agent-plugins/import": {
        "body": {"url": "https://github.com/bruno/plugins"},
        "refusal": "Subject without registry.create cannot import an Agent Plugin",
    },
    "POST /api/authorization/{tenant}/catalog": {
        "body": {"node": "{{tenant_node}}", **APPROVAL},
        "refusal": "Subject without catalog.approve cannot approve a version",
    },
    "POST /api/authorization/{tenant}/catalog/approvals": {
        "body": {"node": "{{tenant_node}}", "approvals": [{**APPROVAL}]},
        "refusal": "Subject without catalog.approve cannot approve a closure",
    },
    "POST /api/marketplace/installations/{id}/activation": {
        "body": {"tenant": "{{tenant}}", "revision": 1, "enabled": True,
                 "expected_activation_revision": 0, "expected_catalog_revision": 0},
    },
    "POST /api/marketplace/approval-sets": {
        "body": {"tenant": "{{tenant}}", "installations": [], "approvals": []},
    },
    "POST /api/discover": {"body": {}},
    "POST /api/workspaces": {"body": {"title": "Refused workspace", "goal": "Must not persist"}},
    "PATCH /api/workspaces/{id}": {
        "body": {"revision": 1, "state": {"refused": True}},
        "path": "/api/workspaces/{{workspace_id}}",
    },
    "POST /api/workspaces/{id}/tasks": {
        "body": {"title": "Refused task", "description": "Must not persist", "requirements": {},
                 "dependencies": [], "parent_id": None},
        "path": "/api/workspaces/{{workspace_id}}/tasks",
    },
    "POST /api/workspaces/{id}/messages": {
        "body": {"content": "Must not persist"},
        "path": "/api/workspaces/{{workspace_id}}/messages",
    },
    "POST /api/conversations": {
        "body": {"title": "Refused conversation", "goal": "Must not persist",
                 "target": AGENT, "target_kind": "agent"},
    },
    "POST /api/tasks/{id}/claim": {
        "body": {"revision": 0, "agent": AGENT},
        "path": "/api/tasks/{{task_id}}/claim",
    },
    "POST /api/tasks/{id}/delegate": {
        "body": {"node_id": "{{node_id}}", "agent": AGENT},
        "path": "/api/tasks/{{task_id}}/delegate",
    },
    "POST /api/runs/{id}/message": {"body": {"content": "Must not persist"}},
    "POST /api/human-requests/{id}/answer": {"body": {"answer": "Must not persist"}},
}
# Tenant reads by subjects. Journey 05 gives the contract subject `registry.read`
# on the Registry itself, which lets it list versions and approvals, but on no
# definition, so every list is empty; the journey's denied subject holds no
# action at all.
DENIED_SUBJECT = {"Authorization": "Bearer {{denied_token}}"}
SUBJECT_READS = {
    "GET /api/registry/versions": [
        ("Tenant subject lists only the versions it may read", 200, SUBJECT,
         "/api/registry/versions?node={{tenant_node}}",
         "expect(res.getBody().versions).to.eql([]);"),
        ("Subject without registry.read cannot list the versions", 403, DENIED_SUBJECT,
         "/api/registry/versions?node={{tenant_node}}", ""),
    ],
    "GET /api/authorization/{tenant}/catalog": [
        ("Tenant subject reads a catalog that approved nothing", 200, SUBJECT,
         "/api/authorization/{{tenant}}/catalog?node={{tenant_node}}",
         "expect(res.getBody()).to.eql([]);"),
        ("Subject without registry.read cannot read the catalog", 403, DENIED_SUBJECT,
         "/api/authorization/{{tenant}}/catalog?node={{tenant_node}}", ""),
    ],
    "GET /api/registry": [
        ("Subject discovers no definition it may not read", 200, SUBJECT, None,
         "expect(res.getBody()).to.eql([]);"),
    ],
    "GET /api/registry/{id}/{version}": [
        ("Subject cannot read a definition it may not read", 403, SUBJECT,
         "/api/registry/aidash.workspace_read/1.0.0", ""),
    ],
}


def key(route):
    return route["method"] + " " + route["path"]


def inventory(output):
    """Only native command output establishes which methods/paths are registered."""
    return Counter(
        (method, match[1])
        for line in output.splitlines()
        if (
            match := re.match(
                r"^(?:\[INFO\] )?\s*(/\S*)\s+((?:(?:GET|HEAD|POST|PUT|PATCH|DELETE|OPTIONS)(?:,\s*)?)+)\s",
                line,
            )
        )
        for method in re.findall(r"GET|HEAD|POST|PUT|PATCH|DELETE|OPTIONS", match[2])
    )


def load_manifest():
    routes = json.loads(CATALOG.read_text())
    manifest = json.loads(MANIFEST.read_text())
    expected = Counter(key(route) for route in routes)
    if not expected or any(count != 1 for count in expected.values()):
        raise ValueError("the catalog must contain distinct method/path endpoints")
    if manifest["endpoint_count"] != len(routes):
        raise ValueError("manifest endpoint count differs from the catalog")
    requests = manifest["requests"]
    names = Counter(request["name"] for request in requests)
    if any(count != 1 for count in names.values()):
        raise ValueError("scenario names must be unique")
    files = Counter(request["file"] for request in requests)
    if any(count != 1 for count in files.values()):
        raise ValueError("scenario files must be unique")
    counts = Counter(request["endpoint"] for request in requests if request["endpoint"])
    if set(counts) != set(expected) or any(
        not 3 <= count <= 10 for count in counts.values()
    ):
        raise ValueError("every endpoint requires 3-10 scenarios")
    actual_files = set((COLLECTION / "journeys").glob("[0-9]*.bru")) | set(
        (COLLECTION / "contracts").glob("[0-9]*.bru")
    )
    if {COLLECTION / request["file"] for request in requests} != actual_files:
        raise ValueError("manifest and collection files differ")
    for request in requests:
        text = (COLLECTION / request["file"]).read_text()
        name = re.search(r"^  name: (.+)$", text, re.M)
        if name is None or name[1] != request["name"]:
            raise ValueError("manifest and request names differ")
        if (
            "status" in request
            and f"expect(res.getStatus()).to.equal({request['status']})" not in text
        ):
            raise ValueError("scenario is missing its exact HTTP contract assertion")
    return routes, requests


def render():
    routes = json.loads(CATALOG.read_text())
    by_key = {key(route): route for route in routes}
    directory = COLLECTION / "contracts"
    directory.mkdir(exist_ok=True)
    requests = []
    rendered = {}
    sequence = 100

    def add(
        endpoint,
        label,
        status,
        *,
        path=None,
        headers=None,
        body=None,
        media="application/json",
        checks="",
        pre="",
        after="",
        seq=None,
        immutable=False,
    ):
        nonlocal sequence
        route = by_key[endpoint]
        sequence += 1
        ident = f"{sequence:04}"
        name = ident + " " + route["name"] + " - " + label
        if path is None:
            path = route["path"]
            for parameter in re.findall(r"\{([^}]+)\}", path):
                value = (
                    "{{tenant}}"
                    if parameter == "tenant"
                    else NODE
                    if parameter == "node"
                    else MISSING
                    if parameter in route["uuid_parameters"]
                    else "__bruno_missing__"
                )
                path = path.replace("{" + parameter + "}", value)
        for parameter in re.findall(r"(?<!\{)\{([^}]+)\}(?!\})", path):
            value = (
                "{{tenant}}"
                if parameter == "tenant"
                else NODE
                if parameter == "node"
                else MISSING
                if parameter in route["uuid_parameters"]
                else "__bruno_missing__"
            )
            path = path.replace("{" + parameter + "}", value)
        url = path if path.startswith("{{") else "{{base_url}}" + path
        method = route["method"].lower()
        fields = {"Accept": "application/json", **(headers or {})}
        if body is not None:
            fields["Content-Type"] = media
        # Axios quotes malformed JSON strings when a JSON content type is set.
        # A file body sends the exact broken bytes, preserving syntax-error tests.
        malformed = body == "{"
        form = body is not None and media == "application/x-www-form-urlencoded"
        kind = "file" if malformed else "formUrlEncoded" if form else "text" if body is not None else "none"
        capability = bool(route["source"] and "/capabilities/views/" in route["source"])
        js = [
            f'test("Exact HTTP {status}", function () {{ expect(res.getStatus()).to.equal({status}); }});'
        ]
        if status in (401, 403):
            message = "unauthorized" if status == 401 else "forbidden"
            envelope = (
                {
                    "error": {
                        "code": "UNAUTHORIZED" if status == 401 else "FORBIDDEN",
                        "message": message,
                        "retryable": False,
                    }
                }
                if capability
                else {"error": message}
            )
            js.append(
                'test("Stable authorization error envelope", function () { expect(res.getHeader("content-type")).to.include("application/json"); expect(res.getBody()).to.eql({error:'
                + json.dumps(envelope["error"])
                + "}); });"
            )
        elif status >= 400 and route["path"] != "/{<path:asset>}":
            js.append(
                r'test("Rejection is diagnostic and does not disclose SQL or credentials", function () { const value=res.getBody(); const text=typeof value === "string" ? value : JSON.stringify(value); expect(text.length).to.be.greaterThan(0); expect(text).not.to.match(/postgres:\/\/|SELECT .* FROM |deployment_administrator_token|peer_token/); });'
            )
        if status == 200 and route.get("response"):
            contract = route["response"]
            js.append(
                'test("Documented response fields and types", function () { const contract='
                + json.dumps(contract, separators=(",", ":"))
                + '; function valid(value,schema){if(schema.$ref)return valid(value,contract.definitions[schema.$ref.split("/").pop()]);if(schema.anyOf)return schema.anyOf.some(item=>valid(value,item));if(schema.oneOf)return schema.oneOf.filter(item=>valid(value,item)).length===1;if(schema.allOf&&!schema.allOf.every(item=>valid(value,item)))return false;if(schema.enum&&!schema.enum.some(item=>JSON.stringify(item)===JSON.stringify(value)))return false;const types=Array.isArray(schema.type)?schema.type:schema.type?[schema.type]:[];if(types.length&&!types.some(type=>type==="null"?value===null:type==="array"?Array.isArray(value):type==="object"?value!==null&&typeof value==="object"&&!Array.isArray(value):type==="integer"?Number.isInteger(value):typeof value===type))return false;if(value!==null&&typeof value==="object"&&!Array.isArray(value)){if((schema.required||[]).some(field=>!(field in value)))return false;for(const [field,property] of Object.entries(schema.properties||{}))if(field in value&&!valid(value[field],property))return false;}if(Array.isArray(value)&&schema.items&&!value.every(item=>valid(item,schema.items)))return false;return true;}expect(valid(res.getBody(),contract.schema)).to.equal(true); });'
            )
        if checks:
            js.append('test("' + label + ' contract", function () { ' + checks + " });")
        if immutable:
            pre += '\nconst snapshot = await bru.sendRequest({method:"GET",url:bru.getEnvVar("base_url")+"/api/workspaces/"+bru.getVar("workspace_id"),headers:{Authorization:"Bearer "+bru.getVar("contract_token")}}); if(snapshot.status!==200 || !Number.isInteger(snapshot.data.workspace.revision)) throw new Error("fixture snapshot unavailable"); bru.setVar("beforeWorkspace",JSON.stringify({revision:snapshot.data.workspace.revision,state:snapshot.data.workspace.state}));\n'
            after += '\nconst snapshot = await bru.sendRequest({method:"GET",url:bru.getEnvVar("base_url")+"/api/workspaces/"+bru.getVar("workspace_id"),headers:{Authorization:"Bearer "+bru.getVar("contract_token")}}); test("Rejected input preserves committed workspace state", function(){expect(snapshot.status).to.equal(200);expect(JSON.stringify({revision:snapshot.data.workspace.revision,state:snapshot.data.workspace.state})).to.equal(bru.getVar("beforeWorkspace"));});\n'
        text = (
            f"meta {{\n  name: {name}\n  type: http\n  seq: {seq if seq is not None else sequence}\n}}\n\n{method} {{\n  url: {url}\n  body: {kind}\n  auth: none\n}}\n\nheaders {{\n"
            + "\n".join("  " + k + ": " + v for k, v in fields.items())
            + "\n}\n"
        )
        if malformed:
            text += "\nbody:file {\n  file1: @file(fixtures/malformed-json.txt) @contentType(application/json)\n}\n"
        elif form:
            # Bruno interpolates URL-encoded fields, but deliberately leaves
            # strings with this media type unchanged. Keep each field typed.
            text += "\nbody:form-urlencoded {\n" + "\n".join(
                "  " + name + ": " + value
                for name, value in (field.split("=", 1) for field in body.split("&"))
            ) + "\n}\n"
        elif body is not None:
            text += (
                "\nbody:text {\n"
                + "\n".join("  " + line for line in body.splitlines())
                + "\n}\n"
            )
        if pre:
            text += "\nscript:pre-request {\n" + pre + "\n}\n"
        if after:
            text += "\nscript:post-response {\n" + after + "\n}\n"
        text += "\ntests {\n" + "\n".join("  " + line for line in js) + "\n}\n"
        filename = f"contracts/{ident}-{route['name'].replace('!', '')}.bru"
        rendered[filename] = text
        requests.append(
            {
                "name": name,
                "file": filename,
                "endpoint": endpoint,
                "scenario": label,
                "status": status,
            }
        )

    # Existing stateful journeys are counted as real endpoint scenarios. The
    # issuer authorization hop is an explicitly separate external request.
    for path in sorted((COLLECTION / "journeys").glob("[0-9]*.bru")):
        text = path.read_text()
        name = re.search(r"^  name: (.+)$", text, re.M)[1]
        method = re.search(r"\n(get|post|put|patch|delete|head) \{", text)[1].upper()
        url = re.search(r"^  url: (.+)$", text, re.M)[1]
        endpoint = None
        if "callback_url" in url:
            endpoint = "GET /auth/callback"
        elif "authorization_url" not in url:
            actual = url.replace("{{base_url}}", "").split("?", 1)[0]
            for route in routes:
                pattern = re.escape(route["path"])
                pattern = re.sub(r"\\\{[^}]+\\\}", r"[^/]+", pattern)
                if route["method"] == method and re.fullmatch(pattern, actual):
                    endpoint = key(route)
                    break
            if endpoint is None:
                raise ValueError("unmapped existing request: " + name)
        requests.append(
            {
                "name": name,
                "file": "journeys/" + path.name,
                "endpoint": endpoint,
                "scenario": "stateful journey"
                if endpoint
                else "external OIDC authorization",
            }
        )

    # Refresh the deliberately revoked bearer/session before the expanded suite.
    # A Deployment Administrator cannot issue it (#484): the harness minted a
    # spare credential for alice with `manage issue-subject-credential`.
    seed = "POST /api/authorization/{tenant}/credentials"
    add(
        seed,
        "Deployment Administrator cannot issue a fresh subject credential",
        403,
        headers=DEPLOYMENT_ADMINISTRATOR,
        body='{"subject":"alice","expires_in_seconds":3600}',
        checks='bru.setVar("contract_token",bru.getEnvVar("contract_alice_token"));',
        seq=35,
    )
    for old, seq in [(18, 36), (19, 37), (20, 38)]:
        path = next((COLLECTION / "journeys").glob(f"{old:02}-*.bru"))
        text = path.read_text()
        old_name = re.search(r"^  name: (.+)$", text, re.M)[1]
        name = "Fresh session: " + old_name
        text = text.replace("name: " + old_name, "name: " + name).replace(
            f"seq: {old}", f"seq: {seq}"
        )
        file = f"contracts/{seq:04}-fresh-browser-session.bru"
        rendered[file] = text
        requests.append(
            {
                "name": name,
                "file": file,
                "endpoint": None
                if old == 21
                else "GET /auth/login"
                if old == 20
                else "GET /auth/callback",
                "scenario": "fresh browser session",
            }
        )
    add(
        "POST /auth/activity",
        "Browser activity rejects missing CSRF",
        403,
        headers={"Cookie": "{{session_cookie}}", "Origin": "{{browser_origin}}"},
        seq=41,
    )
    add(
        "POST /auth/activity",
        "Browser activity rejects foreign origin",
        403,
        headers={**COOKIE, "Origin": "https://untrusted.example"},
        seq=42,
    )
    add(
        "POST /auth/activity",
        "Browser activity records authenticated activity",
        204,
        headers=COOKIE,
        checks='expect(res.getBody()).to.equal("");',
        seq=43,
    )
    add(
        "GET /auth/registration",
        "Approved registration remains visible",
        200,
        headers={"Cookie": "{{session_cookie}}"},
        checks='expect(res.getBody().status).to.equal("approved");expect(res.getBody().id).to.equal(bru.getVar("registration_id"));',
        seq=44,
    )
    add(
        "POST /auth/logout-all",
        "All-session logout requires CSRF",
        403,
        headers={"Cookie": "{{session_cookie}}", "Origin": "{{browser_origin}}"},
        seq=60,
    )
    add(
        "POST /auth/logout-all",
        "All-session logout rejects foreign origin",
        403,
        headers={**COOKIE, "Origin": "https://untrusted.example"},
        seq=61,
    )
    add(
        "POST /auth/logout-all",
        "All-session logout clears the browser authority",
        204,
        headers=COOKIE,
        checks='expect(res.getHeader("cache-control")).to.include("no-store");',
        seq=62,
    )


    # The merged desktop broker is exercised with real browser consent and the
    # same OIDC-backed identity before the browser's all-device logout.
    desktop_start = {"redirect_uri":"http://127.0.0.1:43217/callback","state":"ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssss","code_challenge":"2RMjpSmPO5-BTbKe-qJx8k-9zO390GJJG4q8jge3-2k"}
    add("POST /auth/desktop/start", "Start a bound S256 desktop handoff", 200,
        body=json.dumps(desktop_start), seq=45,
        checks='expect(res.getBody()).to.have.all.keys("authorization_url");expect(res.getBody().authorization_url).to.include(bru.getEnvVar("browser_origin")+"/auth/desktop/authorize?request=");bru.setVar("desktop_authorize_url",res.getBody().authorization_url.replace(bru.getEnvVar("browser_origin"),bru.getEnvVar("base_url")));const request=res.getBody().authorization_url.split("?request=")[1];expect(request).to.match(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);bru.setVar("desktop_request",request);')
    add("GET /auth/desktop/authorize", "Browser consent binds the handoff", 200,
        path="{{desktop_authorize_url}}", headers={"Cookie":"{{session_cookie}}"}, seq=46,
        checks='expect(res.getHeader("content-type")).to.include("text/html");expect(String(res.getBody())).to.include(bru.getVar("desktop_request"));expect(res.getHeader("content-security-policy")).to.include("http://127.0.0.1:43217");')
    add("POST /auth/desktop/authorize", "Desktop consent rejects wrong CSRF", 403,
        body="request={{desktop_request}}&csrf=invalid", media="application/x-www-form-urlencoded", headers=COOKIE, seq=47)
    add("POST /auth/desktop/authorize", "Desktop consent rejects foreign origin", 403,
        body="request={{desktop_request}}&csrf={{csrf}}", media="application/x-www-form-urlencoded", headers={**COOKIE,"Origin":"https://untrusted.example"}, seq=48)
    add("POST /auth/desktop/authorize", "Desktop consent returns the bound callback", 303,
        body="request={{desktop_request}}&csrf={{csrf}}", media="application/x-www-form-urlencoded", headers=COOKIE, seq=49,
        checks='const callback=res.getHeader("location");expect(callback.split("?")[0]).to.equal("http://127.0.0.1:43217/callback");const params=Object.fromEntries(callback.split("?")[1].split("&").map(pair=>pair.split("=").map(decodeURIComponent)));expect(params.state).to.equal("ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssss");expect(params.code).to.match(/^[0-9a-f]{64}$/);bru.setVar("desktop_code",params.code);')
    desktop_exchange = json.dumps({"code":"{{desktop_code}}","state":"ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssss","verifier":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","redirect_uri":"http://127.0.0.1:43217/callback"})
    token_checks='expect(res.getBody()).to.have.all.keys("access_token","refresh_token","expires_in");expect(res.getBody().access_token).to.match(/^aidash_desktop_/);expect(res.getBody().refresh_token).to.match(/^aidash_refresh_/);expect(res.getBody().expires_in).to.equal(300);bru.setVar("desktop_access",res.getBody().access_token);'
    add("POST /auth/desktop/exchange", "Exchange consumes the bound proof once", 200, body=desktop_exchange, seq=50,
        checks=token_checks+'bru.setVar("desktop_refresh",res.getBody().refresh_token);')
    add("POST /auth/desktop/exchange", "Consumed authorization code cannot be replayed", 401, body=desktop_exchange, seq=51)
    rotation=json.dumps({"refresh_token":"{{desktop_refresh}}","next_token":"aidash_refresh_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn"})
    add("POST /auth/desktop/refresh", "Rotate the desktop credential family", 200, body=rotation, seq=52, checks=token_checks)
    add("POST /auth/desktop/refresh", "Recover the same prepared rotation after response loss", 200, body=rotation, seq=53, checks=token_checks)
    add("GET /auth/session", "Desktop bearer resolves the OIDC identity", 200, headers={"Authorization":"Bearer {{desktop_access}}"}, seq=54,
        checks='expect(res.getBody().mappings).to.be.an("array").that.is.not.empty;expect(res.getBody().id).to.be.a("string");')
    add("POST /auth/desktop/revoke", "Revoke the rotating desktop credential family", 204, body=json.dumps({"refresh_token":"aidash_refresh_nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn"}), seq=58,
        checks='expect(res.getBody()).to.equal("");')
    add("GET /auth/session", "Revoked desktop bearer cannot resolve a session", 401, headers={"Authorization":"Bearer {{desktop_access}}"}, seq=59)

    for route in routes:
        endpoint = key(route)
        path = route["path"]
        method = route["method"]
        if path.startswith("/auth/desktop/"):
            if path == "/auth/desktop/start":
                for label, body in [
                    ("Reject unsafe loopback callback", {**desktop_start,"redirect_uri":"https://untrusted.example/callback"}),
                    ("Reject undersized state", {**desktop_start,"state":"short"}),
                    ("Reject malformed S256 challenge", {**desktop_start,"code_challenge":"invalid"}),
                    ("Reject unknown broker input", {**desktop_start,"unexpected":True}),
                ]:
                    add(endpoint,label,422 if label == "Reject unknown broker input" else 400,body=json.dumps(body))
            elif path == "/auth/desktop/authorize" and method == "GET":
                add(endpoint,"Unknown handoff does not reveal consent",401,path=path+"?request="+MISSING)
                add(endpoint,"Consent requires a request identity",400)
                add(endpoint,"Malformed handoff identity is rejected",400,path=path+"?request=invalid")
            elif path == "/auth/desktop/authorize":
                add(endpoint,"Desktop consent requires a browser session",401,body="request="+MISSING+"&csrf=invalid",media="application/x-www-form-urlencoded")
            elif path == "/auth/desktop/exchange":
                add(endpoint,"Missing broker proof is rejected",422,body="{}")
                add(endpoint,"Malformed broker JSON is rejected",400,body="{")
                add(endpoint,"Invalid verifier cannot issue a session",401,body=json.dumps({"code":"short","state":"state","verifier":"invalid","redirect_uri":"http://127.0.0.1:43217/callback"}))
            elif path == "/auth/desktop/refresh":
                add(endpoint,"Missing rotation credentials are rejected",422,body="{}")
                add(endpoint,"Successor must differ from predecessor",401,body=json.dumps({"refresh_token":"aidash_refresh_"+"r"*64,"next_token":"aidash_refresh_"+"r"*64}))
                add(endpoint,"Foreign credential cannot rotate a family",401,body=json.dumps({"refresh_token":"wrong_"+"r"*64,"next_token":"aidash_refresh_"+"n"*64}))
            elif path == "/auth/desktop/revoke":
                add(endpoint,"Revocation requires its input envelope",422,body="{}")
                add(endpoint,"Unknown refresh revocation remains idempotent",204,body=json.dumps({"refresh_token":"unknown"}),checks='expect(res.getBody()).to.equal("");')
            continue
        if path in ("/auth/activity", "/auth/logout-all"):
            continue
        if path in ("/", "/{<path:asset>}"):
            asset = path != "/"
            uri = "/assets/probe.txt" if asset else "/"
            body_check = (
                'expect(res.getBody()).to.equal("bruno-asset\\n");'
                if asset
                else 'expect(res.getBody()).to.include("Bruno frontend fixture");'
            )
            if method == "HEAD":
                body_check = 'expect(res.getBody()).to.equal("");'
            add(
                endpoint,
                "Serve the configured frontend content",
                200,
                path=uri,
                checks=body_check
                + 'expect(res.getHeader("x-content-type-options")).to.equal("nosniff");bru.setVar("frontendEtag",res.getHeader("etag"));',
            )
            add(
                endpoint,
                "Conditional request matches the content ETag",
                304,
                path=uri,
                headers={"If-None-Match": "{{frontendEtag}}"},
                checks='expect(res.getBody()).to.equal("");',
            )
            add(
                endpoint,
                "Stale conditional request serves current content",
                200,
                path=uri,
                headers={"If-None-Match": '"stale"'},
                checks=body_check,
            )
            if asset:
                add(
                    endpoint,
                    "Missing asset does not fall back to the SPA",
                    404,
                    path="/assets/missing.txt",
                )
                add(
                    endpoint,
                    "Client route falls back to the SPA",
                    200,
                    path="/bruno-client-route",
                    checks="expect(res.getBody())."
                    + (
                        'to.equal("");'
                        if method == "HEAD"
                        else 'to.include("Bruno frontend fixture");'
                    ),
                )
            continue
        if path in (
            "/health",
            "/live",
            "/ready",
            "/.well-known/aidash",
            "/auth/config",
            "/api/openapi.json",
        ):
            for label, headers in [
                ("Public contract", {}),
                (
                    "Untrusted bearer does not restrict public reads",
                    {"Authorization": "Bearer invalid"},
                ),
                (
                    "Foreign origin receives the same public contract",
                    {"Origin": "https://untrusted.example"},
                ),
            ]:
                checks = (
                    'expect(res.getBody()).to.equal("");'
                    if path in ("/live", "/ready")
                    else 'expect(res.getHeader("content-type")).to.include("application/json");expect(res.getBody()).to.be.an("object");'
                )
                if path == "/health":
                    checks += 'expect(res.getBody()).to.eql({status:"ok",deployment:bru.getEnvVar("node_id")});'
                if path == "/.well-known/aidash":
                    checks += 'expect(res.getBody().id).to.equal(bru.getEnvVar("node_id"));expect(res.getBody().protocol_version).to.equal("0.5");'
                if path == "/auth/config":
                    checks += 'expect(res.getBody()).to.have.all.keys("enabled","provider","login_url","desktop_protocol");expect(res.getBody().enabled).to.equal(true);expect(res.getBody().login_url).to.equal("/auth/login");'
                if path == "/api/openapi.json":
                    checks += 'expect(res.getBody().openapi).to.match(/^3\\./);expect(res.getBody().paths["/api/workspaces"]).to.have.property("post");expect(JSON.stringify(res.getBody())).not.to.include(bru.getEnvVar("deployment_administrator_token"));'
                add(endpoint, label, 200, headers=headers, checks=checks)
            continue
        if path == "/auth/login" and method == "POST":
            # Sign-in Domain discovery exists only for GCIP deployments. Bruno
            # URL-encodes these typed form fields itself.
            for label, body, headers in [
                ("OIDC deployment cannot route a Sign-in Domain", "email=alice@acme.example&return_to=/", {"Origin": "{{browser_origin}}"}),
                ("OIDC deployment refuses foreign-origin discovery", "email=alice@acme.example", {"Origin": "https://untrusted.example"}),
                ("Discovery without an address reveals nothing", "return_to=/", {"Origin": "{{browser_origin}}"}),
            ]:
                add(
                    endpoint,
                    label,
                    401,
                    headers=headers,
                    body=body,
                    media="application/x-www-form-urlencoded",
                    checks='expect(res.getHeader("set-cookie")).to.equal(undefined);',
                )
            continue
        if path == "/auth/login":
            for destination in (
                "https%3A%2F%2Funtrusted.example",
                "%2F%2Funtrusted.example",
                "%2Fbad%5Cpath",
            ):
                add(
                    endpoint,
                    "Reject unsafe return destination " + destination,
                    400,
                    path=path + "?return_to=" + destination,
                    checks='expect(res.getBody().error).to.equal("invalid return destination");',
                )
            continue
        if path == "/auth/callback":
            add(
                endpoint,
                "Missing callback code is rejected",
                400,
                path=path + "?state=invalid",
            )
            add(
                endpoint,
                "Unbound callback cannot create a session",
                401,
                path=path + "?state=invalid&code=invalid",
            )
            add(
                endpoint,
                "Tampered login cookie cannot bind the callback",
                401,
                path=path + "?state=invalid&code=invalid",
                headers={"Cookie": "__Host-aidash-login=tampered"},
            )
            continue
        if path == "/auth/provider-authorizations/{provider}/callback":
            # Unbound or unknown states fail before any provider exchange.
            uri = path.replace("{provider}", "openrouter")
            add(
                endpoint,
                "Callback without a browser binding is rejected",
                401,
                path=uri + "?state=invalid&code=invalid",
            )
            add(
                endpoint,
                "Tampered browser binding cannot complete an authorization",
                401,
                path=uri + "?state=invalid&code=invalid",
                headers={
                    "Cookie": "aidash-provider-authorization=tampered; __Host-aidash-provider-authorization=tampered"
                },
            )
            add(
                endpoint,
                "Callback without state is rejected",
                401,
                path=uri + "?code=invalid",
                headers={
                    "Cookie": "aidash-provider-authorization=tampered; __Host-aidash-provider-authorization=tampered"
                },
            )
            continue
        if path == "/auth/backchannel-logout":
            add(
                endpoint,
                "Missing logout token is rejected",
                400,
                body="unused=value",
                media="application/x-www-form-urlencoded",
            )
            add(
                endpoint,
                "Unsigned logout token is rejected",
                400,
                body="logout_token=eyJhbGciOiJub25lIn0.eyJzdWIiOiJicnVuby11c2VyIn0.",
                media="application/x-www-form-urlencoded",
            )
            add(
                endpoint,
                "Malformed logout token is rejected",
                400,
                body="logout_token=malformed",
                media="application/x-www-form-urlencoded",
            )
            continue
        if path in ("/auth/invitations/inspect", "/auth/invitations/redeem"):
            # Opening and redeeming a link need a browser session; the JSON
            # body is parsed first, so these scenarios send a valid one.
            link = json.dumps({"tenant": "{{tenant}}", "secret": "bruno-unknown-secret"})
            add(endpoint, "No opaque session cannot open an Invitation", 401,
                body=link, immutable=True)
            add(endpoint, "Forged opaque session cannot open an Invitation", 401,
                headers={"Cookie": "__Host-aidash-session=forged"}, body=link, immutable=True)
            add(endpoint, "Malformed JSON cannot reach the use case", 400,
                body="{", immutable=True)
            continue
        if path.startswith("/auth/"):
            for label, headers in [
                ("No opaque session", {}),
                ("Forged opaque session", {"Cookie": "__Host-aidash-session=forged"}),
                ("Revoked browser session", {"Cookie": "{{session_cookie}}"}),
            ]:
                add(endpoint, label, 401, headers=headers)
            continue
        if path.startswith("/federation/"):
            add(
                endpoint,
                "Reject unsupported federation protocol",
                400,
                headers={**PEER, "X-Aidash-Protocol": "9.9"},
                checks=(
                    'expect(res.getBody()).to.eql({error:{code:"INVALID_INPUT",message:"unsupported federation protocol",retryable:false}});'
                    if "/capabilities/views/" in (route["source"] or "")
                    else 'expect(res.getBody().error).to.equal("unsupported federation protocol");'
                ),
                immutable=method != "GET",
            )
            add(
                endpoint,
                "Reject the retired federation protocol 0.3",
                400,
                headers={**PEER, "X-Aidash-Protocol": "0.3"},
                immutable=method != "GET",
            )
            add(
                endpoint,
                "Require the Federation token",
                401,
                headers={k: v for k, v in PEER.items() if k != "Authorization"},
                immutable=method != "GET",
            )
            add(
                endpoint,
                "Refuse a token that does not verify",
                401,
                headers=PEER,
                immutable=method != "GET",
            )
            continue
        if path == "/api/tenants" and method == "POST":
            # The creation API authenticates itself, after parsing the body.
            label = json.dumps({"label": "Bruno contract"})
            add(endpoint, "Anonymous request cannot create a Tenant", 401,
                body=label, immutable=True)
            add(endpoint, "Subject Credential never creates a Tenant", 403,
                headers=SUBJECT, body=label, immutable=True)
            add(endpoint, "Blank Tenant label is rejected", 400,
                headers=DEPLOYMENT_ADMINISTRATOR, body=json.dumps({"label": "   "}), immutable=True)
            add(endpoint, "Malformed JSON cannot reach the use case", 400,
                headers=DEPLOYMENT_ADMINISTRATOR, body="{", immutable=True)
            continue
        add(endpoint, "Anonymous request is rejected", 401, immutable=method != "GET")
        admitted = route["name"] in ADMIN_ADMITTED
        # The caller for scenarios that must reach an extractor or a handler.
        actor = DEPLOYMENT_ADMINISTRATOR if admitted else SUBJECT
        if route["name"] == "tenants-personal":
            # Only a signed-in browser session recreates its Personal Tenant.
            add(endpoint, "Deployment Administrator bearer cannot recreate a Personal Tenant", 401,
                headers=DEPLOYMENT_ADMINISTRATOR, immutable=True)
            add(endpoint, "Subject Credential bearer cannot recreate a Personal Tenant", 401,
                headers=SUBJECT, immutable=True)
            continue
        if route["name"] == "tenants-delete":
            # Every scenario is refused: a recorded deletion would end the shared Tenant.
            add(endpoint, "Subject Credential bearer never deletes a Tenant", 403,
                headers=SUBJECT, body=json.dumps({"confirmation": "{{tenant}}"}), immutable=True)
            add(endpoint, "Deletion requires the typed Tenant name", 400,
                headers=DEPLOYMENT_ADMINISTRATOR, body=json.dumps({"confirmation": "bruno-other-tenant"}),
                immutable=True)
            add(endpoint, "Malformed JSON cannot reach the use case", 400,
                headers=DEPLOYMENT_ADMINISTRATOR, body="{", immutable=True)
            continue
        if route["name"] == "tenants-restore":
            # The service authenticates the caller itself (ADR 0060).
            add(endpoint, "Subject Credential bearer never restores a Tenant", 401,
                headers=SUBJECT, immutable=True)
            add(endpoint, "Only a deleted Tenant can be restored", 409,
                headers=DEPLOYMENT_ADMINISTRATOR, immutable=True)
            continue
        if route["name"] == "tenants-purge":
            # Only a Deployment Administrator purges, and only a deleted Tenant (ADR 0061).
            add(endpoint, "Subject Credential bearer never purges a Tenant", 403,
                headers=SUBJECT, immutable=True)
            add(endpoint, "Only a deleted Tenant can be purged", 409,
                headers=DEPLOYMENT_ADMINISTRATOR, immutable=True)
            continue
        if route["source"] == "server/src/apps/identity/views/nodes.rs":
            # Reading Nodes needs node.read; creating or relabelling one needs a
            # Tenant Administrator, which only a browser Mapping can be (#484).
            refusal = "Deployment Administrator has no Tenant authority over its Nodes"
            if method == "GET":
                add(endpoint, "Subject without node.read cannot list the Nodes", 403, headers=SUBJECT)
                add(endpoint, refusal, 403, headers=DEPLOYMENT_ADMINISTRATOR)
            elif method == "POST":
                add(endpoint, "Subject Credential bearer never creates a Node", 403,
                    headers=SUBJECT, body=json.dumps({"label": "Bruno node"}), immutable=True)
                add(endpoint, refusal, 403, headers=DEPLOYMENT_ADMINISTRATOR,
                    body=json.dumps({"label": "Bruno node"}), immutable=True)
                add(endpoint, "Malformed JSON cannot reach the use case", 400,
                    headers=SUBJECT, body="{", immutable=True)
            else:
                unknown = "/api/tenants/{{tenant}}/nodes/" + NODE
                body = json.dumps({"label": "Renamed"})
                add(endpoint, "Subject Credential bearer never relabels a Node", 403,
                    path=unknown, headers=SUBJECT, body=body, immutable=True)
                add(endpoint, refusal, 403, path=unknown,
                    headers=DEPLOYMENT_ADMINISTRATOR, body=body, immutable=True)
                add(endpoint, "Malformed JSON cannot reach the use case", 400,
                    path=unknown, headers=SUBJECT, body="{", immutable=True)
            continue
        if endpoint in TENANT_ROUTES:
            tenant_route = TENANT_ROUTES[endpoint]
            body = json.dumps(tenant_route["body"])
            add(endpoint, "Deployment Administrator bearer has no Tenant authority", 403,
                path=tenant_route.get("path"), headers=DEPLOYMENT_ADMINISTRATOR, body=body,
                immutable=True)
            if "refusal" in tenant_route:
                add(endpoint, tenant_route["refusal"], 403, path=tenant_route.get("path"),
                    headers=SUBJECT, body=body, immutable=True)
        if route["source"] == "server/src/apps/identity/views/tenants.rs":
            if method == "GET":
                add(endpoint, "Subject Credential cannot list Tenants", 403, headers=SUBJECT)
                add(endpoint, "Deployment Administrator lists every Tenant root record", 200, headers=DEPLOYMENT_ADMINISTRATOR,
                    checks='expect(res.getBody()).to.be.an("array");expect(res.getBody().map(tenant => tenant.name)).to.include(bru.getEnvVar("tenant"));')
            else:
                body = json.dumps({"label": "Renamed"})
                add(endpoint, "Deployment Administrator bearer never relabels a Tenant", 403,
                    headers=DEPLOYMENT_ADMINISTRATOR, body=body, immutable=True)
                add(endpoint, "Subject Credential bearer never relabels a Tenant", 403,
                    headers=SUBJECT, body=body, immutable=True)
                add(endpoint, "Malformed JSON cannot reach the use case", 400,
                    headers=DEPLOYMENT_ADMINISTRATOR, body="{", immutable=True)
            continue
        if route["source"] == "server/src/apps/identity/views/invitations.rs":
            # Only a browser Mapping or a Deployment Administrator administers Invitations.
            body = "{}" if route["json"] else None
            add(endpoint, "Subject Credential bearer never administers Invitations", 403,
                headers=SUBJECT, body=body, immutable=method != "GET")
            add(endpoint, "Unknown Tenant has no Invitations", 404,
                path=path.replace("{tenant}", "bruno-missing-tenant"), headers=DEPLOYMENT_ADMINISTRATOR,
                body=body, immutable=method != "GET")
            if route["json"]:
                add(endpoint, "Malformed JSON cannot reach the use case", 400,
                    headers=DEPLOYMENT_ADMINISTRATOR, body="{", immutable=True)
            if route["uuid_parameters"]:
                add(endpoint, "Typed resource IDs reject invalid UUIDs", 400,
                    path=path.replace("{id}", "not-a-uuid"), headers=DEPLOYMENT_ADMINISTRATOR, immutable=True)
            continue
        if route["source"] == "server/src/apps/identity/views/tenant_secrets.rs":
            body = json.dumps({"expected_revision": 0}) if route["json"] else None
            add(endpoint, "Forged bearer cannot manage Tenant Secrets", 401,
                headers={"Authorization": "Bearer forged"}, body=body)
            add(endpoint, "Deployment Administrator has no Tenant Secret authority", 403,
                headers=DEPLOYMENT_ADMINISTRATOR, body=body, immutable=method != "GET")
            for label, headers in [
                ("Missing Store fails closed for the subject", SUBJECT),
            ]:
                add(endpoint, label, 404, headers=headers, body=body,
                    checks='expect(res.getHeader("cache-control")).to.equal("no-store");expect(res.getBody()).to.eql({error:"Tenant Secrets are not available on this Deployment"});',
                    immutable=method != "GET")
            if route["json"]:
                add(endpoint, "Wrong JSON media type is rejected", 415,
                    headers=SUBJECT, body=body, media="text/plain", immutable=True)
                add(endpoint, "Malformed JSON cannot reach the use case", 400,
                    headers=SUBJECT, body="{", immutable=True)
                add(endpoint, "Required revision cannot be omitted", 422,
                    headers=SUBJECT, body="{}", immutable=True)
                add(endpoint, "Unknown lifecycle fields are rejected", 422,
                    headers=SUBJECT, body=json.dumps({"expected_revision": 0, "unexpected": True}), immutable=True)
            continue
        if route["source"] == "server/src/apps/identity/views/provider_credentials.rs":
            uri = path.replace("{provider}", "openrouter")
            body = (
                json.dumps({"expected_revision": 0, "provider_credential_id": None})
                if method == "PUT"
                else json.dumps({"expected_revision": 0})
                if route["json"]
                else None
            )
            add(endpoint, "Forged bearer cannot manage Provider Credentials", 401,
                path=uri, headers={"Authorization": "Bearer forged"}, body=body)
            add(endpoint, "Deployment Administrator has no Provider Credential authority", 403,
                path=uri, headers=DEPLOYMENT_ADMINISTRATOR, body=body, immutable=method != "GET")
            for label, headers in [
                ("Missing Store fails closed for the subject", SUBJECT),
            ]:
                add(endpoint, label, 404, path=uri, headers=headers, body=body,
                    checks='expect(res.getHeader("cache-control")).to.equal("no-store");expect(res.getBody()).to.eql({error:"Secret Store is not configured"});',
                    immutable=method != "GET")
            if route["json"]:
                add(endpoint, "Wrong JSON media type is rejected", 415,
                    path=uri, headers=actor, body=body, media="text/plain", immutable=True)
                add(endpoint, "Malformed JSON cannot reach the use case", 400,
                    path=uri, headers=actor, body="{", immutable=True)
                add(endpoint, "Required lifecycle fields cannot be omitted", 422,
                    path=uri, headers=actor, body="{}", immutable=True)
                invalid = json.loads(body)
                invalid["unexpected"] = True
                add(endpoint, "Unknown lifecycle fields are rejected", 422,
                    path=uri, headers=actor, body=json.dumps(invalid), immutable=True)
            elif route["uuid_parameters"]:
                add(endpoint, "Typed credential IDs reject invalid UUIDs", 400,
                    path=uri.replace("{id}", "not-a-uuid"), headers=actor)
            elif route["query_parameters"]:
                add(endpoint, "Credential paging rejects nonnumeric offsets", 400,
                    path=uri + "?offset=invalid", headers=actor)
            continue
        if route["source"] == "server/src/apps/identity/views/provider_authorizations.rs":
            # Provider Authorization binds a browser session; bearer callers
            # never start one, and Key Material is not an accepted field.
            uri = path.replace("{provider}", "openrouter")
            add(endpoint, "Forged bearer cannot start Provider Authorization", 401,
                path=uri, headers={"Authorization": "Bearer forged"}, body="{}")
            for label, headers in [
                ("Deployment Administrator bearer has no browser session to bind", DEPLOYMENT_ADMINISTRATOR),
                ("Subject bearer has no browser session to bind", SUBJECT),
            ]:
                add(endpoint, label, 403, path=uri, headers=headers, body="{}",
                    checks='expect(res.getHeader("set-cookie") || "").not.to.include("provider-authorization");',
                    immutable=True)
            add(endpoint, "Wrong JSON media type is rejected", 415,
                path=uri, headers=actor, body="{}", media="text/plain", immutable=True)
            add(endpoint, "Malformed JSON cannot reach the use case", 400,
                path=uri, headers=actor, body="{", immutable=True)
            add(endpoint, "Pasted Key Material is not an accepted field", 422,
                path=uri, headers=actor, body=json.dumps({"key_material": "sk-or-v1-pasted"}),
                immutable=True)
            continue
        if route["source"] == "server/src/apps/identity/views/plan_connections.rs":
            # A Plan Connection is one subject's own grant: only that subject's
            # dashboard or desktop session reaches it, never a bearer, and
            # Plan Tokens are never an accepted field.
            uri = path.replace("{provider}", "chatgpt")
            body = (
                json.dumps({"callback_url": "http://127.0.0.1:1457/auth/callback?state=s&code=c"})
                if path.endswith("/complete")
                else json.dumps({"expected_revision": 1})
                if method == "DELETE"
                else "{}"
                if route["json"]
                else None
            )
            add(endpoint, "Forged bearer cannot reach a Plan Connection", 401,
                path=uri, headers={"Authorization": "Bearer forged"}, body=body)
            for label, headers in [
                ("Deployment Administrator bearer owns no Plan Connection", DEPLOYMENT_ADMINISTRATOR),
                ("Subject Credential bearer never reaches a Plan Connection", SUBJECT),
            ]:
                add(endpoint, label, 403, path=uri, headers=headers, body=body,
                    immutable=method != "GET")
            if route["json"]:
                add(endpoint, "Wrong JSON media type is rejected", 415,
                    path=uri, headers=actor, body=body, media="text/plain", immutable=True)
                add(endpoint, "Malformed JSON cannot reach the use case", 400,
                    path=uri, headers=actor, body="{", immutable=True)
                add(endpoint, "Plan Tokens are not an accepted field", 422,
                    path=uri, headers=actor, body=json.dumps({"access_token": "pasted"}),
                    immutable=True)
            continue
        if route["source"] == "server/src/apps/execution/observation/views.rs":
            # Administrative observer settings/reports require a browser Mapping;
            # a per-approval projection additionally requires the underlying
            # approval's visibility. Use valid typed inputs so these cases reach
            # the authority boundary rather than failing a query extractor.
            uri = path.replace("{kind}", "capability")
            if path.endswith("/report"):
                uri += "?start=2026-10-01T00:00:00Z&end=2026-10-02T00:00:00Z"
            elif "node" in route["query_parameters"]:
                uri += "?node={{tenant_node}}"
            body = json.dumps({"expected_revision": 0, "decider": None}) if route["json"] else None
            add(endpoint, "Forged bearer cannot inspect approval observations", 401,
                path=uri, headers={"Authorization": "Bearer forged"}, body=body,
                immutable=method != "GET")
            add(endpoint, "Deployment Administrator bearer cannot inspect Tenant observations", 403,
                path=uri, headers=DEPLOYMENT_ADMINISTRATOR, body=body,
                immutable=method != "GET")
            if "{id}" not in path:
                add(endpoint, "Subject Credential bearer cannot administer the observer", 403,
                    path=uri, headers=SUBJECT, body=body, immutable=method != "GET")
            else:
                add(endpoint, "Observation resource IDs reject malformed UUIDs", 400,
                    path=uri.replace("{id}", "not-a-uuid"), headers=actor)
            if route["json"]:
                add(endpoint, "Wrong JSON media type is rejected", 415,
                    path=uri, headers=actor, body=body,
                    media="text/plain", immutable=True)
                add(endpoint, "Malformed JSON cannot reach the use case", 400,
                    path=uri, headers=actor, body="{", immutable=True)
                add(endpoint, "Observer revision is required", 422,
                    path=uri, headers=actor, body="{}", immutable=True)
            continue
        if route["source"] == "server/src/apps/identity/views/approval_preferences.rs":
            # A person's Approval Mode and Approval Rules are their own: only the
            # owner's dashboard or desktop session reaches them, and a Tenant
            # Administrator's read needs a Mapping session too, never a bearer.
            uri = path + "?node={{tenant_node}}" if "node" in route["query_parameters"] else None
            body = (
                json.dumps({"expected_revision": 0, "default_mode": "manual"})
                if path.endswith("/approval-preferences/mine")
                else json.dumps({"expected_revision": 0, "decider": None})
                if path.endswith("/approval-decider")
                else json.dumps({"kind": "tool_call", "target": "every", "effect": "ask"})
                if method == "POST"
                else json.dumps({"mode": None})
                if route["json"]
                else None
            )
            add(endpoint, "Forged bearer cannot reach a person's approval settings", 401,
                path=uri, headers={"Authorization": "Bearer forged"}, body=body)
            for label, headers in [
                ("Deployment Administrator bearer owns no approval settings", DEPLOYMENT_ADMINISTRATOR),
                ("Subject Credential bearer never reaches approval settings", SUBJECT),
            ]:
                add(endpoint, label, 403, path=uri, headers=headers, body=body, immutable=method != "GET")
            if route["json"]:
                add(endpoint, "Wrong JSON media type is rejected", 415,
                    path=uri, headers=actor, body=body, media="text/plain", immutable=True)
                add(endpoint, "Malformed JSON cannot reach the use case", 400,
                    path=uri, headers=actor, body="{", immutable=True)
                add(endpoint, "Unknown approval fields are rejected", 422,
                    path=uri, headers=actor, body=json.dumps({"unexpected": True}),
                    immutable=True)
            continue
        if route["source"] == "server/src/apps/identity/views/tenant_administration.rs":
            # Only a browser Mapping context can be a Tenant Administrator.
            uri = path.replace("{subject}", "alice")
            body = (
                json.dumps({"groups": [], "expected_policy_revision": 1})
                if method == "PUT"
                else json.dumps({"subject": "carol", "groups": []})
                if path.endswith("/approve")
                else json.dumps({"expected_revision": 1})
                if route["json"]
                else None
            )
            for label, headers in [
                ("Deployment Administrator bearer is never a Tenant Administrator", DEPLOYMENT_ADMINISTRATOR),
                ("Subject Credential bearer is never a Tenant Administrator", SUBJECT),
            ]:
                add(endpoint, label, 403, path=uri, headers=headers, body=body,
                    immutable=method != "GET")
            if route["json"]:
                add(endpoint, "Malformed JSON cannot reach the use case", 400,
                    path=uri, headers=actor, body="{", immutable=True)
            if route["uuid_parameters"]:
                add(endpoint, "Typed resource IDs reject invalid UUIDs", 400,
                    path=uri.replace("{id}", "not-a-uuid"), headers=actor,
                    body=body, immutable=method != "GET")
            elif route["query_parameters"]:
                add(endpoint, "Paging rejects nonnumeric offsets", 400,
                    path=uri + "?offset=invalid", headers=actor)
            continue
        if route["source"] == "server/src/apps/execution/capabilities/views/live_previews.rs":
            # A Live preview's status, stop, ticket and the Tenant's lowered limits.
            if path == "/api/runs/{id}/live-preview":
                add(endpoint, "Anonymous request is rejected", 401)
                add(endpoint, "Deployment Administrator cannot read a missing Run", 403,
                    headers=DEPLOYMENT_ADMINISTRATOR)
                add(endpoint, "Subject bearer cannot read a missing Run", 404, headers=SUBJECT)
            elif path.endswith("/stop"):
                add(endpoint, "Anonymous stop is rejected", 401)
                add(endpoint, "Deployment Administrator cannot stop a missing Run", 403,
                    headers=DEPLOYMENT_ADMINISTRATOR)
                add(endpoint, "Subject bearer cannot stop a missing Run", 404, headers=SUBJECT)
            elif path.endswith("/live-tickets"):
                ticket = json.dumps({"live_preview_id": MISSING})
                add(endpoint, "Anonymous request is rejected", 401)
                add(endpoint, "Malformed JSON cannot reach the use case", 400,
                    headers=SUBJECT, body="{")
                add(endpoint, "Unknown Ticket fields are rejected", 422, headers=SUBJECT,
                    body=json.dumps({"live_preview_id": MISSING, "unexpected": True}))
                add(endpoint, "Ticket input is required", 422, headers=SUBJECT, body="{}")
                add(endpoint, "A missing Live preview has no ticket", 404,
                    headers=SUBJECT, body=ticket)
            elif method == "GET":
                add(endpoint, "Anonymous request is rejected", 401)
                add(endpoint, "Deployment Administrator cannot read Tenant limits", 403,
                    headers=DEPLOYMENT_ADMINISTRATOR)
                add(endpoint, "Subject bearer is not a Tenant Administrator", 403,
                    headers=SUBJECT)
            else:
                limits = json.dumps({"tenant": "{{tenant}}", "idle_seconds": 600,
                                     "absolute_seconds": 3600, "expected_revision": 0})
                add(endpoint, "Anonymous request is rejected", 401,
                    body=limits, immutable=True)
                add(endpoint, "Malformed JSON cannot reach the use case", 400,
                    headers=SUBJECT, body="{", immutable=True)
                add(endpoint, "Unknown limit fields are rejected", 422, headers=SUBJECT,
                    body=json.dumps({"tenant": "{{tenant}}", "idle_seconds": 600,
                                     "absolute_seconds": 3600, "expected_revision": 0,
                                     "unexpected": True}), immutable=True)
                add(endpoint, "Limit input is required", 422, headers=SUBJECT,
                    body="{}", immutable=True)
                add(endpoint, "Subject bearer is not a Tenant Administrator", 403,
                    headers=SUBJECT, body=limits, immutable=True)
            continue
        if route["source"] == "server/src/apps/execution/capabilities/views/previews.rs":
            flag = 'expect(res.getBody()).to.have.all.keys("available","preview_domain");expect(res.getBody().available).to.equal(false);expect(res.getBody().preview_domain).to.equal(null);'
            if path.endswith("/availability"):
                add(endpoint, "Anonymous request is rejected", 401)
                add(endpoint, "Deployment Administrator cannot read preview availability", 403,
                    headers=DEPLOYMENT_ADMINISTRATOR)
                add(endpoint, "Subject bearer reads the availability flag", 200,
                    headers=SUBJECT, checks=flag)
            elif path.endswith("/remote-preview"):
                add(endpoint, "Anonymous remote inspection is rejected", 401)
                add(endpoint, "Deployment Administrator cannot inspect a remote Run", 403,
                    headers=DEPLOYMENT_ADMINISTRATOR)
                add(endpoint, "Malformed Run identifier is rejected", 400,
                    path=path.replace("{id}", "not-a-uuid"), headers=SUBJECT)
            elif path.endswith("/preview-tickets"):
                body = json.dumps({"kind": "file", "area_id": MISSING, "file_id": MISSING})
                add(endpoint, "Anonymous remote ticket is rejected", 401, body=body)
                add(endpoint, "Deployment Administrator cannot issue a remote ticket", 403,
                    headers=DEPLOYMENT_ADMINISTRATOR, body=body)
                add(endpoint, "Malformed JSON cannot reach the remote ticket service", 400,
                    headers=SUBJECT, body="{")
                add(endpoint, "Unknown remote ticket fields are rejected", 422,
                    headers=SUBJECT, body=json.dumps({"kind": "file", "area_id": MISSING,
                                                    "file_id": MISSING, "unexpected": True}))
            else:
                body = json.dumps({"area_id": MISSING, "file_id": MISSING})
                add(endpoint, "Deployment Administrator cannot issue a ticket", 403,
                    headers=DEPLOYMENT_ADMINISTRATOR, body=body)
                add(endpoint, "Malformed JSON cannot reach the use case", 400,
                    headers=SUBJECT, body="{")
                add(endpoint, "Unknown Ticket fields are rejected", 422,
                    headers=SUBJECT, body=json.dumps({"area_id": MISSING, "file_id": MISSING, "unexpected": True}))
                add(endpoint, "Ticket input is required", 422, headers=SUBJECT, body="{}")
            continue
        if route["json"]:
            add(
                endpoint,
                "Wrong JSON media type is rejected",
                415,
                headers=actor,
                body="{}",
                media="text/plain",
                immutable=True,
            )
            add(
                endpoint,
                "Malformed JSON cannot reach the use case",
                400,
                headers=actor,
                body="{",
                immutable=True,
            )
            if route["source"] == "server/src/apps/knowledge/views/memory.rs":
                add(endpoint, "Incomplete native memory envelope is rejected", 422,
                    headers=actor, body="{}", immutable=True)

            continue
        if route["uuid_parameters"]:
            bad = path
            for parameter in re.findall(r"\{([^}]+)\}", bad):
                bad = bad.replace(
                    "{" + parameter + "}",
                    "not-a-uuid"
                    if parameter in route["uuid_parameters"]
                    else "{{tenant}}"
                    if parameter == "tenant"
                    else "__bruno_missing__",
                )
            add(
                endpoint,
                "Typed resource IDs reject invalid UUIDs",
                400,
                path=bad,
                headers=actor,
                immutable=method != "GET",
            )
        else:
            add(
                endpoint,
                "Malformed bearer syntax is rejected",
                401,
                headers={"Authorization": "Basic invalid"},
                immutable=method != "GET",
            )
        if method != "GET":
            add(
                endpoint,
                "Unrecognized bearer cannot mutate state",
                401,
                headers={"Authorization": "Bearer invalid"},
                immutable=True,
            )
            if path == "/api/workspaces/{id}/attachments":
                add(
                    endpoint,
                    "Attachment upload requires its typed query",
                    400,
                    headers=actor,
                    body="bruno",
                    media="application/octet-stream",
                    immutable=True,
                )
            continue
        if route["source"] in {
            "server/src/apps/registry/views/management.rs",
            "server/src/apps/marketplace/views/management.rs",
        } and path.endswith("/secret-readiness"):
            add(endpoint, "Deployment Administrator bearer has no Tenant authority", 403,
                headers=DEPLOYMENT_ADMINISTRATOR)
            add(endpoint, "Forged bearer cannot read Tenant Secret readiness", 401,
                headers={"Authorization": "Bearer forged"})
            add(endpoint, "Subject without registry.read cannot read Tenant Secret readiness", 403,
                headers=DENIED_SUBJECT)
            continue
        # The positive/missing-resource read reaches the application and database,
        # rather than merely confirming that the router has a matching endpoint.
        headers = actor
        status = (
            404 if "{" in path and path.count("{") > path.count("{tenant}") else 200
        )
        checks = 'expect(res.getHeader("content-type")).to.include("application/json");'
        if path.startswith("/api/workspaces/"):
            if path.endswith("/message-history"):
                uri = "/api/workspaces/{{workspace_id}}/message-history"
                status = 200
                checks += 'expect(res.getBody().messages).to.be.an("array");'
            elif path == "/api/workspaces/{id}":
                uri = "/api/workspaces/{{workspace_id}}"
                status = 200
                checks += "expect(res.getBody().workspace.revision).to.equal(1);expect(res.getBody().workspace.state).to.eql({bruno:true});"
            elif "/semantic/" in path:
                uri = None
                status = 200 if path.endswith(("/history", "/cleanup")) else 404
                if status == 200 and path.endswith("/history"):
                    checks += "expect(res.getBody()).to.eql([]);"
            else:
                uri = None
        else:
            uri = None
        core = route["source"] and "/capabilities/views/" in route["source"]
        if core:
            headers = SUBJECT
            if not route["uuid_parameters"]:
                status = 200
                checks += 'expect(res.getBody().items).to.be.an("array");expect(res.getBody().next_cursor).to.equal(null);'
        label = "Application read preserves the typed response contract"
        # Authority is checked before disclosing missing resources on these routes.
        # Sources: identity remote/graph, generation reads, capability run_access,
        # Marketplace subject dispatch, the Tenant Registry (ADR 0029) and workbench
        # inspection of a Tenant's version. Deployment Administrator credentials cannot
        # replace a subject or browser-bound graph grant.
        denied_reads = {
            "memory-participant-current",
            "remote-semantic-home-provenance",
            "remote-semantic-run-provenance",
            "remote_execution_list",
            "federated_graph_peers",
            "run_working_area",
            "research-providers",
            "core_outbound_history",
            "core_operation_history",
            "generation-usage",
            "generation-spec",
            "marketplace-browse",
            "marketplace-detail",
            "marketplace-sources",
            "marketplace-read-consent",
            "marketplace-installations",
            "marketplace-installation",
            "registry-list",
            "registry-get",
            "registry-versions",
            "authorization-catalog",
            "authorization-safety-stops",
            "workbench-inspect-version",
            "workbench-export-report",
        }
        if route["name"] in denied_reads:
            status = 403
            checks = ""
            label = "Missing scoped authority cannot disclose application data"
        if route["name"] == "file_recipient_list":
            uri = path + "?node_id={{node_id}}/{{tenant_node}}"
            status = 403
            checks = ""
            label = "Subject without file transfer permission cannot list recipients"
        # A Deployment Administrator names the Tenant of the Run it reads (ADR 0101).
        if route["name"] in {
            "marketplace-administration",
            "workbench-test-profiles",
            "run-management-get",
        }:
            uri = path + "?tenant={{tenant}}"
        if route["name"] in {"authorization-catalog", "registry-versions"}:
            uri = path + "?node=" + NODE
        if route["name"] in {
            "generation-history",
            "workbench-list-incidents",
            "workbench-version-audit",
        }:
            status = 200
            checks += (
                "expect(res.getBody()"
                + (".items" if route["name"] == "workbench-version-audit" else "")
                + ").to.eql([]);"
            )
        if route["name"] == "memory-participants":
            status = 200
            checks += 'expect(res.getBody()).to.eql({items:[],next:null});'
        if path == "/api/session":
            checks += 'expect(res.getBody().access.kind).to.equal("deployment_administrator");'
        if path == "/api/tasks":
            checks += 'expect(res.getBody().tasks.map(value=>value.id)).to.include(bru.getVar("task_id"));'
        if path == "/api/events":
            checks += 'expect(res.getBody()).to.be.an("array");expect(res.getHeader("x-aidash-event-cursor")).to.match(/^\\d+$/);'
        if path == "/api/events/stream":
            add(
                endpoint,
                "SSE rejects an invalid query cursor",
                400,
                path=path + "?after=invalid",
                headers=SUBJECT,
            )
            add(
                endpoint,
                "Deployment Administrator cannot subscribe to Tenant events",
                403,
                path=path + "?after=0",
                headers=DEPLOYMENT_ADMINISTRATOR,
            )
            for number, label in [
                (1, "Initial event replay"),
                (2, "Last-Event-ID overrides the query cursor"),
                (3, "Invalid Last-Event-ID falls back to the query cursor"),
            ]:
                cursor_headers = {**SUBJECT}
                query = "?after=0"
                if number == 2:
                    cursor_headers["Last-Event-ID"] = "{{resume_cursor}}"
                if number == 3:
                    cursor_headers["Last-Event-ID"] = "invalid"
                    query = "?after={{resume_cursor}}"
                add(
                    endpoint,
                    label,
                    200,
                    path="{{stream_base_" + str(number) + "}}" + path + query,
                    headers=cursor_headers,
                    seq=10000 + number,
                    pre='await bru.sendRequest({method:"POST",url:bru.getEnvVar("control_url")+"/drain/'
                    + str(number)
                    + '"});',
                    checks='expect(res.getHeader("content-type")).to.include("text/event-stream");expect(res.getHeader("cache-control")).to.equal("no-store");const ids=String(res.getBody()).split("\\n").filter(line=>line.startsWith("id: ")).map(line=>Number(line.slice(4)));expect(ids.length).to.be.greaterThan(0);expect(ids).to.eql([...ids].sort((a,b)=>a-b));'
                    + (
                        'bru.setVar("resume_cursor",ids[0]);'
                        if number == 1
                        else 'expect(ids.every(id=>id>Number(bru.getVar("resume_cursor")))).to.equal(true);'
                    ),
                )
            continue
        # Tenant content is read as the contract subject on the few routes whose
        # response the journeys fix; every other Tenant route is shown to refuse
        # the administrator (#484).
        subject_read = (
            path in ("/api/workspaces/{id}", "/api/tasks", "/api/events")
            or path.endswith("/message-history")
        )
        if not admitted and not core:
            if subject_read:
                headers = SUBJECT
            else:
                label = "Deployment Administrator has no Tenant authority"
                status = 403
                headers = DEPLOYMENT_ADMINISTRATOR
                checks = ""
        add(
            endpoint,
            label,
            status,
            path=uri,
            headers=headers,
            checks=checks,
        )
        numeric = next(
            (
                parameter
                for parameter in route["query_parameters"]
                if parameter in ("offset", "after", "limit", "revision", "cursor")
            ),
            None,
        )
        # This cursor is an arbitrary profile ID, not a numeric page offset.
        if numeric and route["name"] != "workbench-tenant-test-profiles":
            add(
                endpoint,
                "Paging rejects nonnumeric or malformed cursors",
                400,
                path=(uri or path).replace("{tenant}", "{{tenant}}")
                + ("&" if "?" in (uri or path) else "?")
                + numeric
                + "=invalid",
                headers=actor,
            )
        for read_label, read_status, read_headers, read_path, read_checks in SUBJECT_READS.get(
            endpoint, []
        ):
            add(endpoint, read_label, read_status, path=read_path, headers=read_headers,
                checks=read_checks)

    # Exact generated file set; no stale request can silently increase coverage.
    for path in directory.glob("[0-9]*.bru"):
        if str(path.relative_to(COLLECTION)) not in rendered:
            path.unlink()
    for name, text in rendered.items():
        (COLLECTION / name).write_text(text)
    for request in requests:
        request["required_checks"] = re.findall(
            r'\btest\("([^"\n]+)"', (COLLECTION / request["file"]).read_text()
        )
        if not request["required_checks"]:
            raise ValueError("each scenario must contain contract assertions")
    MANIFEST.write_text(
        json.dumps(
            {"version": 1, "endpoint_count": len(routes), "requests": requests}, indent=2
        )
        + "\n"
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if not args.check:
        render()
    routes, requests = load_manifest()
    print(
        f"Bruno contracts: {len(routes)} endpoints; {len(requests)} requests; 3-10 scenarios per endpoint"
    )


if __name__ == "__main__":
    main()
