use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

async fn replace_checks(manager: &SchemaManager<'_>, personal_agents: bool) -> Result<(), DbErr> {
	for (table, name, expression) in
		super::m20260921_071045_record_constraints::checks(personal_agents)
	{
		if !matches!(name, "registry_agent_config" | "packages_identity") {
			continue;
		}
		// SeaQuery cannot add/drop CHECK constraints on existing tables. Keep
		// this DDL in the transactional migration and preserve all other checks.
		manager.get_connection().execute_unprepared(&format!(
			"ALTER TABLE \"{table}\" DROP CONSTRAINT \"{name}\", ADD CONSTRAINT \"{name}\" CHECK (COALESCE(({expression}), false))"
		)).await?;
	}
	Ok(())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		replace_checks(manager, true).await
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		replace_checks(manager, false).await
	}
}
