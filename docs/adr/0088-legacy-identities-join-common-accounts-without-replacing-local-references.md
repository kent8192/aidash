---
status: accepted
---

# Legacy identities join common accounts without replacing local references

The authentication extraction assigns an Aidash Account to a legacy External Identity lazily, when that Identity first uses the new sign-in flow. The person chooses either a new Account or an existing Account, with fresh proof of both the existing Account and the Identity before attachment, as required by ADR 0084. Matching email addresses do not make this choice. Migration does not preallocate a different Account to every legacy Identity: under the initial no-account-merge rule, that would prevent two such Identities from ever joining the same Account.

The cutover invalidates existing browser and Desktop sign-in sessions and requires a new login. It preserves External Identity UUIDs, issuer/subject pairs, Mappings, Subject Credentials, Personal Tenant references, existing Billing Account ownership references and durable Run origins. Ending an interactive session for migration is Session Revocation, not Identity Disablement or cancellation of admitted work. Before protected operations switch to remote checks, the Authentication Server must be able to validate legacy Run identities, including those that have not yet acquired an Aidash Account.

Product-local identity references remain available for local ownership and authorization. The shared Account and its links are additional records; they do not replace a Mapping's source Identity, combine its permissions with another Identity, or transfer existing ownership. Where local references must outlive extracted authentication records, products retain their own reference records rather than making cross-service foreign keys.

First assignment establishes the historical Account association retained under ADR 0086. It supplies the applicable Account provenance for that Identity's existing durable origins without replacing their Identity UUIDs or local authority. Unlinking later removes a login method, not this history; another Account cannot acquire that Identity in the initial version.

## Considered Options

- **Preallocate an Account for every legacy Identity.** Rejected: without Account merge, this forecloses a later explicitly proven link between two legacy Identities.
- **Replace local Identity UUIDs with Account IDs.** Rejected: this changes existing ownership and provenance rather than merely moving authentication.
- **Keep old interactive sessions valid across the issuer cutover.** Rejected: a fresh login establishes the new Account association and authentication session explicitly.

## Consequences

- Existing Billing Account ownership remains Identity-based until a separate ownership decision changes it; new personal Marketplace ownership follows ADR 0083.
- Migration acceptance must cover unassigned legacy Identities with admitted work, not just accounts that have logged in after cutover.
- This records an accepted target migration, not an applied migration.
