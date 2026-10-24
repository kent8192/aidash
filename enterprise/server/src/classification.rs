//! Every table the Enterprise migrations create, classified like Community's
//! (#223, #266).
//!
//! Community's `server/src/database/classification.rs` lists the Community
//! tables. This list is the Enterprise build's part of the combined list that
//! [`Build::tables`](aidash_server::bootstrap::build::Build::tables) returns,
//! so the comparison tests and the purge of a deleted Tenant read both. A
//! migration that adds an Enterprise table adds one line here. Billing tables
//! (#242) are Deployment-wide: a Billing Account pays for several Tenants, and
//! usage records outlive the purge of the Tenant they name.
//!
//! GCIP Tenants, the Shared Pool, Tenant Bindings, Sign-in Domains and MFA
//! Requirements are `[dashboard.gcip]` settings, not tables, so they have no
//! row here.
use aidash_server::database::classification::Classification::DeploymentWide;
use aidash_server::database::classification::{Table, table};

pub const TABLES: &[Table] = &[
	// how each GCIP External Identity was admitted; names a Tenant, belongs to none
	// Billing Accounts, Subscriptions, consents and usage: each pays for or
	// measures Tenants by name, owns none of them, and outlives their purge
	table("billing", "billing_accounts", DeploymentWide),
	table("billing", "billing_attachment_consents", DeploymentWide),
	table("billing", "billing_attachments", DeploymentWide),
	table("billing", "billing_coverage_periods", DeploymentWide),
	table("billing", "billing_pod_usage", DeploymentWide),
	table("billing", "billing_storage_samples", DeploymentWide),
	table("billing", "billing_subscriptions", DeploymentWide),
	// how each GCIP External Identity was admitted; names a Tenant, belongs to none
	table("gcip", "gcip_admissions", DeploymentWide),
];

#[cfg(test)]
mod tests;
