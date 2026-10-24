# Local independent services

`enterprise/compose.yaml` is a separate Compose project from the root Cloud
stack. Its `auth` and `marketplace` profiles share only PostgreSQL infrastructure.
Every schema, owner/runtime role, password and migration ledger is service-owned.
Neither profile starts Cloud, the dashboard, a worker, NATS or Cloud migrations.
The PostgreSQL initialization hook executes the production bootstrap DDL; each
profile then runs its native migration command, seals its ledger and starts its
runtime. Serving containers never mount an owner/admin database credential.

From the repository root, with Docker Compose and OpenSSL:

```sh
python3 scripts/prepare-standalone-compose.py
# Either profile works by itself, using the same private local credentials.
docker compose -f enterprise/compose.yaml --profile auth up --build -d --wait
docker compose -f enterprise/compose.yaml --profile marketplace up --build -d --wait
```

`cargo make auth-dev`, `cargo make marketplace-dev` and `cargo make standalone-down`
wrap these operations. Auth serves its bundled Pages at
<http://127.0.0.1:18081/sign-in>; Marketplace's separate Pages container serves
<http://127.0.0.1:18082/>. Auth's mTLS listener is published only on
`127.0.0.1:19091`. PostgreSQL and Marketplace's backend have no host port.

Default GCIP settings are placeholders. They support startup, discovery, public
catalog and PKCE redirect checks; they cannot establish live upstream sign-in.
Before provider acceptance, configure a real project, pool, public Firebase API
key and auth domain, including authorized domains and provider callbacks:

```sh
python3 scripts/prepare-standalone-compose.py \
  --gcip-project YOUR_PROJECT --gcip-tenant YOUR_POOL \
  --gcip-api-key YOUR_PUBLIC_FIREBASE_API_KEY \
  --gcip-auth-domain YOUR_PROJECT.firebaseapp.com
docker compose -f enterprise/compose.yaml --profile auth up -d --force-recreate
```

Preparation retains passwords, OAuth secrets and signing/encryption keys. It
writes only `private/standalone-compose`, a host-private `0700` directory.
Individual files are readable through per-file Compose secret mounts by the
unprivileged container UID; the credential directory is never mounted whole.
The development CA/certificates last 30 days. The generated Cloud client has only
`spiffe://aidash/cloud`, without Account administration permission. Real issuance
and rotation use the [standalone chart procedure](../standalone-service/README.md#certificate-issuance-and-rotation).

The Marketplace BFF's local loopback relay preserves the browser's exact
`127.0.0.1` issuer while forwarding to Authentication over the Compose network.
It has no Authentication startup dependency: public metadata works while Auth
is absent and sign-in refuses admission. Production uses a reachable HTTPS
issuer and needs no relay. Only opaque Product cookies reach the browser.

Set `AIDASH_AUTH_PORT`, `AIDASH_MARKETPLACE_PORT` and `AIDASH_AUTH_GRPC_PORT` before
both preparation and Compose commands to change host ports. Preserve them on
restart because callbacks are exact. `AIDASH_STANDALONE_DIRECTORY` selects another
prepared private directory (use an absolute path); `AIDASH_STANDALONE_PROJECT`
selects the Compose project.

For faster local native compilation, build dev-profile runtime images and use
`up --no-build`. Dockerfiles otherwise default to release builds:

```sh
docker compose -f enterprise/compose.yaml --profile auth --profile marketplace \
  build --build-arg CARGO_PROFILE=dev authentication marketplace marketplace-web
docker compose -f enterprise/compose.yaml --profile auth --profile marketplace \
  up --no-build -d --wait
```

Check independence by stopping one process while the other remains healthy:

```sh
docker compose -f enterprise/compose.yaml --profile auth stop authentication
curl --fail http://127.0.0.1:18082/api/marketplace/catalog
docker compose -f enterprise/compose.yaml --profile auth start authentication
docker compose -f enterprise/compose.yaml --profile marketplace stop marketplace
curl --fail http://127.0.0.1:18081/healthz
```

`down` preserves the database volume and private files. Never regenerate passwords
while keeping a database initialized with them. For an intentional disposable
reset, use this project's `down -v`, then remove only its matching private
directory. Bootstrap hooks run only on an empty PostgreSQL data volume.

`python3 -m unittest discover -s scripts -p test_standalone_deployment.py` renders
both profiles and every standalone Helm example. It protects migration/seal
ordering, credential mounts, source paths and non-root/read-only deployment.
Native service acceptance and PostgreSQL ownership have their own tests.

`python3 scripts/test-standalone-compose.py --build` builds all four product
images, provisions a disposable Compose project, checks both independent
startups and the real proxy/PKCE transport, then removes its project and volume.
Without `--build` it uses already-built local images. Full CI runs this container
acceptance in addition to native entry-point and configuration contracts.
