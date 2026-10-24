# Independent service releases

Install this chart separately for authentication and Marketplace. Their image,
configuration, replicas and migration commands are independent; neither release
starts a Cloud server or worker. A frontend release can serve
`enterprise/marketplace/frontend` and proxy its API/auth routes to Marketplace.

Build images from the repository root:

```sh
docker build -f enterprise/auth/backend/Dockerfile -t aidash-auth:local .
docker build -f enterprise/marketplace/backend/Dockerfile -t aidash-marketplace:local .
docker build -t aidash-auth-web:local enterprise/auth/frontend
docker build -t aidash-marketplace-web:local enterprise/marketplace/frontend
```

In production use digest-pinned private-registry image references. Supply existing
Secrets through `secretVolumes` and `secretEnvironment`, rather than placing secret
values in Helm values, Terraform state arguments or source files. See each service
crate's README for its exact configuration and migration commands.

Run migrations with each schema's owner credential before rolling its service.
Runtime credentials receive only the service schema's data privileges. PostgreSQL
schemas, roles and credentials are externally provisioned; sharing a database
instance does not mean sharing a role or running Cloud migrations.

For authentication set port 8081, enable the private gRPC port 9091, mount its
configuration and TLS files, and pass `--config /run/aidash/config/auth.toml serve`.
For Marketplace set port 8082 and the `AIDASH_MARKETPLACE_*` settings. Place HTTP
behind the deployment's HTTPS edge. The gRPC Service is internal; authenticate and
authorize peers even on the cluster network.

Install a third release for the Marketplace frontend with
`-f marketplace-web.values.yaml --set image=<frontend-image-digest>`. Set
`environment.MARKETPLACE_BACKEND_URL` to the backend Service's private HTTP URL.
The frontend listens on 8080 and needs the two supplied ephemeral directories:
Nginx renders its configuration into `/etc/nginx/conf.d` and keeps its PID/cache
under `/tmp`. The rest of its filesystem remains read-only, with the same
unprivileged UID and group as the backend releases. Authentication's service
image already bundles its own independent web assets.

## Helm examples and native startup proof

Use `auth-backend.values.yaml` and `marketplace-backend.values.yaml` as non-secret
examples. Set real public origins, configuration/credential Secret names, service
account and certificate issuer/SAN before installation. The Authentication
configuration Secret contains `auth.toml`; its runtime Secret uses the mounted
filenames in `enterprise/auth/backend/auth.example.toml`. Migration-owner
credentials are supplied only to the explicit migration command, never to the
serving Deployment. The Marketplace runtime Secret contains `database-url` and
its separately named client/resource/Billing/enrollment/Recall secret files.

```sh
helm lint enterprise/deploy/standalone-service \
  -f enterprise/deploy/standalone-service/auth-backend.values.yaml \
  --set-string image="$AUTH_IMAGE_DIGEST"
helm template authentication enterprise/deploy/standalone-service \
  -f enterprise/deploy/standalone-service/auth-backend.values.yaml \
  --set-string image="$AUTH_IMAGE_DIGEST"
# Use marketplace-backend.values.yaml for the independent Marketplace release.
```

A local entry-point smoke test starts a disposable PostgreSQL database, applies
each production bootstrap file and each binary's own migrations, then uses only
runtime credentials. It starts Marketplace with Authentication absent, reads
public metadata, starts Authentication, checks OIDC discovery/Pages and the BFF's
PKCE redirect/HttpOnly cookie, then stops Marketplace while Authentication stays
healthy. It starts no Cloud process, worker, NATS or Cloud migration. It requires
Docker, OpenSSL and already-built native binaries/Authentication Pages assets:

```sh
cargo build --locked --manifest-path enterprise/Cargo.toml \
  -p aidash-auth-server -p aidash-marketplace-server --bins
python3 scripts/test-standalone-services.py \
  --auth-binary enterprise/target/debug/aidash-auth-server \
  --marketplace-binary enterprise/target/debug/aidash-marketplace-server \
  --auth-assets enterprise/auth/frontend/dist
```

The Authentication PostgreSQL suite separately verifies every service role's
cross-service read/write denial, sealed migration history and absence of
cross-schema foreign keys. Its signed GCIP fixtures and real mTLS transports do
not establish live GCIP provider configuration or cluster certificate issuance.

## Certificate issuance and rotation

The optional Certificate resource integrates an already-installed cert-manager
controller and trusted Issuer/ClusterIssuer. Installation of that controller and
issuer remains infrastructure provisioning, not an implicit runtime action.

Use a DNS SAN for the authentication server name checked by Cloud. Give Cloud's
client certificate the configured service URI SAN, such as `spiffe://aidash/cloud`,
and `client auth` usage. Give authentication administration a separate URI SAN,
credential and tightly controlled workload. Merely trusting the same CA does not
give a Cloud service permission to administer Accounts.

Configure server/client certificates with `privateKey.rotationPolicy: Always`
and finite `duration`/`renewBefore`. The current service/channel construction loads
PEM files at startup: after a certificate Secret renews, roll the affected service
or Cloud workload before the old certificate expires. Rotation is incomplete
until the new connection succeeds and wrong/expired peer credentials are refused.

For CA rotation distribute overlapping old/new trust roots first, issue and roll
leaf credentials under the new CA, verify all peers, then remove old trust and
roll again. Do not overwrite the only trusted root before both sides can connect.
No deployment, certificate issuance or credential rotation is performed merely by
adding these chart files.
