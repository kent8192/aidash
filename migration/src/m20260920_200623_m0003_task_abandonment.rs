// SeaQuery 0.32 cannot express PostgreSQL trigger functions, triggers, or ALTER CHECK constraints.
// Those DDL operations intentionally use SeaORM execution; ordinary queries use builders.
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		// SeaQuery 0.32 has no ALTER TABLE CHECK constraint builder.
		manager
			.get_connection()
			.execute_unprepared("ALTER TABLE tasks DROP CONSTRAINT tasks_status_check")
			.await?;
		// SeaQuery 0.32 has no ALTER TABLE CHECK constraint builder.
		manager.get_connection().execute_unprepared("ALTER TABLE tasks ADD CONSTRAINT tasks_status_check CHECK (status IN ('OPEN','CLAIMED','RUNNING','COMPLETED','FAILED','BLOCKED','CANCELLED','ABANDONED'))").await?;
		Ok(())
	}
	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager.get_connection().execute_unprepared("ALTER TABLE tasks DROP CONSTRAINT tasks_status_check; ALTER TABLE tasks ADD CONSTRAINT tasks_status_check CHECK (status IN ('OPEN','CLAIMED','RUNNING','COMPLETED','FAILED','BLOCKED','CANCELLED'))").await?;
		Ok(())
	}
}
