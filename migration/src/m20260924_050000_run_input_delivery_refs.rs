use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.alter_table(
				Table::alter()
					.table(Alias::new("run_inputs"))
					.add_column(
						ColumnDef::new(Alias::new("delivery_retry_at")).timestamp_with_time_zone(),
					)
					.add_column(
						ColumnDef::new(Alias::new("reference_only"))
							.boolean()
							.not_null()
							.default(false),
					)
					.to_owned(),
			)
			.await
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.alter_table(
				Table::alter()
					.table(Alias::new("run_inputs"))
					.drop_column(Alias::new("reference_only"))
					.drop_column(Alias::new("delivery_retry_at"))
					.to_owned(),
			)
			.await
	}
}
