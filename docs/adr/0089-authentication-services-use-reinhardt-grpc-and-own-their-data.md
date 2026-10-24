---
status: accepted
---

# Authentication services use Reinhardt gRPC and own their data

Cloud's internal authentication integration uses `reinhardt-grpc` through the Reinhardt facade. The private gRPC interface requires mutual TLS and permissions for each calling service. An ordinary product service may perform its authorized validity and session operations but does not thereby receive Authentication Administrator authority to suspend or restore Aidash Accounts. TLS client identity and request authorization are distinct checks; a router namespace or injected dependency alone grants neither permission.

The Authentication Server, Cloud and Marketplace may share a physical PostgreSQL instance initially. Each service owns separate schemas, database roles and credentials, and migrations. Runtime integration uses service interfaces, without reading or writing another service's tables or creating foreign keys across service ownership boundaries. Product-local identity reference records preserve existing references as described in ADR 0088.

The initial authentication cutover covers Cloud and the Marketplace. Dedicated and Self-hosted Deployments retain their own sign-in paths and their optional Marketplace Connections; an Account Link does not become a Deployment login dependency. The source remains one codebase, with the Community/Enterprise dependency and GCIP placement constraints of ADR 0032 intact.

The standalone Marketplace exposes public listing and detail metadata, including descriptions, prices and Declared Permissions, without requiring sign-in. Library access, purchases, Publisher operations and artifact acquisition require authentication and the applicable local authorization. Public metadata does not grant access to protected artifact contents.

## Considered Options

- **Use shared authentication tables directly from each product.** Rejected: shared infrastructure would then preserve the runtime coupling the extraction is intended to remove.
- **Require separate database instances immediately.** Deferred: schema, credential and migration ownership establish the required service boundary without an immediate infrastructure split.
- **Give all authenticated gRPC clients account-administration authority.** Rejected: authenticating a product service does not authorize it to stop every product's users.
- **Move every Deployment's sign-in to the shared server.** Rejected for the initial extraction: independent Deployment login and optional Marketplace connectivity are existing contracts.

## Consequences

- The pinned Reinhardt revision `b6a006a769ca340c22a51e67fef1bb4be1dc3039` provides the required TLS/mTLS transport and peer identity support. Service-specific source/session/admission authorization remains in the owning adapters; successful certificate authentication alone grants no Account-administration permission.
- Certificate issuance and rotation remain deployment responsibilities. Existing Workload Identity configuration does not supply product mTLS certificates.
- Service credentials, migrations and independent startup checks demonstrate ownership boundaries even while PostgreSQL infrastructure is shared.
- Local transport/PostgreSQL fixtures establish implementation contracts; live GCIP, real-cluster cutover and production certificate rotation remain separate acceptance.

## Implementation

Authentication is implemented under `enterprise/auth` and Marketplace under
`enterprise/marketplace`, each as one full-stack project following actual
`startproject/startapp --with-pages` output. App-owned Pages components and
shared typed `server_fn` declarations connect to the same process's native
services through request/service DI. Each project owns its bootstrap/seal DDL,
Reinhardt migrations, static assets and single product image. Native and WASM share
one Cargo package and the Enterprise workspace lockfile. wasm-pack/Python build
the bundle without Node.js; E2E uses Reinhardt's `fixtures::wasm::e2e` helpers. The shared database
role/FK contract is verified in Authentication's PostgreSQL suite.

Independent Compose profiles now apply bootstrap, owning-service migrations and
ledger sealing before serving with runtime credentials. Cloud's chart and GCP
controller accept the explicit common-auth profile with a private product mTLS
Secret mounted on server and worker. Existing GCIP resources/IDs survive the
runtime-authority switch. Service image builds and releases remain independent.
