# Tenant tools authenticate with Tenant Secrets, and Deployment secrets never reach a tool

Decided on 2026-10-15 for #296. Since Registries belong to Tenants (ADR 0029), a Tenant's HTTP or MCP tool could name any `AIDASH_SECRET_*` variable as its `credential_env` and have the Deployment send that value to an endpoint the Tenant chose, including a Research provider's key or a Peer Credential. Tool definitions now never name a Deployment secret: `credential_env` leaves the tool transports, and a Deployment secret is sent only to endpoints the Deployment's own settings name, such as a declared Research provider (ADR 0048), a peer or the runner. A Tenant's tool authenticates instead with a Tenant Secret: a named, write-only value stored per Tenant in the Secret Store (formerly the Provider Credential Store) under its own resource namespace, as Plan Tokens are.

Each Tenant Secret records the hosts it may be sent to, and Aidash refuses, at registration, installation and dispatch, any definition that would send it elsewhere. Storing and replacing a Tenant Secret or changing its hosts (`secret.manage`) is a separate permission from naming it in a definition (`secret.use`), and Catalog Approval shows which Tenant Secret goes to which host. The person who chooses where a secret may go is therefore not necessarily the person who writes a definition, which stops a malicious Marketplace package, a hijacked package update or another member of the Tenant from redirecting it.

A Tenant types or pastes a Tenant Secret's value. Arbitrary tool servers offer no OAuth flow, and ADR 0006 forbids pasting only Key Material. A value is never returned; only its Fingerprint and last four characters are shown. On a self-hosted node the server reads the value to make the call, as ADR 0008 accepts for Key Material. On Cloud and Dedicated the application host stays write-only (ADR 0002): a tool call that carries a Tenant Secret goes through the Credential Broker, under a Capability Token that pins the secret and the tool's endpoint, and the broker attaches the value.

## Considered Options

- **Reject `credential_env` only in Tenant definitions and keep it for built-ins.** Deployment secrets would stay one built-in mistake away from any endpoint, and Tenants would still have no way to authenticate their own tools. Rejected.
- **Let the application host read Tenant Secrets on Cloud**, from a project separate from Key Material. Simpler, but a host compromise would expose every Tenant's tool secrets, which ADR 0002 exists to prevent. Rejected.
- **A store of its own for Tenant Secrets.** It would duplicate the per-Tenant encryption, versioning, Fingerprint and purge the Secret Store already has. Rejected.
- **Refuse to send Tenant Secrets to Provider Catalog hosts**, so that a Tenant cannot bypass Provider Authorization with a pasted model key. The Catalog lists only the providers Aidash itself calls, so every other model API and any relay would remain open; the rule could never be complete. A Tenant calling a model with its own key is its own choice, as with a Keyless endpoint (ADR 0036), and Catalog Approval states that Aidash's guarantees, such as zero data retention and Usage Records, do not apply. Rejected.
- **Make Tenant Secrets available inside Execution Pods** as environment variables or files. Execution Pods have no network, but code could print the value into its tool result, from which the model could pass it to any other tool or message; redacting output is defeated by any encoding. Rejected: a Tenant Secret never enters an Execution Pod, on any platform.

## Consequences

- ADR 0006's consequence that tool secrets keep their existing configuration is superseded for tools. Runner secrets and Peer Credentials stay Deployment secrets.
- Until the Credential Broker carries tool calls (a separate Issue), Cloud and Dedicated offer no Tenant Secrets. Aidash is undeployed, so nothing breaks.
- A Marketplace package declares the Tenant Secrets it needs, and whoever installs it picks which of the Tenant's Tenant Secrets fills each; the installed copy names them.
- Replacing a value changes no definition version and keeps Catalog Approvals. Deleting a Tenant Secret makes the calls that need it fail with `secret_unavailable`.
- A tool that carries a Tenant Secret is governed like any other tool under Federation: the receiving Node's authority decides, and the value never leaves the owning Tenant's Node.
- Tenant Deletion purges Tenant Secrets with the Tenant's other Secret Store contents.

## Amendment (2026-10-26, #310)

- On Cloud and Dedicated, Tenant tool calls that carry a Tenant Secret go to a Credential Broker identity that reads only Tenant Secrets, never Key Material, under a Tenant Secret Capability Token (ADR 0095).
- There, a Tenant Secret's hosts are sealed with its value, so changing them takes entering the value again (ADR 0096). Self-hosted Deployments change hosts without the value.
