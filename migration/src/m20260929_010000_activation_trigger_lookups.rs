use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_index(
				Index::create()
					.name("activation_dependency_runs")
					.table(Alias::new("runs"))
					.col(Alias::new("task_id"))
					.and_where(Expr::col(Alias::new("phase")).is_not_in([
						"COMPLETED",
						"FAILED",
						"CANCELLED",
					]))
					.to_owned(),
			)
			.await?;
		// Function DDL is not expressible in SeaQuery. Upgrade already-provisioned
		// workers as well as fresh databases without changing migration history.
		manager
			.get_connection()
			.execute_unprepared(include_str!(
				"m20260929_010000_activation_trigger_lookups.sql"
			))
			.await?;
		Ok(())
	}
	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.drop_index(
				Index::drop()
					.name("activation_dependency_runs")
					.table(Alias::new("runs"))
					.to_owned(),
			)
			.await?;
		// Restore the previous function definitions, retaining the existing triggers.
		let previous = include_str!("m20260928_000000_worker_activation.sql")
			.split("CREATE TRIGGER")
			.next()
			.expect("activation functions")
			.replace("CREATE FUNCTION", "CREATE OR REPLACE FUNCTION");
		manager
			.get_connection()
			.execute_unprepared(&previous)
			.await?;
		Ok(())
	}
}
