# A Web disclosure approval covers only what Aidash sends

Decided on 2026-10-15 for #227. A declared Research provider adds a second hop: Aidash sends a query or a page address to the provider's service, and that service then queries its search engine or reads the page. Aidash sees, and can show a person, only the first hop. A Web disclosure approval therefore approves the exact request Aidash builds and sends to the provider's endpoint. What the service sends onward is governed by the Research account terms that whoever runs the Deployment records for it, which Catalog Approval shows the Tenant, and not by approving each request.

For Page reading, the approval's destination and a Run grant's scope remain the page's host, and the provider is part of the approved digest. Scoping them by the provider's host would let one Run grant cover reading any address through that provider.

Before anything is sent, Aidash refuses page addresses that are literally non-public, as its own page reader does. A provider's endpoint must resolve to a public address unless the Deployment Administrators permit its private origin, as for Keyless endpoints (ADR 0036). Each Document snapshot records the Research provider that read it, and the evidence viewer shows it, because Aidash cannot verify that a provider returned a page's real content.

## Considered Options

- **Scope Page reading approvals by the provider's host**, as Web search approvals are. Rejected for the reason above.
- **Require providers to report their onward requests for approval.** Aidash cannot verify such a report, and an approval it cannot enforce would mislead the approver. Rejected.

## Consequences

- The disclosure approval card (#228) names the Research provider as well as the destination.
- A Run grant for Web search through a declared Research provider covers later queries the approver has not seen, as it already does for Brave.
