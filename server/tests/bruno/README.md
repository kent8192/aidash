# Real API contracts with Bruno

This collection exercises all native method/path endpoints in the committed catalog with 3–10 scenarios
per endpoint. The checked-in `contracts.json` is compared with the real
`manage showurls` output before requests start. `scenarios.json` identifies every
request, including the external OIDC authorization hops. It checks the public health and
OIDC configuration responses, anonymous rejection, scoped credentials, workspace
ownership, atomic revision conflicts, task creation and paging, strict JSON,
content types, credential revocation, browser registration, operator approval,
opaque cookies, origin and CSRF enforcement, and server-side logout.

The harness starts the compiled `manage server` custom command with isolated settings and
a unique empty PostgreSQL database. A second real Aidash peer has its own database
and reciprocally registered identity. Both use the normal Reinhardt bootstrap and
migrations. NATS and PostgreSQL are real services. A local RS256 OIDC issuer is
the external protocol fixture; it validates the authorization code, redirect
URI and S256 PKCE verifier. No application endpoint or repository is mocked.

From the workspace root, after the implementation is ready for verification:

```bash
docker compose up -d --wait postgres nats
npm exec --yes --package=@usebruno/cli@3.1.3 -- scripts/test-bruno-api.sh
```

To reuse an already built binary and an existing fixture PostgreSQL container:

```bash
python3 scripts/test-bruno-api.py \
  --binary /absolute/path/to/manage \
  --postgres-container aidash-test-postgres \
  --postgres-port 54370
```

The existing container must have the project's `pg_jsonschema` extension. Set
`AIDASH_BRUNO_POSTGRES_PASSWORD` when its test password differs from the Compose
default. `--nats-url` selects the existing test NATS endpoint. The harness creates
and removes only its own uniquely named databases and child processes; it
leaves the shared services running.

The `aidash` compatibility executable can also be supplied with `--binary`;
both entry points use the same Reinhardt command registry and bootstrap.

The CLI receives a temporary JSON environment using `--env-file`. Requests use
Bruno runtime variables for issued credentials and cookies. Automatic cookies
are disabled so anonymous and bearer requests remain explicit. Raw reports,
provider keys, credentials and logs stay in a private temporary directory that
is removed on exit. Sanitized request names, HTTP statuses, check results, source
revision, executable hash and provider call counts are written under
`.ignore/bruno/<run-id>/`. A partial, duplicated or skipped collection is a failure. Endpoint coverage
counts executed scenarios and successful assertions independently; missing
requests cannot be replaced by a duplicate.

Generate the ordinary Bruno requests after an intentional contract change with
`python3 scripts/bruno_contracts.py`, then inspect the changed expected statuses,
response schemas and assertions. `python3 scripts/bruno_contracts.py --check`
checks the catalog, manifest and request-file set. The expanded cases include
JSON/media rejection, typed IDs, paging, real credential/peer authentication,
state preservation on rejected writes, documented response fields and types,
frontend GET/HEAD/ETag behavior and browser activity/all-session logout.
Malformed JSON comes from a file body so the HTTP client's automatic JSON string
encoding cannot turn a syntax-error case into valid JSON of the wrong type.

SSE delivery and Last-Event-ID replay start three additional real Aidash processes
sequentially, sharing the isolated database. Only one streaming process runs at a
time to stay within the PostgreSQL connection budget. A local controller requests graceful drain
after the streaming response has started, so Bruno receives a finite real
`text/event-stream` body using a scoped subject credential. It checks ordered event IDs, resume exclusion, header
precedence and malformed-header fallback. The controller never supplies an API
response or event. Every process and database is owned by the harness scope.

These contracts complement the Rust, browser and cluster suites. Rejection cases
exercise transport and authority boundaries; they do not claim exhaustive happy
paths for every remote execution, inference or Kubernetes operation.

CLI usage: [Bruno command options](https://docs.usebruno.com/bru-cli/commandOptions).
