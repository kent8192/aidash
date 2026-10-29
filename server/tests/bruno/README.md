# Real API contracts with Bruno

This collection exercises all native method/path endpoints in the committed catalog with 3–10 scenarios
per endpoint. The checked-in `contracts.json` is compared with the real
`manage showurls` output before requests start. `scenarios.json` identifies every
request, including the external OIDC authorization hops. It checks the public health and
OIDC configuration responses, anonymous rejection, scoped credentials, workspace
ownership, atomic revision conflicts, task creation and paging, strict JSON,
content types, credential revocation, browser registration, Deployment Administrator approval,
opaque cookies, origin and CSRF enforcement, and server-side logout.

A Deployment Administrator holds no Tenant content or settings authority (#484, ADR 0102),
so the harness plays the host operator: it creates the Tenant and writes a bootstrap
policy as the administrator, then mints the journeys' Subject Credentials (alice, bob and a
spare for the expanded suite) with `manage issue-subject-credential`, using the server's own
settings. Journeys 06, 07 and the seed request assert that the administrator cannot issue
them, and pass the host-minted credentials on through the environment. Journey 25 revokes
alice's credential through her browser Mapping, the only way to administer a Tenant's
credentials, which is why the browser login journeys run before it. Requests that must reach
an extractor on a Tenant route use the contract subject, because the access boundary
answers the administrator with 403 first; the administered routes keep using the
administrator.

The harness starts the compiled `manage server` custom command with isolated settings and
a unique empty PostgreSQL database. It uses the normal Reinhardt bootstrap and
migrations. Federation routes only prove that the boundary refuses what it cannot
verify, because a Federation token is signed for its exact request. NATS and PostgreSQL are real services. A local RS256 OIDC issuer is
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

The existing container must have the project's `pg_jsonschema` extension and the
`aidash_owner` and `aidash_server` logins that `deploy/postgres/init-roles.sh`
creates. Migrations run as `aidash_owner` and the servers log in as
`aidash_server`; set `AIDASH_BRUNO_OWNER_PASSWORD` and
`AIDASH_BRUNO_SERVER_PASSWORD` when their passwords differ from the Compose
defaults. `--nats-url` selects the existing test NATS endpoint. The harness creates
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
`private/bruno/<run-id>/`. A partial, duplicated or skipped collection is a failure. Endpoint coverage
counts executed scenarios and successful assertions independently; missing
requests cannot be replaced by a duplicate.

After an intentional native contract change, save the unfiltered `manage showurls`
output and the normal `manage openapi` JSON from the same qualified Community
build. Keep the build/source identity with those artifacts. Public OpenAPI is
not a complete transport inventory: internal Federation and static HEAD routes
still come from `showurls`. The catalog adapter only reads these saved files;
it never launches a command, connects to a service or inspects application source.
From the workspace root, using your own saved paths:

```bash
python3 scripts/export-bruno-catalog.py \
  --showurls notes/bruno-native-showurls.txt \
  --openapi notes/bruno-native-openapi.json \
  --metadata server/tests/bruno/contracts.json \
  --output server/tests/bruno/contracts.json
```

Use `--check server/tests/bruno/contracts.json` instead of `--output ...` to
compare JSON structure without writing; repository formatting may compact arrays,
but value types and array order still have to match. Both native inputs are mandatory. The reviewed catalog
supplies legacy names/source dispatch metadata, not route inclusion or payload
schemas. The adapter recognizes eight explicitly reviewed Node/receiving-key
additions; another unannotated operation is an error requiring review. It also
rejects empty, unparsed or duplicate route inventories, mismatched operation
identities and unresolved native schema references. Output retains reviewed
metadata order for known method/path identities and appends new native operations
in method/path order; metadata controls layout, not inclusion. JSON keys have
stable ordering. Only transitively referenced native schema
components are retained, with JSON response schemas keyed by their documented
status. Non-public routes retain reviewed transport metadata without carrying
forward stale payload schemas.

Then generate the ordinary Bruno requests with
`python3 scripts/bruno_contracts.py` and inspect the changes. This renderer writes
the request files and `scenarios.json`; it does not generate the catalog. Expected
statuses and authority/state assertions remain independently authored in the
renderer and journeys, never learned from a server response or selected from
OpenAPI. The schema for an independently authored success status, including HTTP201,
comes from the native catalog export. `python3 scripts/bruno_contracts.py --check`
checks the catalog, manifest and request-file set. The expanded cases include
JSON/media rejection, typed IDs, paging, real credential/peer authentication,
state preservation on rejected writes, documented response fields and types,
frontend GET/HEAD/ETag behavior and browser activity/all-session logout.
Malformed JSON comes from a file body so the HTTP client's automatic JSON string
encoding cannot turn a syntax-error case into valid JSON of the wrong type.

The Node transport journey creates one disposable Node with HTTP201, switches
and restores the default, deletes/restores the same Node and reads back its
state/address. It leaves the original default selected. Rejected lifecycle writes
read back the actual target Node. The shared Subject policy is not widened into
Tenant administration. Receiving-key requests exercise exact required selectors,
typed quarantine/dual-CAS bodies, ordinary Subject refusal and missing foreign
tuples; these are not successful trust-confirmation cases. The existing native
HTTP/PostgreSQL owners retain successful independent PEM/fingerprint confirmation,
TA/DA policy, CAS/concurrency, retained-record recovery and crypto proof. Bruno
does not duplicate their foreign-peer matrix or claim physical erasure from a
purge request. Inventory failures report missing/extra method/path multiplicities.

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
