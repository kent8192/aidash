# The Tenant Configurator acts with its Tenant Administrator's full authority

Decided on 2026-11-26 in the Tenant Configurator grilling (Issues #1154 and #1155). A Tenant Administrator should be able to say what they want in plain words and have an Agent configure the Tenant to fit, without learning where each setting lives. ADR 0143's fourth brake keeps every Agent away from approvals, policy, groups, invitations, Approval Modes and Rules, Instructions and Budget raises, even when its root person is a Tenant Administrator, so no Agent could do this work.

## Decisions

- **One Built-in agent, the Tenant Configurator, is exempt from brake 4.** Within the authority of the Tenant Administrator who started it, it may make Catalog Approvals, decide Registration Requests, invite, change the policy, groups, Approval Modes, Approval Rules, Tenant and Node Instructions, and create or raise Budgets. Every other Agent keeps brake 4.
- **Its authority is still the chain's.** It acts as its root person through the ordinary intersection along the delegation chain; the last-administrator check and every other policy check apply unchanged. Only Tenant Administrators may start it, and no Remote Grant can.
- **It never purges a Tenant or a Node.** A purge is offered to the person as a Setting Request button that they press themselves.
- **Its tools run in-process as the root person's browser request.** Tenant administration routes that require a browser session accept its calls; Runs still receive no Aidash API credential, and Tenant Secrets still refuse Aidash Subject Credentials.
- **Its Threads start in Bypass.** Whatever the Tenant Administrator's default Approval Mode, a Thread running the Tenant Configurator starts in Bypass and may be switched like any Thread.

## Considered Options

- **Lift brake 4 for every Agent.** Rejected: one named Agent that can change authority keeps the audit question "which Agent changed the policy?" answerable.
- **Give the Run an Aidash API credential.** Rejected: in-process execution needs no secret in the sandbox and no change to the Tenant Secret refusal.
- **A plan-then-confirm step before any change.** Rejected by the Tenant Administrator's choice of Bypass; tool call approval and Approval Modes already let a person slow it down.

## Consequences

- A Tenant Administrator who runs the Tenant Configurator in Bypass delegates their whole administrative authority, except purges, to it for that Thread.
- Change records name the Tenant Configurator and the person it acted for through the Run's delegation chain.
- ADR 0143's brake 4 now reads "except the Tenant Configurator (ADR 0151)".
