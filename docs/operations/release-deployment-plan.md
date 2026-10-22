# Release deployment implementation plan

The approved flow is: merge into `develop/x.y.z`, deploy that exact successful
CI SHA to staging and create an immutable release PR in parallel; merge the
validated release PR into `main`, then deploy the actual successful main SHA to
production. PR environments remain explicitly requested with `/preview up`.

## Implementation boundaries

- Controller: exact event source, latest-source fencing, staging auto-create,
  isolated production policy and human-merged managed-release verification;
  immutable staging evidence and notification outbox after verified readiness.
- Release automation: GitHub App token, immutable timestamped branches,
  superseded draft handling, exact CI and trusted staging receipt check.
- Terraform: per-incarnation cryptographic staging/PR hostnames, separate
  production project/state/runtime identities, exact workflow/environment WIF,
  private notification bucket with separate sender identity.
- Edge and sign-in: trusted wildcard certificate secret, exact Host routing,
  host-only cookies and CSRF unchanged; exact GCIP authorized hosts and fixed
  Firebase OAuth helper, no wildcard redirect URIs.
- Notifications: Cloudflare Email Sending, fixed sender
  `deployment.noreply@aidash.run` and fixed stg/prod/test role recipients;
  authenticated API calls, durable deduplication and notification-only retries.

Nonproduction privileged code is pinned to one commit of the protected
`automation_branch`, the repository default branch on which scheduled and
workflow-run automation executes. Production cloud jobs execute the production
workflow and keep their code anchor on `main`; `trusted_branch` names that
production anchor and no nonproduction check reads it. See
[ADR 0081](../adr/0081-nonproduction-delivery-trusts-the-protected-development-branch.md).

## Verification

Exercise lifecycle and source races, production isolation, main/release tree
divergence, stale staging evidence, untrusted comment/fork requests, hostname
incarnation continuity and destruction, TLS and authorized-domain boundaries,
fixed mail routing, API errors and durable duplicate/retry handling. Run the
focused Python tests, Terraform validation/provider-mocked tests, chart checks,
Ruff, actionlint and diff checks, then review the complete change for security.

Publishing the reviewed change does not activate the workflows. Cloud mutation,
DNS changes and real mail require the documented account inputs and supervised
deployment acceptance checks. Production additionally requires the isolated
restore and operational readiness gates.
