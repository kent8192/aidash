---
status: accepted
---

# Marketplace enforces acquisition without promising local-copy DRM

The standalone Marketplace enforces Entitlements at its own artifact acquisition and update interfaces. It authenticates the requester and verifies the right to the requested plugin version from authoritative service records; a Self-hosted caller's assertion that a purchase exists is not evidence of that right. A Marketplace Connection identifies the registered Deployment, not an unmodified runtime.

Registry obtains the artifact, validates its contents and immutable version/digest, and stores a Tenant-local copy. The Agent version carries that pinned copy under ADR 0072 and remains subject to its Catalog Approval and local authority rules. Existing same-Deployment Shared Package publication and installation are distinct from this Marketplace distribution contract.

Once an executable local copy has been delivered to a Self-hosted Deployment controlled by the person running it, Aidash does not promise to prevent that person from removing execution, expiry or Recall checks or copying the content. Signatures protect provenance and integrity for a verifier that performs the checks; neither signatures, Connection credentials nor mTLS compel that person to run the verifier. Aidash-operated Cloud and Dedicated can enforce runtime checks because Aidash controls the runtime. This distinction limits technical enforcement, not the Entitlement's stated rights.

Where strict metering or time-based service access must be enforced against an untrusted person running a Deployment, the protected core operation must remain behind a remote MCP or API service that enforces its own authorization. Local delivery must not be advertised as providing that guarantee. The Marketplace itself is not required for each execution of a previously installed local copy; any remote service used by a plugin remains a separate dependency of that plugin.

## Considered Options

- **Trust local Entitlement checks as proof against a person running a modified Self-hosted Deployment.** Rejected: that person controls the process, database and runner executing those checks.
- **Treat artifact signatures or mTLS as proof of an unmodified runtime.** Rejected: possession of a credential or successful verification does not compel subsequent local behavior.
- **Move every plugin to remote execution.** Rejected as the general distribution model: it abandons the accepted local pinned-copy and offline-execution contracts. Remote protected functionality is available when its enforcement properties are required.

## Consequences

- Acquisition and update authorization must protect the actual artifact storage and delivery path, not only the visible catalog or client UI.
- Unauthorized redistribution or execution of an already delivered copy cannot be represented as solved by this extraction.
- This records the accepted trust boundary; artifact distribution and Entitlement enforcement are not implemented here.
