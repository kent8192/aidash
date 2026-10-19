//! Every table's place in Tenant isolation (#223, #263).
//!
//! A Tenant-owned table holds data that exists for exactly one Tenant; a
//! Deployment-wide table holds data that belongs to no Tenant. A test compares
//! this list with the migrated database and fails on any table that is missing
//! from it or classified differently from its policies, so a migration that
//! adds a table classifies it here in the same change. A build that adds
//! tables lists them itself and hands its list over through
//! [`Extension::tables`](crate::bootstrap::build::Extension::tables); the same
//! comparison (`classification/comparison.rs`) checks the combined list.

/// How row-level security treats a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
	/// Tenant-owned, with the forced row-level policy enforced.
	TenantOwned,
	/// Tenant-owned, but its policy is enforced by the named Issue. #265
	/// copies the Tenant onto the tables that reach it only through a parent,
	/// and #241 gives Nodes and peers a Tenant. A new table with a `tenant`
	/// column is Tenant-owned and enforced from its first migration.
	Pending(u32),
	/// Deployment-wide: no row-level policy.
	DeploymentWide,
}

/// One table of the Community workspace's migrations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Table {
	pub app: &'static str,
	pub name: &'static str,
	pub classification: Classification,
}

pub const fn table(app: &'static str, name: &'static str, classification: Classification) -> Table {
	Table {
		app,
		name,
		classification,
	}
}

use Classification::{DeploymentWide, Pending, TenantOwned};

