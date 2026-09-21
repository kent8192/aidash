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
		]
	}
}
