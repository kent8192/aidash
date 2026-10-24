#!/usr/bin/env python3
"""Project saved native routes and OpenAPI into the reviewed Bruno catalog.

No command is launched. Registered routes own inclusion; native OpenAPI owns
payloads. Reviewed metadata supplies only names/provenance for scenario dispatch.
Expected HTTP statuses and authority remain independently authored in Bruno.
"""

import argparse
from copy import deepcopy
import json
from pathlib import Path
import re

METHODS = {"GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"}
METHOD_PATTERN = "(?:GET|HEAD|POST|PUT|PATCH|DELETE|OPTIONS)"
ROUTE = re.compile(
    rf"^(/\S*)\s+({METHOD_PATTERN}(?:,\s*{METHOD_PATTERN})*)\s+(\S+)\s+(\S+)\s*$"
)
NODE_PATH = "/api/tenants/{tenant}/nodes/{node}"
REGISTRATION_KEYS = NODE_PATH + "/registration-keys"
GRAPH_KEYS = "/api/authorization/{tenant}/peer-graph-grant-keys"
# Only reviewed dispatch metadata for the eight source-defined additions.
# These entries cannot include a route absent from the saved native inventory.
ADDITIONS = {
    ("POST", NODE_PATH + "/default"): ("tenant-node-default", "nodes"),
    ("POST", NODE_PATH + "/deletion"): ("tenant-node-deletion", "nodes"),
    ("POST", NODE_PATH + "/restore"): ("tenant-node-restore", "nodes"),
    ("POST", NODE_PATH + "/purge"): ("tenant-node-purge", "nodes"),
    ("GET", REGISTRATION_KEYS): ("tenant-registration-keys", "manual_keys"),
    ("POST", REGISTRATION_KEYS): ("tenant-registration-keys-change", "manual_keys"),
    ("GET", GRAPH_KEYS): ("peer-graph-grant-keys", "manual_keys"),
    ("POST", GRAPH_KEYS): ("peer-graph-grant-keys-change", "manual_keys"),
}
# An existing method/path whose stale catalog spelling predates both normal
# native exports. This migration is exact; arbitrary name drift still fails.
LEGACY_RENAMES = {
    ("POST", "/api/tasks/{task}/remote-grants/{grant}/human-requests/answer"):
        ("remote-human-answer", "remote_human_answer"),
}