/// Every table the Community migrations create, grouped by app.
pub const TABLES: &[Table] = &[
	// execution
	// undecodable activation messages, before any Tenant is known
	table("execution", "activation_quarantine", DeploymentWide),
	table("execution", "context_compaction_attempts", TenantOwned),
	table("execution", "core_areas", TenantOwned),
	table("execution", "core_objects", TenantOwned),
	table("execution", "core_operations", TenantOwned),
	table("execution", "core_quotas", TenantOwned),
	table("execution", "core_records", TenantOwned),
	table("execution", "core_requests", TenantOwned),
	table("execution", "core_runs", TenantOwned),
	table("execution", "core_task_sessions", TenantOwned),
	// Decision Evidence (#290) belongs to its Run, like the Context Journal.
	table("execution", "decision_attempts", TenantOwned),
	table("execution", "decision_evidence", TenantOwned),
	table("execution", "decision_outcomes", TenantOwned),
	table("execution", "decision_states", TenantOwned),
	// per-model token statistics without Tenant content
	table("execution", "estimate_calibration_samples", DeploymentWide),
	table("execution", "events", TenantOwned),
	table("execution", "generation_budgets", TenantOwned),
	table("execution", "generation_compaction_usage", TenantOwned),
	table("execution", "generation_embedding_usage", TenantOwned),
	table("execution", "generation_history", TenantOwned),
	table("execution", "generation_policies", TenantOwned),
	table("execution", "generation_policy_history", TenantOwned),
	table("execution", "generation_remote_dispatches", TenantOwned),
	table("execution", "generation_remote_finalizations", TenantOwned),
	table("execution", "generation_remote_intents", TenantOwned),
	table("execution", "generation_remote_usage", TenantOwned),
	table("execution", "generation_requests", TenantOwned),
	table("execution", "generation_summary_usage", TenantOwned),
	table("execution", "generation_usage", TenantOwned),
	table("execution", "human_requests", TenantOwned),
	// inbound event deduplication
	table("execution", "inbox", DeploymentWide),
	table("execution", "inference_attempts", TenantOwned),
	table("execution", "inference_progress", TenantOwned),
	table("execution", "inference_usage", TenantOwned),
	table("execution", "invocations", TenantOwned),
	// recorded ends of Research account holders' agreements (ADR 0058)
	table("execution", "research_account_terminations", DeploymentWide),
	table("execution", "research_attempts", TenantOwned),
	table("execution", "research_fragments", TenantOwned),
	table("execution", "research_originals", TenantOwned),
	table("execution", "research_page_images", TenantOwned),
	table("execution", "research_revocations", TenantOwned),
	table("execution", "research_snapshots", TenantOwned),
	table("execution", "research_sources", TenantOwned),
	table("execution", "run_activations", TenantOwned),
	table("execution", "run_context_events", TenantOwned),
	table("execution", "run_inputs", TenantOwned),
	table("execution", "run_task_snapshots", TenantOwned),
	table("execution", "runs", TenantOwned),
	table("execution", "url_intents", TenantOwned),
	// federation
	// cross-node transaction plumbing
	table("federation", "atomic_authority_attempts", DeploymentWide),
	table("federation", "atomic_coordinators", DeploymentWide),
	table("federation", "atomic_gate", DeploymentWide),
	table("federation", "atomic_history", DeploymentWide),
	table("federation", "atomic_participants", DeploymentWide),
	table("federation", "atomic_peer_trust", DeploymentWide),
	table("federation", "atomic_preflights", DeploymentWide),
	table("federation", "atomic_subjects", DeploymentWide),
	table("federation", "atomic_votes", DeploymentWide),
	table(
		"federation",
		"authorization_peer_mapping_history",
		TenantOwned,
	),
	table("federation", "authorization_peer_mappings", TenantOwned),
	table("federation", "authorization_remote_admissions", TenantOwned),
	table(
		"federation",
		"authorization_remote_grant_reads",
		TenantOwned,
	),
	table("federation", "authorization_remote_grants", TenantOwned),
	table("federation", "delegations", TenantOwned),
	table("federation", "peer_events", Pending(241)),
	table("federation", "peers", Pending(241)),
	table("federation", "remote_run_message_fences", TenantOwned),
	// identity
	table("identity", "authorization_bundles", TenantOwned),
	table("identity", "authorization_catalog", TenantOwned),
	table("identity", "authorization_catalog_history", TenantOwned),
	table("identity", "authorization_credentials", TenantOwned),
	table("identity", "authorization_decisions", TenantOwned),
	table("identity", "authorization_execution", TenantOwned),
	table("identity", "authorization_peer_graph_grants", TenantOwned),
	table("identity", "authorization_remote_commands", TenantOwned),
	table("identity", "authorization_remote_execution", TenantOwned),
	table("identity", "authorization_remote_outputs", TenantOwned),
	table("identity", "authorization_revisions", TenantOwned),
	table("identity", "authorization_run_outputs", TenantOwned),
	table("identity", "authorization_run_reads", TenantOwned),
	table("identity", "authorization_run_registry_reads", TenantOwned),
	table("identity", "authorization_run_remote_reads", TenantOwned),
	table("identity", "authorization_task_origins", TenantOwned),
	table("identity", "authorization_workspaces", TenantOwned),
	// the Master Key check
	table("identity", "credential_store_keys", DeploymentWide),
	table("identity", "credential_store_resources", TenantOwned),
	table("identity", "credential_store_versions", TenantOwned),
	table("identity", "dashboard_administration_history", TenantOwned),
	table("identity", "dashboard_execution_origins", TenantOwned),
	// External Identities
	table("identity", "dashboard_identities", DeploymentWide),
	// sign-in state
	table("identity", "dashboard_login_transactions", DeploymentWide),
	// back-channel logout
	table("identity", "dashboard_logout_tokens", DeploymentWide),
	table("identity", "dashboard_mappings", TenantOwned),
	// Deployment Administrator grants
	table(
		"identity",
		"dashboard_deployment_administrator_grants",
		DeploymentWide,
	),
	// Registration Requests
	table(
		"identity",
		"dashboard_registration_requests",
		DeploymentWide,
	),
	// sessions
	table("identity", "dashboard_sessions", DeploymentWide),
	// desktop sign-in
	table("identity", "desktop_handoffs", DeploymentWide),
	// desktop sessions
	table("identity", "desktop_refresh_credentials", DeploymentWide),
	// the identity's one Personal Tenant, read at sign-in
	table("identity", "personal_tenants", TenantOwned),
	// pending Sign in with ChatGPT flows bound to a browser, like sign-in state
	table("identity", "plan_authorizations", DeploymentWide),
	// this Deployment's Plan Connection host ID
	table("identity", "plan_connection_hosts", DeploymentWide),
	table("identity", "plan_connections", TenantOwned),
	// pending OAuth flows bound to a browser, like sign-in state
	table("identity", "provider_authorizations", DeploymentWide),
	table("identity", "provider_credential_bindings", TenantOwned),
	table("identity", "provider_credentials", TenantOwned),
	table("identity", "run_plan_connections", TenantOwned),
	table("identity", "run_provider_credentials", TenantOwned),
	table("identity", "tenant_invitations", TenantOwned),
	table("identity", "tenant_retention_rule_history", TenantOwned),
	table("identity", "tenant_retention_rules", TenantOwned),
	// Tenant Deletions outlive purge and are read while the Tenant refuses everyone (ADR 0060)
	table("identity", "tenant_deletions", DeploymentWide),
	// Tenant root records: the Deployment's list of Tenants (ADR 0050)
	table("identity", "tenants", DeploymentWide),
	// knowledge
	table("knowledge", "memory_bank_decay", TenantOwned),
	table("knowledge", "memory_bank_policy_changes", TenantOwned),
	table("knowledge", "memory_bank_settings", TenantOwned),
	table("knowledge", "memory_banks", TenantOwned),
	table("knowledge", "memory_candidate_reviews", TenantOwned),
	table("knowledge", "memory_candidates", TenantOwned),
	table("knowledge", "memory_dependencies", TenantOwned),
	table("knowledge", "memory_engine_jobs", TenantOwned),
	table("knowledge", "memory_history", TenantOwned),
	// the learning discovery cursor
	table("knowledge", "memory_learning_discovery", DeploymentWide),
	table("knowledge", "memory_model_attempts", TenantOwned),
	table("knowledge", "memory_model_operations", TenantOwned),
	table("knowledge", "memory_participants", TenantOwned),
	table("knowledge", "memory_publications", TenantOwned),
	table("knowledge", "memory_purge_jobs", TenantOwned),
	table("knowledge", "memory_receipts", TenantOwned),
	table("knowledge", "memory_receiver_caches", TenantOwned),
	table("knowledge", "memory_remote_read_gates", TenantOwned),
	table("knowledge", "memory_remote_reads", TenantOwned),
	table("knowledge", "memory_run_bindings", TenantOwned),
	table("knowledge", "memory_run_read_gates", TenantOwned),
	table("knowledge", "memory_run_reads", TenantOwned),
	table("knowledge", "memory_task_participants", TenantOwned),
	table("knowledge", "memory_unit_origins", TenantOwned),
	table("knowledge", "memory_unit_retention", TenantOwned),
	table("knowledge", "memory_unit_run_origins", TenantOwned),
	table("knowledge", "memory_units", TenantOwned),
	table("knowledge", "semantic_collections", TenantOwned),
	table("knowledge", "semantic_entries", TenantOwned),
	table("knowledge", "semantic_history", TenantOwned),
	table("knowledge", "semantic_indexes", TenantOwned),
	table("knowledge", "semantic_points", TenantOwned),
	table("knowledge", "semantic_remote_attempts", TenantOwned),
	table("knowledge", "semantic_remote_operations", TenantOwned),
	table("knowledge", "semantic_remote_reads", TenantOwned),
	table("knowledge", "semantic_remote_receipts", TenantOwned),
	table("knowledge", "semantic_run_reads", TenantOwned),
	// vector collections per embedding dimension
	table("knowledge", "semantic_vector_collections", DeploymentWide),
	table("knowledge", "semantic_vectors", TenantOwned),
	// marketplace
	// what the Marketplace offers
	table("marketplace", "marketplace_audiences", DeploymentWide),
	table("marketplace", "marketplace_consents", DeploymentWide),
	table("marketplace", "marketplace_installations", TenantOwned),
	table("marketplace", "marketplace_provenance", DeploymentWide),
	table("marketplace", "marketplace_requests", TenantOwned),
	table("marketplace", "marketplace_revisions", TenantOwned),
	table("marketplace", "marketplace_versions", DeploymentWide),
	// registry
	table("registry", "agent_draft_registrations", TenantOwned),
	table("registry", "agent_draft_shares", TenantOwned),
	table("registry", "agent_drafts", TenantOwned),
	table("registry", "agent_incident_events", TenantOwned),
	table("registry", "agent_incidents", TenantOwned),
	table("registry", "agent_knowledge", TenantOwned),
	table("registry", "agent_test_limits", TenantOwned),
	table("registry", "agent_test_profiles", TenantOwned),
	table("registry", "agent_test_sessions", TenantOwned),
	table("registry", "registry", TenantOwned),
	table("registry", "registry_agent_model_refs", TenantOwned),
	table("registry", "registry_agent_resource_refs", TenantOwned),
	table("registry", "registry_memory_role_refs", TenantOwned),
	table("registry", "registry_requests", TenantOwned),
	// reinhardt
	// the migration ledger
	table("reinhardt", "reinhardt_migrations", DeploymentWide),
	// workspaces
	table("workspaces", "artifacts", TenantOwned),
	table("workspaces", "channel_attachments", TenantOwned),
	table("workspaces", "channel_message_context", TenantOwned),
	table("workspaces", "channel_threads", TenantOwned),
	table("workspaces", "conversations", TenantOwned),
	table("workspaces", "messages", TenantOwned),
	table("workspaces", "task_dependencies", TenantOwned),
	table("workspaces", "tasks", TenantOwned),
	table("workspaces", "workspaces", TenantOwned),
];

#[cfg(test)]
mod tests;
