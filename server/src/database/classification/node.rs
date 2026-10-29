//! Explicit Node ownership (ADR 0083). A migration must classify every table.
//!
//! `Shared` means Tenant/Deployment-owned, never inferred from an incidental
//! Node address. `Recovery` retains exact recovery evidence and allocation accounting.
//! Roots and edges below are persistence contracts, not column-name discovery.
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
	Workspace,
	Run,
	Task,
	Area,
	Request,
	Bank,
	Unit,
	Entry,
	Grant,
	Admission,
	MemoryOperation,
	SemanticOperation,
	RemoteUsage,
	Thread,
	Operation,
	Observation,
	Message,
	RetentionHold,
	Preview,
	LivePreview,
	EgressSession,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ownership {
	Shared,
	Recovery,
	Node(&'static str),
	Subject(&'static str),
	Link(&'static str, Root),
	/// Owned through either nullable link; each row sets at least one.
	Either(&'static str, Root, &'static str, Root),
	Areas,
	Records,
	Objects,
	Dispatches,
	SemanticJournals,
	Registrations,
	SourceReads,
	RemoteUsageRows,
	Finalizations,
	CascadeTasks,
	Approvals,
	Audit,
	Event,
}
use Ownership::*;
use Root::*;

/// Tenant-owned tables require an explicit decision. Canonical DeploymentWide
/// classification is itself proof that a reviewed table survives Node purge.
pub fn ownership(name: &str) -> Result<Ownership> {
	Ok(match name {
		"workspaces"
		| "runs"
		| "generation_requests"
		| "node_cancellation_jobs"
		| "authorization_catalog"
		| "node_registration_history"
		| "authorization_peer_mappings"
		| "authorization_peer_mapping_history"
		| "authorization_remote_admissions"
		| "authorization_remote_grants" => Node("node"),
		"nodes" => Audit,
		"authorization_remote_terminal_receipts" => Recovery,
		"node_registrations" => Registrations,
		"core_areas" => Areas,
		"core_records" => Records,
		"core_objects" => Objects,
		"task_dependencies" => CascadeTasks,
		"authorization_catalog_history" | "dashboard_administration_history" | "node_deletions" => {
			Audit
		}
		// Safety Stop audit outlives its Run and even its Tenant (ADR 0101).
		"run_safety_stops" => Audit,
		"authorization_credentials"
		| "identity_approval_preferences"
		| "identity_approval_rules" => Subject("subject"),
		"registry_requests" | "core_requests" => Subject("principal"),
		"identity_thread_approval_modes" => Link("thread_id", Thread),
		"approval_observations" | "approval_observation_population" => Approvals,
		"approval_observation_attempts" => Link("observation", Observation),
		"tasks"
		| "artifacts"
		| "messages"
		| "conversations"
		| "channel_threads"
		| "channel_attachments"
		| "channel_message_context"
		| "authorization_workspaces"
		| "semantic_indexes"
		| "semantic_entries"
		| "semantic_collections"
		| "semantic_history"
		| "semantic_vectors"
		| "memory_banks"
		| "memory_participants" => Link("workspace_id", Workspace),
		"core_operations" => Link("area_id", Area),
		"core_runs" => Link("run_id", Run),
		"core_task_sessions"
		| "delegations"
		| "memory_task_participants"
		| "generation_remote_intents" => Link("task_id", Task),
		"authorization_task_origins" => Link("task_id", Task),
		"url_intents"
		| "authorization_execution"
		| "authorization_run_reads"
		| "authorization_run_registry_reads"
		| "authorization_run_remote_reads"
		| "authorization_run_outputs"
		| "dashboard_execution_origins"
		| "run_provider_credentials"
		| "run_plan_connections"
		| "run_context_events"
		| "run_activations"
		| "inference_progress"
		| "decision_states"
		| "research_revocations"
		| "research_fragments"
		| "research_originals"
		| "research_page_images"
		| "research_snapshots"
		| "memory_run_bindings"
		| "memory_run_reads"
		| "memory_run_read_gates"
		| "memory_receiver_caches"
		| "semantic_run_reads"
		| "context_compaction_attempts"
		| "decision_attempts"
		| "decision_evidence"
		| "decision_outcomes"
		| "generation_usage"
		| "generation_compaction_usage"
		| "generation_summary_usage"
		| "human_requests"
		| "inference_attempts"
		| "inference_usage"
		| "invocations"
		| "research_attempts"
		| "research_journal_results"
		| "research_sources"
		| "run_inputs"
		| "run_task_snapshots" => Link("run_id", Run),
		"generation_budgets" => Link("request_id", Request),
		"generation_remote_usage" => RemoteUsageRows,
		"generation_embedding_usage" => Link("workspace_id", Workspace),
		"generation_history" => Link("request_id", Request),
		"generation_remote_finalizations" => Finalizations,
		"generation_remote_dispatches" => Dispatches,
		"authorization_remote_commands"
		| "authorization_remote_execution"
		| "authorization_remote_outputs"
		| "authorization_remote_grant_reads"
		| "memory_remote_read_gates" => Link("grant_id", Grant),
		"memory_remote_reads" => Link("grant_id", Grant),
		"semantic_remote_reads" => SourceReads,
		"semantic_remote_operations" => SemanticJournals,
		"semantic_remote_attempts" => Link("operation_id", SemanticOperation),
		"semantic_remote_receipts" => Link("run_id", Run),
		"semantic_points" => Link("entry_id", Entry),
		"memory_bank_decay"
		| "memory_bank_settings"
		| "memory_units"
		| "memory_history"
		| "memory_unit_retention"
		| "memory_dependencies"
		| "memory_candidates"
		| "memory_receipts"
		| "memory_model_operations"
		| "memory_publications"
		| "memory_candidate_reviews"
		| "memory_purge_jobs"
		| "memory_engine_jobs"
		| "memory_bank_policy_changes" => Link("bank_id", Bank),
		"memory_unit_origins" | "memory_unit_run_origins" => Link("unit_id", Unit),
		"memory_model_attempts" => Link("operation_id", MemoryOperation),
		"remote_run_message_fences" => Link("task_id", Task),
		// A Live preview's Egress request has no core operation (develop #405).
		"egress_requests" => Either("operation_id", Operation, "session_id", EgressSession),
		"egress_budgets" => Link("operation_id", Operation),
		"core_retention_holds" => Link("area_id", Area),
		"core_retention_files" => Link("hold_id", RetentionHold),
		"message_file_versions" => Link("message_id", Message),
		"core_previews" => Either("area_id", Area, "live_preview_id", LivePreview),
		"core_preview_tickets" | "core_preview_sessions" => Link("preview_id", Preview),
		"core_live_previews" => Either("run_id", Run, "area_id", Area),
		"egress_sessions" => Either("run_id", Run, "live_preview_id", LivePreview),
		"events" => Event,
		"atomic_authority_attempts"
		| "atomic_coordinators"
		| "atomic_gate"
		| "atomic_history"
		| "atomic_participants"
		| "atomic_preflights"
		| "atomic_subjects"
		| "atomic_votes"
		| "deployment_key_rollovers"
		| "runner_allocation_journals" => Recovery,
		"activation_quarantine"
		| "approval_observer_settings"
		| "core_quotas"
		| "core_preview_limits"
		| "estimate_calibration_samples"
		| "generation_policies"
		| "generation_policy_history"
		| "inbox"
		| "research_account_terminations"
		| "authorization_bundles"
		| "authorization_decisions"
		| "authorization_revisions"
		| "credential_store_keys"
		| "credential_store_resources"
		| "credential_store_versions"
		| "dashboard_identities"
		| "dashboard_login_transactions"
		| "dashboard_logout_tokens"
		| "dashboard_mappings"
		| "dashboard_deployment_administrator_grants"
		| "dashboard_registration_requests"
		| "dashboard_sessions"
		| "demo_data_loads"
		| "demo_reservations"
		| "demo_tenants"
		| "desktop_handoffs"
		| "desktop_refresh_credentials"
		| "node_names"
		| "identity_tenant_approval_settings"
		| "personal_tenants"
		| "plan_authorizations"
		| "plan_connection_hosts"
		| "plan_connections"
		| "provider_authorizations"
		| "provider_credential_bindings"
		| "provider_credentials"
		| "tenant_invitations"
		| "tenant_retention_rule_history"
		| "tenant_retention_rules"
		| "tenant_secrets"
		| "tenant_deletions"
		| "tenants"
		| "memory_learning_discovery"
		| "semantic_vector_collections"
		| "marketplace_audiences"
		| "marketplace_consents"
		| "marketplace_installations"
		| "marketplace_provenance"
		| "marketplace_requests"
		| "marketplace_revisions"
		| "marketplace_versions"
		| "agent_draft_registrations"
		| "agent_draft_shares"
		| "agent_drafts"
		| "agent_incident_events"
		| "agent_incidents"
		| "agent_knowledge"
		| "agent_plugin_copies"
		| "agent_test_limits"
		| "agent_test_profiles"
		| "agent_test_sessions"
		| "registry"
		| "registry_agent_model_refs"
		| "registry_agent_resource_refs"
		| "registry_memory_role_refs"
		| "reinhardt_migrations"
		| "billing_accounts"
		| "billing_attachment_consents"
		| "billing_attachments"
		| "billing_pod_usage"
		| "billing_storage_samples"
		| "billing_subscriptions"
		| "billing_coverage_periods"
		| "gcip_admissions" => Shared,
		_ => {
			// A reviewed build's canonical DeploymentWide classification itself
			// proves that this table has no Node owner. New Tenant-owned tables
			// still require an explicit decision above; unlisted catalog tables
			// never receive this fallback.
			if crate::bootstrap::build::current().tables().any(|table| {
				table.name == name && table.classification == super::Classification::DeploymentWide
			}) {
				Shared
			} else {
				return Err(Error::Conflict(format!(
					"Node purge has no ownership classification for table {name}"
				)));
			}
		}
	})
}

#[cfg(test)]
mod tests;
