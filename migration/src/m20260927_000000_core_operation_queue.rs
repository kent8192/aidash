use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_index(
				Index::create()
					.name("core_operation_pending_queue")
					.table(Alias::new("core_operations"))
					.col(Alias::new("updated_at"))
					.and_where(Expr::col(Alias::new("state")).is_in([
						"prepared",
						"submitted",
						"running",
						"cancelling",
					]))
					.to_owned(),
			)
			.await
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.drop_index(
				Index::drop()
					.name("core_operation_pending_queue")
					.table(Alias::new("core_operations"))
					.to_owned(),
			)
			.await
	}
}
