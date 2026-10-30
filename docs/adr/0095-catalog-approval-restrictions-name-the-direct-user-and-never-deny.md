---
status: accepted
---

# A Catalog Approval restriction names whoever uses the definition directly, and nothing is denied for good

A Catalog Approval lets one exact version of a definition be used on one Node, optionally only by
certain subjects there (ADR 0029), but until #440 no approval stored a restriction. When an
approver approves an agent version together with everything it needs (an Approval Review), it is
not clear who a restriction on a dependency is meant for, because the agent's subject, not the
person, uses the model, tools and other definitions it depends on. We decided that a restriction
names subjects or Assignable Groups and is checked against whoever uses the definition directly.
For an agent version, that is who may start its Runs. For a definition an agent depends on, it is
which agents may use it, and an Approval Review restricts each dependency to the agent being
approved unless the approver chooses otherwise. A restriction only narrows what the Tenant's
policy allows. A Catalog Approval can be withdrawn but never recorded as a standing refusal.

## Considered Options

- **Checking every restriction against the person who started the Run.** It is easier to explain,
  but any agent such a person runs could use the dependency. Rejected.
- **Restricting only agent versions.** That leaves dependencies open to every agent on the Node.
  Rejected.
- **An explicit deny that outranks approval.** It would change the Tenant's safety model and
  interact with Marketplace activation and Built-in definitions. No acceptance criterion needs
  it: withdrawal and declining a Catalog Approval Request (ADR 0096) are enough. Rejected.

## Consequences

- Existing approvals carry no restriction and keep working unchanged.
- One definition version has one Catalog Approval per Node. When a second agent needs a
  dependency that is restricted to another agent, the review widens the restriction to both.
  The widening is a new revision, shown to the approver and requiring `catalog.approve`.
- Agent subjects carry their version (ADR 0062), so a new agent version needs its dependencies'
  restrictions widened again. The Approval Review of the new version shows this.
