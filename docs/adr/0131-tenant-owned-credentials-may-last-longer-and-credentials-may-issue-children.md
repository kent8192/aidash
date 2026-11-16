# Tenant-owned credentials may last longer, and credentials may issue Child Credentials

Decided on 2026-11-12 in #876. Supersedes ADR 0063's consequences that "a Subject Credential cannot issue another credential" and that lifetimes stay between one second and 30 days "with no setting".

**Lifetime.** A credential that a Tenant owns, meaning one issued under `credential.issue` for a service or agent subject, may last up to a Deployment-configured maximum. The maximum defaults to 90 days and cannot exceed 365 days. A Tenant may set a shorter maximum for itself. Lowering the maximum also shortens existing credentials, and those that end because of it are shown with that reason. A self-issued credential (`credential.create`) still lasts at most 30 days.

**Child Credential.** Presenting a Subject Credential may issue a Child Credential for the same subject. A Child Credential expires no later than its parent and ends when its parent ends. It carries the parent's authority, because Subject Credentials have no scopes yet. A Child Credential cannot issue another credential. One parent may hold at most 100 active Child Credentials, and they do not count toward a Mapping's 20 self-issued credentials.

**Rotation without a browser.** A credential of a Tenant-owned subject that the policy grants `credential.issue` may issue Tenant-owned credentials. What it issues belongs to the Tenant and survives the issuer's revocation. A self-issued credential and a Child Credential cannot issue Tenant-owned credentials. A new self-issued credential, other than a Child Credential, still needs a browser session. Each of these rules is an explicit check in the issue path.

**Telling a caller why.** A refused credential keeps its status code (401 when expired or revoked, 403 when its Mapping is disabled, 503 when the Authentication Server cannot answer). The body adds a `reason`: `expired`, `revoked`, `parent_ended`, `mapping_disabled`, `identity_disabled` or `account_suspended`. The dashboard's credential lists show credentials that expire within seven days, and those ended by a Mapping disablement or a lowered maximum.

## Considered Options

- **Keep 30 days fixed.** Rejected: unattended automation needs a person to rotate it in a browser every month.
- **Lengthen self-issued credentials too.** Rejected: a self-issued credential acts as the person, and a short life limits what a leaked one can do.
- **Let a credential exchange itself for a fresh one.** Rejected: that makes the lifetime unbounded. It would also need a separate absolute ceiling, and a policy-granted rotation service already covers the need.
- **Let a Child Credential or a self-issued credential issue Tenant-owned credentials.** Rejected: a leaked short-lived token could grow into long-lived tokens that outlive the person.
- **Scopes for Child Credentials now.** Deferred to a separate Issue: scopes change every policy decision.
- **Email before expiry.** Deferred to a separate Issue: no user mail is sent yet.

## Consequences

- A Tenant can run unattended rotation as a service subject without a browser.
- A CI job can receive a short-lived Child Credential that dies with the credential it came from.
- Status codes stay compatible, and clients that read `reason` can tell why a credential stopped working.
