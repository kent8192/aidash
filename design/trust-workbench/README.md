# Trust Workbench detail tab proposals

Status: all four designs approved by the user. Generated with imagegen from the supplied Overview reference.

- [Policies](policies.png): contextual permission inputs, decision summary, permission matrix.
- [Audit](audit.png): timeline, event details and pagination, evidence scope.
- [Certifications](certifications.png): external assessment unavailable state and links to factual records.
- [Incidents](incidents.png): report filters and list beside report/evidence form.

These are visual proposals, not runtime screenshots. Empty values are intentional; production content must come from authenticated APIs. Existing app navigation, identity, localization and access constraints remain authoritative. Generated wording suggesting assessment submission, all-tenant access, or unsupported input limits is not a functional requirement and must be corrected during implementation. Approval covers the visual layout, not invented backend behavior. Implementation follows these approved layouts; runtime verification is tracked in [qa.md](qa.md).

## Implementation captures

These browser captures use isolated regression-test fixtures. They show the implemented layout, not live account data. Production screens request authenticated API records.

- [Overview](implementation/overview.png)
- [Policies](implementation/policies.png)
