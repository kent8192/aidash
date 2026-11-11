---
status: accepted
---

# Whoever may run an Agent can have its Run read the Reference files of its approved Sources

Decided on 2026-11-05 for #719. A Reference file used to be readable only by its uploader, so an Agent version shared across a Tenant still failed with `404 reference unavailable` for anyone else. Run admission also asked every subject for `reference.read`, including the Agent, and the default `members` policy has no such action. Now a Run may read the Reference files of a Source when the Agent version binding that Source is approved in the Catalog and the person may run it. The Agent passes through the same rule. Reading happens only inside that Run. Direct download stays with the uploader.

Registering a Source and having it approved is the moment a person makes its files available to the Agent's users, so registration refuses a file the registrant cannot read. Reference Documents and their images already work this way: anyone authorized to run an Agent can cause it to use what is attached to it.

## Considered Options

- **Keep Reference files private to the uploader.** Nothing is ever disclosed by accident, but a project's documents cannot serve a team, and scheduled or delegated Runs under another subject fail. Rejected.
- **Add a Tenant-wide shared document store with its own permissions.** Clearer ownership and sharing, but a second permission model alongside Catalog Approval, for no case that approval does not cover. Rejected.
- **Grant `reference.read` to `members`.** The smallest change, but it would let every member read every Reference in the Tenant, not only those of Agents they may run. Rejected.

## Consequences

- A person who runs the Agent can have the model quote its documents, or read them through their Run's file routes, even without the right to download them. This boundary limits direct access only.
- Files stay usable after the uploader leaves the Tenant. A Tenant Administrator stops them by withdrawing the Source's Catalog Approval. The Run's file routes then refuse as its worker stops, because both run the Use Recheck over the whole Binding (#788). Withdrawal does not reach a copy already made; only revoking the file does. A subject the Tenant's policy grants `reference.delete` can also revoke a file after its uploader leaves, which reaches copies already made in Working Areas (#826).
- Revoking one file refuses new Runs of every Agent version that binds it, and the error names the file's path. A Run never continues with part of its Reference set missing.
- An Area given a Reference set cannot export files to another Node. Revoking a file never locks that Area, because it holds nothing of the file until it copies it. While a copy is there, revoking the file makes the Area unusable, and only the uploader can download the copy as made. A copy as made cannot be shared, nor can a file with the same content as one of the Area's Reference files, so copying it again does not get around this. Editing a copy makes it that person's own file, and this boundary does not follow it: someone who may read a file can already retype it.
- A Run that created a Subnode reads its members' Areas but not their Reference sets, which stay inside the Runs whose Agent versions bind them. A copy a member made is readable.
- Who may upload is not changed by this decision.
