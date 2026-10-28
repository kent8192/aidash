# The Operator role is named Deployment Administrator

Decided on 2026-10-14. The deployment-wide authority role formerly called Operator is now the Deployment Administrator, and the operator grant is now the Deployment Administrator grant. "Operator" carried two meanings at once: the authority role, and whoever runs the Deployment's machines, as in an "operator-supplied" Master Key or environment key. The two coincide in Self-hosted but not in Cloud or Dedicated, and Kubernetes Operators add a third sense. The new name states its scope the same way Tenant Administrator does. Text that meant the machine sense now says "whoever runs the Deployment" or "the node" instead of using either term.

## Considered Options

- **System Administrator**: rejected. Aidash has no "System" concept; the scope is the Deployment. "Sysadmin" means the person running the machines, which strengthens the collision this rename removes, and it shortens to "admin", which the glossary avoids.
- **Keep Operator and reword only the machine sense**: cheaper, but it keeps a name that does not state its scope and loses the symmetry with Tenant Administrator.

## Consequences

- Code, migrations, OpenAPI, contract tests and tracked docs still say `operator` until the code rename Issue lands; there is no compatibility alias, because nothing is deployed yet.
- The glossary term Search service operator is renamed Search account holder for the same reason.
- Decided on 2026-10-15 for Issue #277: the graph operator grant does not become a Deployment Administrator grant. It lets one named Deployment Administrator of another Deployment view one Tenant's graph and confers no Deployment Administrator authority, so it is the Peer Graph Grant.
- Decided on 2026-10-15 for Issue #277, revised on 2026-10-16: code that used the operator actor for Aidash's own work, such as retention, purge, memory recovery and engine jobs, is not a Deployment Administrator. That work is named for acting without a Subject, not after the role, because some of it touches a single Tenant. Where it reaches every Tenant, it carries a Deployment Scope reason (#263).

Amended 2026-10-27 (#486): the Peer Graph Grant naming above stands as history only. #301 removed Peer Graph Grants and #486 deleted them from the code, so no Deployment Administrator of another Deployment reads a Tenant's graph; a Deployment Administrator reads Tenant content only through an explicit Tenant Subject.
