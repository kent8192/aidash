pub use sea_orm_migration::prelude::*;
mod m20260920_200623_m0001_core;
mod m20260920_200623_m0002_federation;
mod m20260920_200623_m0003_task_abandonment;
mod m20260920_200623_m0004_authorization;
mod m20260920_200623_m0005_subject_credentials;
mod m20260920_200623_m0006_execution_authority;
mod m20260920_200623_m0007_interaction_authority;
mod m20260920_200623_m0008_generation;
mod m20260920_200623_m0009_generation_compaction;
mod m20260920_200623_m0010_delivery_retries;
mod m20260920_204136_m0011_resource_reads;
mod m20260920_210304_review_isolation;
mod m20260920_211600_atomic_transactions;
mod m20260920_215543_semantic_memory;
mod m20260920_235900_generation_embeddings;
mod m20260921_003656_authorization_peer_mappings;
mod m20260921_010628_authorization_remote_reads;
mod m20260921_014954_authorization_remote_grants;
mod m20260921_021051_authorization_remote_admissions;
mod m20260921_024105_authorization_remote_grant_reads;
mod m20260921_070000_registry_requests;
mod m20260921_071045_record_constraints;

mod m20260921_230000_agent_knowledge;
mod m20260922_080000_personal_agent_constraints;
mod m20260922_134000_inference_timeout_constraints;
mod m20260923_010000_channel_threads;
mod m20260923_020000_channel_attachments;
mod m20260923_120000_atomic_gate_commit_epoch;
mod m20260923_190000_dashboard_oidc;
mod m20260924_020000_dashboard_oidc_indexes;
mod m20260924_030000_dashboard_session_revocation_indexes;
mod m20260924_040000_run_inputs;
mod m20260924_050000_run_input_delivery_refs;
mod m20260924_060000_legacy_run_input_bridge;
mod m20260924_070000_legacy_run_input_gate;
mod m20260924_080000_legacy_federated_input_gate;
mod m20260924_090000_worker_ledger_fence;
mod m20260924_100000_remote_message_fence;
mod m20260925_000000_agent_workbenches;
mod m20260925_140000_core_working_files;
mod m20260926_010000_scoped_remote_execution;
mod m20260927_000000_core_operation_queue;

pub struct Migrator;
#[async_trait::async_trait]
impl MigratorTrait for Migrator {
	fn migrations() -> Vec<Box<dyn MigrationTrait>> {
		vec![
			Box::new(m20260920_200623_m0001_core::Migration),
			Box::new(m20260920_200623_m0002_federation::Migration),
			Box::new(m20260920_200623_m0003_task_abandonment::Migration),
			Box::new(m20260920_200623_m0004_authorization::Migration),
			Box::new(m20260920_200623_m0005_subject_credentials::Migration),
			Box::new(m20260920_200623_m0006_execution_authority::Migration),
			Box::new(m20260920_200623_m0007_interaction_authority::Migration),
			Box::new(m20260920_200623_m0008_generation::Migration),
			Box::new(m20260920_200623_m0009_generation_compaction::Migration),
			Box::new(m20260920_200623_m0010_delivery_retries::Migration),
			Box::new(m20260920_204136_m0011_resource_reads::Migration),
			Box::new(m20260920_210304_review_isolation::Migration),
			Box::new(m20260920_211600_atomic_transactions::Migration),
			Box::new(m20260920_215543_semantic_memory::Migration),
			Box::new(m20260920_235900_generation_embeddings::Migration),
			Box::new(m20260921_003656_authorization_peer_mappings::Migration),
			Box::new(m20260921_010628_authorization_remote_reads::Migration),
			Box::new(m20260921_014954_authorization_remote_grants::Migration),
			Box::new(m20260921_021051_authorization_remote_admissions::Migration),
			Box::new(m20260921_024105_authorization_remote_grant_reads::Migration),
			Box::new(m20260921_070000_registry_requests::Migration),
			Box::new(m20260921_071045_record_constraints::Migration),
			Box::new(m20260921_230000_agent_knowledge::Migration),
			Box::new(m20260922_080000_personal_agent_constraints::Migration),
			Box::new(m20260922_134000_inference_timeout_constraints::Migration),
			Box::new(m20260923_010000_channel_threads::Migration),
			Box::new(m20260923_020000_channel_attachments::Migration),
			Box::new(m20260923_120000_atomic_gate_commit_epoch::Migration),
			Box::new(m20260923_190000_dashboard_oidc::Migration),
			Box::new(m20260924_020000_dashboard_oidc_indexes::Migration),
			Box::new(m20260924_030000_dashboard_session_revocation_indexes::Migration),
			Box::new(m20260924_040000_run_inputs::Migration),
			Box::new(m20260924_050000_run_input_delivery_refs::Migration),
			Box::new(m20260924_060000_legacy_run_input_bridge::Migration),
			Box::new(m20260924_070000_legacy_run_input_gate::Migration),
			Box::new(m20260924_080000_legacy_federated_input_gate::Migration),
			Box::new(m20260924_090000_worker_ledger_fence::Migration),
			Box::new(m20260924_100000_remote_message_fence::Migration),
			Box::new(m20260925_000000_agent_workbenches::Migration),
			Box::new(m20260925_140000_core_working_files::Migration),
			Box::new(m20260926_010000_scoped_remote_execution::Migration),
			Box::new(m20260927_000000_core_operation_queue::Migration),
		]
	}
}