def object_without_duplicates(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def read_json(path):
    return json.loads(path.read_text(), object_pairs_hook=object_without_duplicates)


def native_routes(text):
    routes = {}
    for raw in text.splitlines():
        line = re.sub(r"^\[INFO\]\s*", "", raw).strip()
        if not line.startswith("/"):
            continue
        match = ROUTE.fullmatch(line)
        if match is None:
            raise ValueError(f"unparsed native route: {line}")
        path, methods, name, _namespace = match.groups()
        for method in re.findall(METHOD_PATTERN, methods):
            identity = (method, path)
            if identity in routes:
                raise ValueError(f"duplicate native route: {method} {path}")
            routes[identity] = None if name == "-" else name
    if not routes:
        raise ValueError("native route inventory is empty")
    return routes


def reference(document, ref):
    if not isinstance(ref, str) or not ref.startswith("#/"):
        raise ValueError(f"only local native references are supported: {ref}")
    value = document
    for token in ref[2:].split("/"):
        token = token.replace("~1", "/").replace("~0", "~")
        if not isinstance(value, dict) or token not in value:
            raise ValueError(f"unresolved native reference: {ref}")
        value = value[token]
    return value


def resolve_object(document, value):
    seen = set()
    while isinstance(value, dict) and "$ref" in value:
        ref = value["$ref"]
        if len(value) != 1 or ref in seen:
            raise ValueError(f"ambiguous native object reference: {ref}")
        seen.add(ref)
        value = reference(document, ref)
    if not isinstance(value, dict):
        raise ValueError("native operation metadata must be an object")
    return value


def schema_contract(document, schema):
    definitions = {}

    def visit(value):
        if isinstance(value, list):
            for item in value:
                visit(item)
        elif isinstance(value, dict):
            if "$ref" in value:
                ref = value["$ref"]
                prefix = "#/components/schemas/"
                if not isinstance(ref, str) or not ref.startswith(prefix):
                    raise ValueError(f"unsupported native schema reference: {ref}")
                encoded = ref[len(prefix):]
                if "/" in encoded:
                    raise ValueError(f"unsupported native schema sub-reference: {ref}")
                name = encoded.replace("~1", "/").replace("~0", "~")
                target = reference(document, ref)
                if name not in definitions:
                    definitions[name] = deepcopy(target)
                    visit(target)
            for key, child in value.items():
                if key != "$ref":
                    visit(child)

    visit(schema)
    return {"schema": deepcopy(schema), "definitions": definitions}


def operations(document):
    result = {}
    for path, raw_item in document["paths"].items():
        if not path.startswith("/"):
            raise ValueError(f"invalid native OpenAPI path: {path}")
        item = resolve_object(document, raw_item)
        for method, operation in item.items():
            if method.upper() in METHODS:
                identity = (method.upper(), path)
                if method != method.lower() or identity in result:
                    raise ValueError(f"ambiguous native operation: {identity}")
                result[identity] = (item, resolve_object(document, operation))
            elif method.lower() == "trace":
                raise ValueError(f"unsupported native HTTP method: {method} {path}")
    return result


def parameters(document, item, operation):
    merged = {}
    # OpenAPI explicitly permits an operation to override a path parameter.
    for owner in (item, operation):
        local = {}
        for raw in owner.get("parameters", []):
            parameter = resolve_object(document, raw)
            identity = (parameter["in"], parameter["name"])
            if identity in local:
                raise ValueError(f"duplicate native parameter: {identity}")
            local[identity] = parameter
            if "schema" in parameter:
                schema_contract(document, parameter["schema"])
        merged.update(local)
    return merged.values()


def project(showurls, document, reviewed):
    registered = native_routes(showurls)
    public = operations(document)
    extras = sorted(set(public) - set(registered))
    if extras:
        raise ValueError(f"OpenAPI operations absent from native inventory: {extras}")
    metadata = {}
    for route in reviewed:
        identity = (route["method"], route["path"])
        if identity in metadata:
            raise ValueError(f"duplicate reviewed route: {identity}")
        metadata[identity] = route
    result = []
    # Reviewed order controls layout only. Native reflection still owns which
    # identities exist; new operations are appended in deterministic order.
    ordered = [identity for identity in metadata if identity in registered]
    ordered.extend(sorted(set(registered) - set(metadata)))
    for method, path in ordered:
        identity = (method, path)
        native_name = registered[identity]
        prior = metadata.get(identity)
        if prior is None:
            addition = ADDITIONS.get(identity)
            if addition is None or native_name != addition[0]:
                raise ValueError(f"unannotated native route: {method} {path}")
            name, module = addition
            prior = {"name": name, "source": f"server/src/apps/identity/views/{module}.rs"}
            if identity not in public:
                raise ValueError(f"new typed operation missing from native OpenAPI: {identity}")
        name = prior["name"]
        rename = LEGACY_RENAMES.get(identity)
        if rename is not None and (name, native_name) == rename:
            name = rename[1]
        # HEAD companions have reviewed distinct Bruno names, while showurls
        # reports the original route name for both methods of the same handler.
        native_names = {name}
        if method == "HEAD" and name.endswith("-head"):
            native_names.add(name.removesuffix("-head"))
        if native_name is not None and native_name not in native_names:
            raise ValueError(f"native/reviewed operation name differs: {method} {path}")
        route = {"method": method, "path": path, "name": name, "source": prior["source"]}
        if "offerings" in prior:
            route["offerings"] = deepcopy(prior["offerings"])
        if identity not in public:
            # Unsupported-public-operation transport metadata is reviewed, not
            # schema authority. Never retain an old payload schema here.
            for field in ("json", "uuid_parameters", "query_parameters"):
                route[field] = deepcopy(prior[field])
        else:
            item, operation = public[identity]
            if operation.get("operationId") != name:
                raise ValueError(f"native OpenAPI operationId differs: {method} {path}")
            body = resolve_object(document, operation.get("requestBody", {}))
            content = body.get("content", {})
            route["json"] = "application/json" in content
            for media in content.values():
                if "schema" in media:
                    schema_contract(document, media["schema"])
            params = list(parameters(document, item, operation))
            route["uuid_parameters"] = sorted(
                parameter["name"] for parameter in params
                if parameter["in"] == "path"
                and resolve_object(document, parameter.get("schema", {})).get("format") == "uuid"
            )
            route["query_parameters"] = sorted(
                parameter["name"] for parameter in params if parameter["in"] == "query"
            )
            responses = {}
            for status, raw_response in sorted(operation.get("responses", {}).items()):
                response = resolve_object(document, raw_response)
                for media, value in response.get("content", {}).items():
                    if "schema" in value:
                        contract = schema_contract(document, value["schema"])
                        if media == "application/json":
                            responses[status] = contract
            if responses:
                route["responses"] = responses
        result.append(route)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--showurls", required=True, type=Path, help="Saved unfiltered native showurls output")
    parser.add_argument("--openapi", required=True, type=Path, help="Saved native openapi JSON")
    parser.add_argument("--metadata", required=True, type=Path, help="Current reviewed Bruno catalog")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--output", type=Path, help="Write the projected catalog")
    mode.add_argument("--check", type=Path, help="Compare with this committed catalog without writing")
    args = parser.parse_args()
    try:
        catalog = project(args.showurls.read_text(), read_json(args.openapi), read_json(args.metadata))
        encoded = json.dumps(catalog, indent=2, sort_keys=True) + "\n"
        if args.check:
            # Repository JSON formatting may compact arrays. Compare native
            # structure while retaining value types and semantic array order.
            if json.dumps(read_json(args.check), sort_keys=True) != json.dumps(catalog, sort_keys=True):
                raise ValueError("Bruno catalog differs from the saved native projection")
        else:
            args.output.write_text(encoded)
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f"Bruno catalog export failed: {error}\n")
    print(f"Bruno native catalog: {len(catalog)} endpoints")


if __name__ == "__main__":
    main()
