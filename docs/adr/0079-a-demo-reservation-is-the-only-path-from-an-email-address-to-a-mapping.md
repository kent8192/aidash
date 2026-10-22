# A Demo Reservation is the only path from an email address to a Mapping

Decided on 2026-10-20 for #432. Demo data names real people who should open an Environment and find themselves already inside its Demo Tenants. A Mapping links an External Identity, never an email address, and an Environment's GCIP pools are created fresh for each incarnation. So nobody can know a person's External Identity before that person first signs in.

A Demo Reservation, which only loading Demo data creates, promises that the first External Identity signing in with a given GCIP-verified email address is mapped to a named subject of a Demo Tenant, as a Tenant Administrator or a member. Loading maps an External Identity at once when exactly one with that verified email address already exists. A Reservation never reaches an External Identity whose GCIP Tenant has a Tenant Binding, and it adds only the Mapping: Personal Tenant provisioning and the dashboard's default selection stay as the Tenancy Rules say. The first External Identity spends it, so a second identity of the same person gets nothing.

## Considered Options

- **Map existing identities on a later load.** The person signs in first, and the next load finds their External Identity by email. Sign-in stays unchanged, but the person waits for another deploy before seeing any Demo Tenant. Rejected.
- **Issue Invitations.** Each person would open a link and make a Registration Request, which someone must approve. A fresh Environment has nobody to approve the first one. Rejected.
- **Name External Identities in the secret.** Their subjects are unknown until the pools exist. Rejected.

## Consequences

- Sign-in gains one step that looks up Reservations by verified email. It runs only while `[demo_data] enabled`, so a production Deployment, which never enables it, holds no Reservation and never takes this step.
- A Reservation never grants Deployment Administrator authority.
- Removing a person from the secret does not remove a Reservation or a Mapping already made: loading only adds.
