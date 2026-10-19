# BYOK Key Material lives in GCP Secret Manager, write-only for the application host

Decided on 2026-10-08 (#135). Aidash Cloud stores tenant Key Material in GCP Secret Manager, with one secret per Provider Credential. The application host's service account may create secrets and add, disable and destroy versions. It never holds `secretmanager.versions.access`. Read access belongs only to the credential broker identity (#137). A single-VM K3s host shares one service account across all Pods, so read access must sit on a separate identity for a host compromise to fall short of key exfiltration.

## Considered Options

- KMS envelope encryption with ciphertext in PostgreSQL. Rejected in favour of Secret Manager's managed versioning, per-secret IAM and data-access audit logs. It would also need the host to call decrypt, which recreates the shared-identity problem.
- Per-tenant environment variables. Rejected: they need restarts, cannot be self-served and stay node-global.

## Consequences

- PostgreSQL holds only metadata, a Fingerprint, last4 and the secret's resource name. Records pass through a `pending` state before the Secret Manager write, so a reconciler can find orphans by scanning PostgreSQL alone.
- Superseded in part on 2026-10-09 by ADR 0006. This consequence originally read: self-hosted nodes without a Provider Credential Store keep `credential_env` and cannot use Provider Credentials. `credential_env` is now removed for model providers (#158), self-hosted nodes use the encrypted PostgreSQL Store (#157, ADR 0008), and a node without a Store cannot make model-provider calls.
- A compromised host can still overwrite or destroy Key Material (tampering and denial of service). It cannot read Key Material.
- Provider Credential secrets live in a dedicated BYOK project, separate from the shared application project. `secretmanager.secrets.create` is authorized on the parent project, so an IAM condition on the secret-name prefix cannot scope it. Isolating the project lets the runtime service account hold an unconditioned create permission there without being able to create secrets elsewhere. Its version and delete operations remain conditioned on each environment's name prefix, and the data-access audit logs cover only BYOK secrets.
- The deploy automation identity must not be able to escalate to Key Material reads. Bootstrap, run by a human, defines the BYOK roles once and grants broker read access. In the BYOK project, the deploy identity can only `setIamPolicy` under a `modifiedGrantsByRole` condition, which limits it to the create and manage roles, and it cannot edit custom roles. Compromising the deployment pipeline therefore exposes only keys entered after the compromise, which pass through the compromised server. Keys already stored stay unreadable.
- The deploy pipeline remains part of the trust root for v1 (decided on 2026-10-09, #151). Two paths stay open: the shared-project `roles/iam.serviceAccountAdmin` grant would let the deploy identity impersonate the broker, and the deploy identity could redeploy the broker image. An IAM deny policy blocks the first path. Protected deploy environments, digest-pinned broker images and alerts guard against the second. A separately deployed credential enclave is deferred. Broker service accounts and their read binding are created by the human-run bootstrap and implemented in #137.
