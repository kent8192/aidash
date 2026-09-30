# Subject-scoped Marketplace authorization

Status: accepted; design interview complete. The user confirmed Q1-Q14, the consolidated contract and ADR-0013 at the final shared-understanding checkpoint. This document records the accepted design, not an implemented or tested authorization contract. The [consolidated contract](2026-09-30-marketplace-authorization-contract.md) contains the accepted action matrix, implementation boundaries, dashboard behavior and acceptance matrix derived from those decisions.

## Scope and evidence

- [Issue #74](https://github.com/kent8192/aidash/issues/74), targeting `develop/0.1.0` and milestone `v0.1.0`.
- [Functional requirements](https://app.notion.com/p/3e172fa877aa8096bca5c8d8c2c73b24): FR-MKT-001 through FR-MKT-004 and FR-AUTH-001; fetched during this interview, with page last edited at `2026-09-28T12:03:07.241Z`.
- Implementation inspected at `827480c13d796bca142787be5bbd25ce34c80551`, confirmed as the remote development branch head on 2026-09-30.
- The release scope remains unchanged. [Issue #67](https://github.com/kent8192/aidash/issues/67) has no accepted scope reduction.

At this baseline, `src/api.rs` restricts Marketplace routes to operators. `src/registry.rs` identifies packages and installations by node-local `id` and `version`, without tenant ownership; installing again can replace the node-wide configuration overlay while preserving the published manifest. `src/authorization/catalog.rs` accepts a catalog approval only after its exact Registry version exists. `docs/authorization.md` separates catalog approval from subject permission, and the dashboard excludes subject contexts from Marketplace workflows.

These are source-code observations, not runtime acceptance results. Existing digest verification, immutable package versions, idempotency, dependency validation and operator recovery remain requirements. Default deny, explicit-deny precedence, delegation constraints, trusted attributes, revocation at authorization boundaries and protection against metadata disclosure are also fixed requirements rather than optional design choices.

## Accepted decisions

### Q1: Package ownership and distribution audience

Each package belongs to a tenant. Its initial audience is limited to that tenant; the design also supports explicit sharing to named recipient tenants. Audience inclusion does not authorize a subject to browse, read or install a package without the applicable action permissions. Display authorship is separate from the authenticated publisher and persisted ownership.

The user accepted the recommended choice in round 1. Recorded in [the ownership and identity contract](2026-09-30-marketplace-authorization-contract.md#domain-and-resource-identity).

### Q2: Installation isolation

Installation state and local configuration are isolated by tenant. Installing or reconfiguring a package for tenant A must not change tenant B's configuration, installation state or authority. Subjects inside a tenant may collaborate on installations according to their permissions; installations are not personal by default.

The user accepted the recommended choice in round 1. Recorded with Q3 in [the installation and execution contract](2026-09-30-marketplace-authorization-contract.md#installation-revisions-and-execution).

### Q3: Installation and catalog admission

A subject with installation authority may install a package before the resulting Registry definition is approved in the tenant catalog. The result is installed and awaiting approval. Neither installation nor Registry registration automatically grants catalog approval or execution permission; use continues to require the existing approval and action checks.

Dependency preparation and configuration approval are further constrained by Q10-Q11. Installation does not confer authority to approve the result.

The user accepted the recommended choice in round 1. Recorded in [the installation and execution contract](2026-09-30-marketplace-authorization-contract.md#installation-revisions-and-execution).

### Q4: Repository- and owner-qualified identity

Use the repository Node, owner tenant, package identifier and exact version to distinguish published resources. Identically named packages owned by different tenants remain separate, and publishing another version requires authority over the existing package identity rather than ownership inferred from a submitted author field. The consolidated contract defines the identity mapping; exact wire encoding is an implementation detail that must preserve the qualified identity and the existing policy identifier bounds.

The user accepted the recommended choice in round 2. Recorded in [the ownership and identity contract](2026-09-30-marketplace-authorization-contract.md#domain-and-resource-identity).

### Q5: Explicit authority to share exact versions

Require a separate sharing permission in the owner tenant; being the original publisher alone is not an exemption from policy. Sharing names the recipient tenant and exact immutable package version, without implicitly including future versions. A recipient cannot change the original package's distribution audience. Republishing an installed definition is a separate source-disclosure question, not settled by permission to edit the original audience.

The user accepted the recommended choice in round 2. Recorded in [the ownership and identity contract](2026-09-30-marketplace-authorization-contract.md#domain-and-resource-identity).

### Q6: Publish an exact registered definition

For subject publication, select an existing exact Registry definition and require separate source read/export authority plus permission to publish the destination package. Build the definition portion of the manifest from trusted stored content rather than accepting a substituted request-body definition; do not copy tenant installation overrides or resolve credential values into the publication. Q8-Q10 further constrain redistribution and dependencies.

The user accepted the recommended choice in round 2. Recorded in [the publication and redistribution contract](2026-09-30-marketplace-authorization-contract.md#publication-sharing-and-redistribution).

### Q7: Separate distribution withdrawal from installed-copy authority

Withdrawing Marketplace access prevents subsequent distribution reads and acquisitions, and an installation that loses required authority before its commit boundary must fail without partial state. A previously committed tenant installation remains governed by the installing tenant's catalog and execution policy, rather than giving the publisher an implicit power to stop recipient workloads. Disabling an installed definition therefore uses the recipient's catalog/execution controls; this separation does not automatically authorize redistribution.

The user accepted the recommended choice in round 2. Recorded in [the installation and execution contract](2026-09-30-marketplace-authorization-contract.md#installation-revisions-and-execution).

### Q8: Explicit upstream consent for redistribution

An installed definition does not acquire an automatic right of onward distribution. Require the source owner's separate consent for the exact source version and intended audience in addition to the recipient tenant's local read/export and publication permissions. Preserve known installation/publication provenance so duplicating a registered imported definition does not erase that gate. Withdrawal prevents further authorized redistribution without retroactively disabling already committed recipient installations under Q7; Q13 defines the continuing checks on downstream publications.

The user accepted the recommended choice in round 3. Recorded in [the publication and redistribution contract](2026-09-30-marketplace-authorization-contract.md#publication-sharing-and-redistribution).

### Q9: Authorized summaries without hidden dependency disclosure

A package may remain discoverable through an explicitly authorized summary even if the caller cannot read a dependency. The summary excludes the inaccessible dependency's name, identifier, metadata and count; search filters and counts must not reveal those fields indirectly. Full-manifest reads and installation fail without disclosing which dependency is missing or denied, and the immutable manifest is never returned with silent redactions that would misrepresent its digest.

The user accepted the recommended choice in round 3. Recorded in [the publication and redistribution contract](2026-09-30-marketplace-authorization-contract.md#publication-sharing-and-redistribution).

### Q10: Explicit dependency preparation

Preserve explicit dependency preparation: dependencies must already be registered or installed for the receiving tenant and accessible under the relevant management/read authority. Do not fetch or install missing dependencies implicitly during the root installation. An authorized root installation is atomic; execution still requires catalog approval and use permissions for the complete execution dependency set. This is not a requirement to execute or approve a dependency merely to stage an otherwise authorized installation.

The user accepted the recommended choice in round 3. This retains the existing explicit dependency-preparation behavior while adding tenant and authorization boundaries; it does not need a separate ADR.

### Q11: Approval of exact effective configuration revisions

An effective configuration change creates a pending installation revision requiring fresh approval; it does not inherit an earlier approval merely because the package version is unchanged. Preserve the previous approved revision as active until the replacement is explicitly approved, and never mix pending configuration into existing execution. Identical replays do not create a new revision. Q12 defines the handling of already admitted Runs when an approved replacement becomes active.

The user accepted the recommended choice in round 3. Recorded in [the installation and execution contract](2026-09-30-marketplace-authorization-contract.md#installation-revisions-and-execution).

### Q12: Pin admitted Runs to their approved installation revisions

New Runs use the newly active approved revision; an already admitted Run continues with its exact previously approved revision and dependency definitions while that authority remains valid. Activating a replacement does not silently change a running Agent's configuration. Explicit revocation of the old revision's approval still pauses affected work at the next existing authorization boundary; retained content is not a perpetual execution grant.

The user accepted the recommended choice in round 4. Recorded in [the installation and execution contract](2026-09-30-marketplace-authorization-contract.md#installation-revisions-and-execution).

### Q13: Recheck consent for further downstream distribution

Withdrawing source redistribution consent prevents new publication and subsequent Marketplace disclosure/acquisition of already published downstream packages through their known provenance chain. Existing committed tenant installations continue to follow Q7. Persist the consent and source lineage rather than relying on the original publishing request or the publisher's continuing login session; validate chains without revealing hidden upstream metadata.

The user accepted the recommended choice in round 4. Recorded in [the publication and redistribution contract](2026-09-30-marketplace-authorization-contract.md#publication-sharing-and-redistribution).

### Q14: Keep legacy data under operator authority until explicit adoption

Keep existing unowned packages and node-wide installations in their operator-managed legacy scope. Never infer a tenant owner from an author string or assign them to the first tenant or caller. An operator may explicitly adopt/copy selected definitions into a named tenant's qualified package and installation scope with provenance and fresh tenant approval, while preserving the existing operator recovery paths and immutable source content.

The user accepted the recommended choice in round 4. Recorded in [the ownership and identity contract](2026-09-30-marketplace-authorization-contract.md#domain-and-resource-identity).

## Design tree

| Branch                                                  | State                          | Consequence                                                                                                       |
| ------------------------------------------------------- | ------------------------------ | ----------------------------------------------------------------------------------------------------------------- |
| Ownership, qualified identity and exact-version sharing | Accepted: Q1, Q4-Q5            | Naming collisions do not merge authority; sharing requires explicit permission.                                   |
| Tenant installation and separate use approval           | Accepted: Q2-Q3, Q7            | Distribution withdrawal does not revoke local use approval.                                                       |
| Stored publication inputs and onward distribution       | Accepted: Q6, Q8, Q13          | Source consent is checked for new and existing downstream distribution.                                           |
| Summaries and dependency preparation                    | Accepted: Q9-Q10               | Hidden dependencies stay undisclosed; dependency preparation is explicit.                                         |
| Configuration revision approval and admitted Runs       | Accepted: Q11-Q12              | Pending revisions need approval; admitted Runs retain pinned approved definitions.                                |
| Legacy packages and operator recovery                   | Accepted: Q14                  | Tenant adoption is explicit; legacy operator behavior remains isolated.                                           |
| Action matrix, Registry mapping and concurrency         | Accepted at final confirmation | See the accepted [Registry mapping](2026-09-30-marketplace-authorization-contract.md#installed-registry-mapping). |
| Dashboard, errors, audit and acceptance matrix          | Accepted at final confirmation | The contract covers each Issue acceptance criterion and en-US/ja-JP behavior.                                     |
| Final shared understanding                              | Confirmed                      | The user accepted the consolidated contract, including the immutable installed-definition mapping.                |

## Final confirmation

The decision frontier is empty. After reviewing the [consolidated contract](2026-09-30-marketplace-authorization-contract.md) and ADR-0013, the user confirmed the final shared understanding with "OK". The design interview is complete. No implementation, runtime verification, commit, push or PR publication is represented by completion of this design interview.
