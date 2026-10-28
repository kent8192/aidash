---
status: accepted
---

# Data residency is a property of the Deployment, and Cloud runs one per region

Decided on 2026-10-27. A Deployment has one database, so where a Tenant's data is stored and processed is decided by its Deployment, not by the Tenant. Each Deployment declares a Deployment Region from a closed, code-defined list of jurisdictions (EU, US and JP to start), covering its database, Secret Store, backups, logs, Execution Pods, Egress gateway and broker. External Identities, Billing Accounts and the Marketplace are shared across Deployments by design (ADR 0083) and lie outside it, which is disclosed. Cloud becomes one shared Deployment per region Aidash offers; a Tenant is created in one of them and stays there.

Choosing where a Tenant lives restricts nothing. A separate Data Region restriction in the Tenant's policy, off by default and recommended off, makes Aidash send the Tenant's content only to destinations that process it inside the Tenant's Deployment Region: OpenRouter's regional endpoints, Provider tools OpenRouter offers in that region, Research providers whose terms record that region, and Deployment-hosted models. Keyless endpoints are refused, and Plan Connections only through their exemption (ADR 0104). Regional availability is checked at each send, as the ZDR list is. A delegation chain must satisfy the regions of every Tenant in it that has the restriction, and a chain whose regions do not overlap is refused.

Because few destinations serve every region (OpenRouter has no JP endpoint), the restriction cannot be turned on while no model would remain usable under it, and both the region picker at Tenant creation and the restriction setting show in advance what each choice would make unavailable.

## Considered Options

- **Cover only content sent outside Aidash.** Rejected: residency requirements also cover storage and processing.
- **Per-Tenant storage regions inside one Cloud Deployment.** Rejected: it breaks the one-database Deployment and its row-level security design.
- **Cloud stays one US Deployment; residency only through Dedicated.** Rejected as the direction, though it is the state until regional Cloud Deployments exist.
- **A region chosen freely as part of the Data Protection Level.** Rejected: regions are not ordered, and a person who picks an unsupported region would find nothing usable.

## Consequences

- The current Cloud infrastructure declares US. Regional Cloud Deployments are separate infrastructure work.
- Tenant data in the Secret Store replicates only within the Deployment Region, and log buckets and backups stay in it. Terraform-managed operational secrets that hold no Tenant data keep automatic replication, because Secret Manager cannot change a secret's replication without replacing it.
- A Tenant on another Deployment whose policy cannot be read (#566) counts as unrestricted for the region, unlike for the level: an unknown region would refuse every external destination. The assumption is logged.
- Self-hosted and Dedicated Deployments declare their own region; Aidash cannot verify a Self-hosted declaration.
