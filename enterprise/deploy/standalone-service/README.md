# Independent service releases

Install this chart separately for authentication and Marketplace. Their image,
configuration, replicas and migration commands are independent; neither release
starts a Cloud server or worker. Each product image bundles its Pages WASM
assets and native executable; the same service serves the browser, typed
`server_fn`, BFF and external API routes.

Build images from the repository root:

```sh
docker build -f enterprise/auth/Dockerfile -t aidash-auth:local .
docker build -f enterprise/marketplace/Dockerfile -t aidash-marketplace:local .
```

In production use digest-pinned private-registry image references. Supply existing
Secrets through `secretVolumes` and `secretEnvironment`, rather than placing secret
values in Helm values, Terraform state arguments or source files. See each service
crate's README for its exact configuration and migration commands.

Run migrations with each schema's owner credential before rolling its service.
Runtime credentials receive only the service schema's data privileges. PostgreSQL
schemas, roles and credentials are externally provisioned; sharing a database
instance does not mean sharing a role or running Cloud migrations.
For the corresponding disposable local bootstrap/migration/seal lifecycle, use
[the standalone Compose profiles](../local/README.md), not root `compose.yaml`.

For authentication set port 8081, enable the private gRPC port 9091, mount its
configuration and TLS files, and pass `--config /run/aidash/config/auth.toml serve`.
For Marketplace set port 8082 and the `AIDASH_MARKETPLACE_*` settings, including
`AIDASH_MARKETPLACE_TRUSTED_PROXY_IPS` (and Auth's `trusted_proxy_ips`) with the edge's
IPs or CIDR networks, for example the cluster Pod CIDR when the edge Pods' IPs change.
Everything in a trusted network may assert forwarded HTTPS, so restrict ingress to the
edge Pods with the chart's NetworkPolicy (below). Place HTTP behind the deployment's
HTTPS edge. The gRPC Service is internal; authenticate and authorize peers even on the
cluster network.

## Ingress NetworkPolicy

`networkPolicy.enabled` renders one `NetworkPolicy` (`policyTypes: [Ingress]`) selecting
this release's Pods. It is off in `values.yaml` so an upgrade cannot silently cut off
callers, and on in both examples. `networkPolicy.ingress` lists rules, each a `port`
(`http`, or `grpc` when `grpc.enabled`) and a non-empty `from` of peers. A peer is a
`podSelector` and/or `namespaceSelector`; the chart refuses to render an empty `from`, an
empty selector (which would match every Pod or namespace), any other peer type, or a
port the release does not serve. A `podSelector` alone selects Pods in the release's
namespace; add a `namespaceSelector` when the peer lives elsewhere. A policy that allows
no port at all is refused too.

| Port                 | Peers admitted in the examples                                                                                                                        | Why                                                                                                                                                                                                                                                                                                                                                                                                                              |
| -------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `http` (Auth)        | Edge Pods, `aidash.run/edge=env` (as `backendIngress.podSelectors` in `deploy/helm/aidash` and `EDGE_PODS` in `enterprise/infra/gcp/control/kube.py`) | Browsers, Marketplace's `/oauth/token`, `/oauth/introspect` and `/oauth/revoke` calls (public `AIDASH_AUTH_ISSUER` origin) and Cloud's OIDC flow arrive through the edge.                                                                                                                                                                                                                                                        |
| `http` (Marketplace) | The same edge Pods, plus the in-cluster Billing, enrollment and Recall integration Pods                                                               | Browsers and remote Registry Connections (`/api/marketplace/downloads`, `/recalls`) use the HTTPS edge. With an HTTPS origin the service answers 400 to plain HTTP except `GET /healthz` and the three `/internal/` POST routes (`billing/events`, `enrollments`, `recalls`). The public edge denies `/internal/`, so the integrations call the Service directly with their Bearer secrets and are admitted only by this policy. |
| `grpc` (Auth)        | Cloud server and worker Pods (`component in (server, worker)`) and the separate Account-administration workload                                       | Only Cloud and administration use the private gRPC/mTLS port; Marketplace does not.                                                                                                                                                                                                                                                                                                                                              |

Placeholders to replace before installation, because the standalone services are not
yet deployed from `enterprise/infra/gcp` (Issue #516): the Auth `grpc` peers name
`REPLACE_WITH_CLOUD_NAMESPACE`, `REPLACE_WITH_ADMINISTRATION_NAMESPACE` and
`aidash.run/authentication-admin=REPLACE_WITH_ADMINISTRATION_WORKLOAD`; the Marketplace
integration peer names `REPLACE_WITH_INTEGRATIONS_NAMESPACE` and
`aidash.run/marketplace-integration=REPLACE_WITH_BILLING_ENROLLMENT_RECALL_WORKLOAD`.
They match nothing (fail closed) until set. Plain HTTP to the three `/internal/` POST
routes is the existing in-cluster design (in-cluster TLS is tracked in Issue #516); a
Bearer secret is the sole credential there and this policy limits who can reach it, so
keep the policy enabled. Kubelet probes (`/healthz` readiness, TCP
liveness on the http port) come from the node, which GKE Dataplane V2 always admits, so
no rule is needed for them.

Install one release per product using `auth.values.yaml` or
`marketplace.values.yaml`. Assets reside at `/app/pages` in each product image;
no additional proxy image, frontend release or frontend-to-backend URL is needed.
The public HTTPS edge must deny `/internal/` and forward the product origin to
its single HTTP service. Both serving containers run non-root with read-only
root filesystems.

## Helm examples and native startup proof

Use `auth.values.yaml` and `marketplace.values.yaml` as non-secret
examples. Set real public origins, configuration/credential Secret names, service
account and certificate issuer/SAN before installation. The Authentication
configuration Secret contains `auth.toml`; its runtime Secret uses the mounted
filenames in `enterprise/auth/auth.example.toml`. Migration-owner
credentials are supplied only to the explicit migration command, never to the
serving Deployment. The Marketplace runtime Secret contains `database-url` and
its separately named client/resource/Billing/enrollment/Recall secret files.

```sh
helm lint enterprise/deploy/standalone-service \
  -f enterprise/deploy/standalone-service/auth.values.yaml \
  --set-string image="$AUTH_IMAGE_DIGEST"
helm template authentication enterprise/deploy/standalone-service \
  -f enterprise/deploy/standalone-service/auth.values.yaml \
  --set-string image="$AUTH_IMAGE_DIGEST"
# Use marketplace.values.yaml for the independent Marketplace release.
```

A local entry-point smoke test starts a disposable PostgreSQL database, applies
each production bootstrap file and each binary's own migrations, then uses only
runtime credentials. It starts Marketplace with Authentication absent, reads
public metadata, starts Authentication, checks OIDC discovery/Pages and the BFF's
PKCE redirect/HttpOnly cookie, then stops Marketplace while Authentication stays
healthy. It starts no Cloud process, worker, NATS or Cloud migration. It requires
Docker, OpenSSL and already-built native binaries and both products' Pages assets:

```sh
cargo build --locked --manifest-path enterprise/Cargo.toml \
  -p aidash-auth-server -p aidash-marketplace-server --bins
python3 scripts/test-standalone-services.py \
  --auth-binary enterprise/target/debug/aidash-auth-server \
  --marketplace-binary enterprise/target/debug/aidash-marketplace-server \
  --auth-assets enterprise/auth/dist \
  --marketplace-assets enterprise/marketplace/dist
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

On GKE, provision Authentication's own Kubernetes/Google service accounts and
Workload Identity binding, and grant its configured GCIP pools the tenant-scoped
account-read permission required by Application Default Credentials. These are
separate from Cloud's product mTLS identity. Preserve existing pool IDs/users
while transferring infrastructure ownership and retiring obsolete direct-lookup
grants. See [Cloud common-auth configuration](cloud-common-auth.md) for the
runtime descriptor; changing runtime authority must not delete upstream pools.
