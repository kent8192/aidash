# A Scratch Space reaches its External services through its proxy, and no Agent binds them

Decided on 2026-11-16 (#738). ADR 0111 made External services available to a Scratch Space instead of bound to a Run, and ADR 0136 gave each process in an Execution Pod a proxy credential for its Run. Until now the only admission for an External service was the legacy `/v1/<operation-token>/<service>/<path>` route. Its token belonged to one Runner operation, its limits were per operation, and it checked the Agent's own binding to the service. No Pod process could use it.

Code in an Execution Pod now calls an External service as plain `http://services.aidash.internal/<service>/<path>`. The request travels through the proxy that the process already has, with that process's own credential. It therefore needs no environment variable, token or header of its own. `.internal` is reserved for private use, so no public host can take the name. The gateway sees the reserved host and admits the request as an External service request, never as a Host connection. It then attaches the service's Tenant Secret and forwards the request over HTTPS. The hop between the Pod and the gateway is plain HTTP inside the cluster. It carries the request and the response, never the secret, and NetworkPolicy lets the Pod reach only the gateway.

A service is usable from a Scratch Space when two things hold. The service must be in the Scratch Space's settings. And a Catalog Approval for the Scratch Space's Node, limited to no subjects or Agents, must cover one of its versions. The newest such version is used. An approval limited to certain subjects or Agents does not count, because code in a shared Pod can claim another Run's credential (ADR 0136). Each request is refused when any of these holds:

- its Run is revoked;
- its Scratch Space is not Active;
- its Tenant or Node is deleted;
- the service has no usable approval; or
- the method is not allowed.

The gateway's periodic check of an open request rechecks the same conditions for the admitted version. A withdrawn approval, or a service taken off the Scratch Space, therefore stops a request already under way, as the Use Recheck stops a Run (ADR 0122). A refusal names the reason and the services the Scratch Space can use now.

The Credential Broker sees the Scratch Space as the user of the Tenant Secret (`SecretSubject::ScratchSpace`). The Egress request record names the Run the credential claims. Limits are the Scratch Space egress allowance plus the service's own daily share, with no request count (ADR 0054). Admission reads no Run row and writes no Authorization Decision: the decisions behind it are the Catalog Approval and the Scratch Space settings, and both are recorded where they are made.

Agent definitions no longer bind External services. New registrations with such a binding are refused, and drafts lose theirs. Registered versions keep theirs, because their digests are pinned, but nothing follows them: Run start leaves them out of the Binding snapshot without loading their definitions, and neither Approval Review nor Marketplace publication counts them as dependencies. Runner operations outside an Execution Pod (Reference extraction, verification), preparation commands without a Run, and Live preview pages in a viewer's browser reach no External service.

## Considered Options

- **Pass a gateway URL and token in new environment variables.** Code would have to read them and add a header, and the token would show up in process listings and logs. Rejected.
- **Intercept TLS with an Aidash certificate authority in the Pod.** The hop to the gateway would be encrypted. But every image and language runtime would have to trust that authority, which breaks "any ordinary image works" (ADR 0111). Rejected.
- **One host name per service.** Registry ids are case-sensitive and up to 100 characters long, so not all of them fit in a DNS label. Rejected.
- **Keep the Agent's binding as a declaration, or honour Agent-limited approvals.** A Run cannot be held to either, since code can claim another Run. Rejected.
- **Pin a version when the service is added to the Scratch Space.** A newly approved version would then need someone to add the service again. The Tenant Secret's own sealed hosts already bound where the secret can go. Rejected.

## Consequences

- Narrowing who may use an External service is done with its Catalog Approval, not with Scratch Space membership or the Agent.
- A Live preview page that needs a service calls its own server in the Pod, which calls the service.
- The per-operation budgets, the Live preview egress sessions and the `/v1` and Bearer `/v2/services` routes are removed.
- Amends ADR 0054, ADR 0111 and ADR 0136.
