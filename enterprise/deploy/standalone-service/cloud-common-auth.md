# Cloud common-auth deployment

Provision Authentication as an independent release, import retained source
Identities, register the exact `cloud` confidential client callback and issue a
Cloud product certificate before cutover. Its client URI SAN must match the
configured product peer. Upstream GCIP pool access belongs to Authentication.
Select common auth through the runtime descriptor. Retain existing GCIP
Terraform resources: removing pool declarations can delete upstream users. The
controller preserves those IDs/resources while selecting the new authority.
Dedicated/Self-hosted retain independent sign-in. The GCP controller
keeps random preview origins on their existing managed GCIP path.

For a fixed-origin Cloud environment, the runtime Secret Manager JSON accepts
this descriptor alongside existing provider credential keys. Replace every
example/PEM placeholder with actual provisioned values:

```json
{
  "AIDASH_OIDC_CLIENT_ID": "cloud",
  "AIDASH_OIDC_CLIENT_SECRET": "REPLACE_WITH_AT_LEAST_32_RANDOM_CHARACTERS",
  "dashboard": {
    "oidc": {
      "issuer": "https://auth.example.com",
      "public_origin": "https://test.aidash.run"
    },
    "common_auth": {
      "grpc_endpoint": "https://authentication.internal:9091",
      "server_domain": "authentication.internal",
      "source_issuers": {
        "https://securetoken.google.com/PROJECT/tenants/shared-pool": ""
      }
    }
  },
  "common_auth_tls": {
    "ca.pem": "REPLACE_WITH_PEM_CA_CERTIFICATE",
    "cloud.pem": "REPLACE_WITH_PEM_CLIENT_CERTIFICATE",
    "cloud-key.pem": "REPLACE_WITH_PEM_CLIENT_PRIVATE_KEY"
  }
}
```

`public_origin` must match the environment exactly. Source issuer keys retain
their original GCIP pool suffix; values are empty for unrestricted pools or name
the exact product Tenant binding. The private gRPC hostname must resolve from
Cloud and match the issued server certificate SAN. Replace the PEM placeholders
with complete PEM strings, including their BEGIN/END lines.

The GCP controller sends `app-common-auth` through kubectl stdin, containing
`settings.toml` and three PEM files. `commonAuth.existingSecret` mounts these
read-only on **both server and worker**, since workers also verify admission and
source lifecycle. The managed TOML profile overlays the image's base settings,
selects `REINHARDT_ENV=managed-common-auth`, and references the existing
`${AIDASH_OIDC_CLIENT_SECRET}` environment variable. PEM/private keys never enter
Helm values, Terraform arguments or environment variables. Direct GCIP and
common auth are mutually exclusive.

For a manual Cloud Helm release, create the same four-key Secret first. Its TOML
has `[dashboard.oidc]`, `[dashboard.common_auth]` and
`[dashboard.common_auth.source_issuers]` as in
`server/settings/base.example.toml`. The TLS paths are `/run/auth-tls/ca.pem`,
`/run/auth-tls/cloud.pem`, and `/run/auth-tls/cloud-key.pem`. Set
`commonAuth.existingSecret` and provide the OIDC client-secret environment
variable through the existing runtime Secret. No key material belongs in values.

TLS files load at startup. Renew and roll both writers before their certificates
expire; the GCP settings-reload path rolls both. See [certificate rotation](README.md#certificate-issuance-and-rotation)
for overlapping-CA rotation. Removing common auth to bypass retained stops is not
a valid rollback. Back up Authentication separately and retain its stop epochs.

Auth/Marketplace images remain outside Cloud's release bundle and lifecycle.
Build and install their [independent chart releases](README.md). Configuration
and rendering tests do not establish live GCIP, cluster issuance or production
cutover acceptance.
