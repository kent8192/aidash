//! Every table's place in Tenant isolation (#223, #263).
//!
//! A Tenant-owned table holds data that exists for exactly one Tenant; a
//! Deployment-wide table holds data that belongs to no Tenant. A test compares
//! this list with the migrated database and fails on any table that is missing
//! from it or classified differently from its policies, so a migration that
//! adds a table classifies it here in the same change.

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

const fn table(app: &'static str, name: &'static str, classification: Classification) -> Table {
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
	table("execution", "context_compaction_attempts", Pending(265)),
	table("execution", "core_areas", TenantOwned),
	table("execution", "core_objects", TenantOwned),
	table("execution", "core_operations", TenantOwned),
	table("execution", "core_quotas", TenantOwned),
	table("execution", "core_records", TenantOwned),
	table("execution", "core_requests", TenantOwned),
	table("execution", "core_runs", Pending(265)),
	table("execution", "core_task_sessions", Pending(265)),
	// Decision Evidence (#290) belongs to its Run, like the Context Journal.
	table("execution", "decision_attempts", Pending(265)),
	table("execution", "decision_evidence", Pending(265)),
	table("execution", "decision_outcomes", Pending(265)),
	table("execution", "decision_states", Pending(265)),
	// per-model token statistics without Tenant content
	table("execution", "estimate_calibration_samples", DeploymentWide),
	table("execution", "events", Pending(265)),
	table("execution", "generation_budgets", Pending(265)),
	table("execution", "generation_compaction_usage", Pending(265)),
	table("execution", "generation_embedding_usage", Pending(265)),
	table("execution", "generation_history", Pending(265)),
	table("execution", "generation_policies", TenantOwned),
	table("execution", "generation_policy_history", TenantOwned),
	table("execution", "generation_remote_dispatches", Pending(265)),
	table("execution", "generation_remote_finalizations", Pending(265)),
	table("execution", "generation_remote_intents", TenantOwned),
	table("execution", "generation_remote_usage", Pending(265)),
	table("execution", "generation_requests", TenantOwned),
	table("execution", "generation_summary_usage", Pending(265)),
	table("execution", "generation_usage", Pending(265)),
	table("execution", "human_requests", Pending(265)),
	// inbound event deduplication
	table("execution", "inbox", DeploymentWide),
	table("execution", "inference_attempts", Pending(265)),
	table("execution", "inference_progress", Pending(265)),
	table("execution", "inference_usage", Pending(265)),
	table("execution", "invocations", Pending(265)),
	// recorded ends of Research account holders' agreements (ADR 0058)
	table("execution", "research_account_terminations", DeploymentWide),
	table("execution", "research_attempts", Pending(265)),
	table("execution", "research_fragments", Pending(265)),
	table("execution", "research_originals", Pending(265)),
	table("execution", "research_page_images", Pending(265)),
	table("execution", "research_revocations", TenantOwned),
	table("execution", "research_snapshots", Pending(265)),
	table("execution", "research_sources", Pending(265)),
	table("execution", "run_activations", Pending(265)),
	table("execution", "run_context_events", Pending(265)),
	table("execution", "run_inputs", Pending(265)),
	table("execution", "run_task_snapshots", Pending(265)),
	table("execution", "runs", Pending(265)),
	table("execution", "url_intents", Pending(265)),
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
		Pending(265),
	),
	table("federation", "authorization_remote_grants", TenantOwned),
	table("federation", "delegations", Pending(265)),
	table("federation", "peer_events", Pending(241)),
	table("federation", "peers", Pending(241)),
	table("federation", "remote_run_message_fences", Pending(265)),
	// identity
	table("identity", "authorization_bundles", TenantOwned),
	table("identity", "authorization_catalog", TenantOwned),
	table("identity", "authorization_catalog_history", TenantOwned),
	table("identity", "authorization_credentials", TenantOwned),
	table("identity", "authorization_decisions", TenantOwned),
	table("identity", "authorization_execution", TenantOwned),
	table("identity", "authorization_peer_graph_grants", TenantOwned),
	table("identity", "authorization_remote_commands", Pending(265)),
	table("identity", "authorization_remote_execution", Pending(265)),
	table("identity", "authorization_remote_outputs", Pending(265)),
	table("identity", "authorization_revisions", TenantOwned),
	table("identity", "authorization_run_outputs", Pending(265)),
	table("identity", "authorization_run_reads", Pending(265)),
	table("identity", "authorization_run_registry_reads", Pending(265)),
	table("identity", "authorization_run_remote_reads", Pending(265)),
	table("identity", "authorization_task_origins", TenantOwned),
	table("identity", "authorization_workspaces", TenantOwned),
	// the Master Key check
	table("identity", "credential_store_keys", DeploymentWide),
	table("identity", "credential_store_resources", TenantOwned),
	table("identity", "credential_store_versions", TenantOwned),
	table("identity", "dashboard_administration_history", TenantOwned),
	table("identity", "dashboard_execution_origins", Pending(265)),
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
	table("knowledge", "memory_bank_decay", Pending(265)),
	table("knowledge", "memory_bank_policy_changes", Pending(265)),
	table("knowledge", "memory_bank_settings", Pending(265)),
	table("knowledge", "memory_banks", TenantOwned),
	table("knowledge", "memory_candidate_reviews", Pending(265)),
	table("knowledge", "memory_candidates", Pending(265)),
	table("knowledge", "memory_dependencies", Pending(265)),
	table("knowledge", "memory_engine_jobs", Pending(265)),
	table("knowledge", "memory_history", Pending(265)),
	// the learning discovery cursor
	table("knowledge", "memory_learning_discovery", DeploymentWide),
	table("knowledge", "memory_model_attempts", Pending(265)),
	table("knowledge", "memory_model_operations", Pending(265)),
	table("knowledge", "memory_participants", TenantOwned),
	table("knowledge", "memory_publications", Pending(265)),
	table("knowledge", "memory_purge_jobs", Pending(265)),
	table("knowledge", "memory_receipts", Pending(265)),
	table("knowledge", "memory_receiver_caches", Pending(265)),
	table("knowledge", "memory_remote_read_gates", Pending(265)),
	table("knowledge", "memory_remote_reads", Pending(265)),
	table("knowledge", "memory_run_bindings", Pending(265)),
	table("knowledge", "memory_run_read_gates", Pending(265)),
	table("knowledge", "memory_run_reads", Pending(265)),
	table("knowledge", "memory_task_participants", Pending(265)),
	table("knowledge", "memory_unit_origins", Pending(265)),
	table("knowledge", "memory_unit_retention", Pending(265)),
	table("knowledge", "memory_unit_run_origins", Pending(265)),
	table("knowledge", "memory_units", Pending(265)),
	table("knowledge", "semantic_collections", Pending(265)),
	table("knowledge", "semantic_entries", Pending(265)),
	table("knowledge", "semantic_history", Pending(265)),
	table("knowledge", "semantic_indexes", TenantOwned),
	table("knowledge", "semantic_points", Pending(265)),
	table("knowledge", "semantic_remote_attempts", Pending(265)),
	table("knowledge", "semantic_remote_operations", Pending(265)),
	table("knowledge", "semantic_remote_reads", Pending(265)),
	table("knowledge", "semantic_remote_receipts", Pending(265)),
	table("knowledge", "semantic_run_reads", Pending(265)),
	// vector collections per embedding dimension
	table("knowledge", "semantic_vector_collections", DeploymentWide),
	table("knowledge", "semantic_vectors", TenantOwned),
	// marketplace
	// what the Marketplace offers
	table("marketplace", "marketplace_audiences", DeploymentWide),
	table("marketplace", "marketplace_consents", Pending(265)),
	table("marketplace", "marketplace_installations", TenantOwned),
	table("marketplace", "marketplace_provenance", DeploymentWide),
	table("marketplace", "marketplace_requests", Pending(265)),
	table("marketplace", "marketplace_revisions", Pending(265)),
	table("marketplace", "marketplace_versions", DeploymentWide),
	// registry
	table("registry", "agent_draft_registrations", Pending(265)),
	table("registry", "agent_draft_shares", Pending(265)),
	table("registry", "agent_drafts", TenantOwned),
	table("registry", "agent_incident_events", Pending(265)),
	table("registry", "agent_incidents", TenantOwned),
	table("registry", "agent_knowledge", Pending(265)),
	table("registry", "agent_test_limits", TenantOwned),
	table("registry", "agent_test_profiles", TenantOwned),
	table("registry", "agent_test_sessions", TenantOwned),
	table("registry", "registry", Pending(265)),
	table("registry", "registry_agent_model_refs", Pending(265)),
	table("registry", "registry_agent_resource_refs", Pending(265)),
	table("registry", "registry_memory_role_refs", Pending(265)),
	table("registry", "registry_requests", Pending(265)),
	// reinhardt
	// the migration ledger
	table("reinhardt", "reinhardt_migrations", DeploymentWide),
	// workspaces
	table("workspaces", "artifacts", Pending(265)),
	table("workspaces", "channel_attachments", Pending(265)),
	table("workspaces", "channel_message_context", Pending(265)),
	table("workspaces", "channel_threads", Pending(265)),
	table("workspaces", "conversations", Pending(265)),
	table("workspaces", "messages", Pending(265)),
	table("workspaces", "task_dependencies", Pending(265)),
	table("workspaces", "tasks", Pending(265)),
	table("workspaces", "workspaces", Pending(265)),
];

#[cfg(test)]
mod tests;
