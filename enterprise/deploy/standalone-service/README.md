# Independent service releases

Install this chart separately for authentication and Marketplace. Their image,
configuration, replicas and migration commands are independent; neither release
starts a Cloud server or worker. A frontend release can serve
`enterprise/marketplace-web` and proxy its API/auth routes to Marketplace.

Build images from the repository root:

```sh
docker build -f enterprise/crates/aidash-auth-server/Dockerfile -t aidash-auth:local .
docker build -f enterprise/crates/aidash-marketplace-server/Dockerfile -t aidash-marketplace:local .
docker build -t aidash-marketplace-web:local enterprise/marketplace-web
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
