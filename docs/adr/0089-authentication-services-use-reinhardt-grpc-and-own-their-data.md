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

- At Aidash's pinned Reinhardt revision `bb2562bc4839250d277a7ceecbf7fe5c18dd82c8`, `reinhardt-grpc` provides service registration and DI, but its standard listener exposes no TLS/mTLS configuration or dedicated client/channel API. The inspected upstream main revision `746e267ffc2edb75ee626d7508d273b526c387d6` has the same gaps. These are observed implementation prerequisites, not reasons to bypass the selected framework.
- Implementation must supply the missing Reinhardt server/client transport support, peer identity handling and service authorization integration before authentication cutover. Certificate issuance and rotation are also required; they are not provided by the existing Workload Identity configuration.
- Service credentials, migrations and independent startup checks must demonstrate ownership boundaries even while PostgreSQL infrastructure is shared.
- This is an accepted target architecture; neither the framework extensions nor service extraction are implemented here.

## Implementation

Authentication is implemented under `enterprise/auth/backend` and Marketplace
under `enterprise/marketplace/backend`, each as a generated Reinhardt project
with an owning app. Their sibling `frontend` directories are generated Pages
projects with independent static builds. Each backend owns its bootstrap/seal
DDL and native migrations; neither starts or migrates Cloud. The shared database
role/FK contract is verified in Authentication's PostgreSQL suite.

Independent Compose profiles now apply bootstrap, owning-service migrations and
ledger sealing before serving with runtime credentials. Cloud's chart and GCP
controller accept the explicit common-auth profile with a private product mTLS
Secret mounted on server and worker. Existing GCIP resources/IDs survive the
runtime-authority switch. Service image builds and releases remain independent.
