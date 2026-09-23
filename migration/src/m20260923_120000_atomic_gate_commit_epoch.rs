use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.alter_table(
				Table::alter()
					.table(Alias::new("atomic_gate"))
					.add_column(
						ColumnDef::new(Alias::new("commit_epoch"))
							.big_integer()
							.not_null()
							.default(0),
					)
					.to_owned(),
			)
			.await
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.alter_table(
				Table::alter()
					.table(Alias::new("atomic_gate"))
					.drop_column(Alias::new("commit_epoch"))
					.to_owned(),
			)
			.await
	}
}
