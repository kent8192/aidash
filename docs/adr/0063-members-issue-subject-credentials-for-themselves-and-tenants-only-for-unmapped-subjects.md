# Members issue Subject Credentials for themselves, and a Tenant issues them only for subjects no person was mapped to

Only Deployment Administrators issued Subject Credentials, so on Cloud every CLI or automation
token needed Aidash staff (#358). Issuing becomes two Tenant policy actions. `credential.create`
lets a member issue a credential only for the subject its own Mapping acts as, from a browser
session in that Mapping's context; the credential records that Mapping and ends with it.
`credential.issue` lets a subject issue credentials for the Tenant's service and agent subjects
that no Mapping has ever named; those belong to the Tenant and outlive whoever issued them.

## Considered Options

- **One action on resource kind `subject`.** Policy conditions cannot express "the caller's own
  subject", because a subject carries no identifier attribute, so members holding it could mint
  tokens for any subject. Rejected.
- **Issuing for user subjects.** A token for another person's subject acts as that person in
  every decision and audit. Rejected; each person issues their own.
- **Credentials for other subjects stay Deployment Administrator-only,** as #358 was filed.
  Automation tokens would stay support cases. Rejected.
- **Self-issued credentials outlive the Mapping.** The subject outlives the Mapping, so a member
  who left the Tenant would keep acting in it. Rejected.

## Consequences

- Disabling a Mapping, whether by Leaving a Tenant, by a Tenant Administrator or by a Deployment
  Administrator, revokes its self-issued credentials in the same transaction. Each boundary also
  checks the Mapping and its External Identity, as for a Mapping's own credential, so Identity
  Disablement stops them and a restore resumes them. Tenant Deletion revokes none (ADR 0060).
- "No Mapping ever named it" is judged from Mapping history by subject name, so changing a mapped
  user subject's kind, or deleting it and recreating it under the same name, does not open it to
  `credential.issue`.
- A Subject Credential cannot issue another credential. Lifetimes stay between one second and
  30 days with no setting, and a Mapping holds at most 20 active self-issued credentials.
- A Peer Mapping binds only a credential its author could issue now. A Mapping's own credential
  and self-issued credentials are never bound.
- Listing and revoking across the Tenant need `credential.read` and `credential.revoke`; a member
  lists and revokes its own with any enabled Mapping.
