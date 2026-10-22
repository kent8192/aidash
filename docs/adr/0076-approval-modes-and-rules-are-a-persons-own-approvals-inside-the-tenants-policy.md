# Approval Modes and Approval Rules are a person's own approvals inside the Tenant's policy

Decided on 2026-10-19 for #408 and #411. Autonomy is what Aidash offers, yet each approval stops a Run until a person answers. People can now decide in advance, as in Claude Code's permission modes and rules: an Approval Mode (Manual, Auto, Don't ask or Bypass) and allow, ask or deny Approval Rules. Unlike Claude Code, where the person who allows an action also bears its risk, a Tenant answers for what its members' Runs do. So a mode or rule is an approval given in advance by its owner, recorded under their name with the rule or mode that gave it, and never reaches beyond what the Tenant's policy lets that person approve. The Tenant's policy also says who may use each mode, and Bypass is unavailable unless it allows it.

A person's modes and rules cover only Runs whose delegation chain starts at that person, child Runs included within their delegated authority, and never Runs that arrived through Federation or Tenant maintenance, Workbench tests or evaluation fixtures. Aidash decides at the moment an approval is needed, in a fixed order: an explicit deny in the Tenant's policy refuses; direct permission or a Read-only operation proceeds; then the person's rules, deny before ask before allow; then the person's mode.

## Considered Options

- **Remove the stop for whatever the person relaxes**, with no approval recorded. Nobody could later tell who let a Run act. Rejected; removing a stop altogether stays the Tenant Administrators' lever, through direct permission.
- **Let a person's rules cover other people's Runs**, for example a team lead approving for the team. That is the Tenant's decision to make through its policy. Rejected.
- **Pin the mode and rules in the Run's Binding snapshot.** A lowered ceiling or a removed rule would not reach running Runs, though authority is otherwise rechecked at durable boundaries (ADR 0013). Rejected.
- **Make Bypass available by default**, as Claude Code does on a person's own machine. Rejected for the reason above.

## Consequences

- A rule beyond a lowered ceiling stays stored without effect, and the dashboard shows it as disabled.
- Tenant Administrators can read a member's modes and rules but change only the ceiling.
- Failed-subtask intervention, Completion Reviews and Reconciliation are not approvals and keep waiting for a person.
