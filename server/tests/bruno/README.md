# Real API contracts with Bruno

This collection calls a running Aidash server. It checks the public health and
OIDC configuration responses, anonymous rejection, scoped credentials, workspace
ownership, atomic revision conflicts, task creation and paging, strict JSON,
content types, credential revocation, browser registration, operator approval,
opaque cookies, origin and CSRF enforcement, and server-side logout.

The harness starts the compiled `aidash server` binary with isolated settings and
a unique empty PostgreSQL database. It uses the normal Reinhardt bootstrap and
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
  --binary /absolute/path/to/aidash \
  --postgres-container aidash-test-postgres \
  --postgres-port 54370
```

The existing container must have the project's `pg_jsonschema` extension. Set
`AIDASH_BRUNO_POSTGRES_PASSWORD` when its test password differs from the Compose
default. `--nats-url` selects the existing test NATS endpoint. The harness creates
and removes only its own `aidash_bruno_<run-id>` database and child processes; it
leaves the shared services running.

The CLI receives a temporary JSON environment using `--env-file`. Requests use
Bruno runtime variables for issued credentials and cookies. Automatic cookies
are disabled so anonymous and bearer requests remain explicit. Raw reports,
provider keys, credentials and logs stay in a private temporary directory that
is removed on exit. Sanitized request names, HTTP statuses, check results, source
revision, executable hash and provider call counts are written under
`.ignore/bruno/<run-id>/`. A partial or skipped collection is a failure.

These finite HTTP journeys complement the Rust, browser and cluster acceptance
suites. The collection checks unauthenticated SSE reconnection rejection;
stream delivery and Last-Event-ID replay use the dedicated SSE regression suite.

CLI usage: [Bruno command options](https://docs.usebruno.com/bru-cli/commandOptions).
