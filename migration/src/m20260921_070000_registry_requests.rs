use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;
#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_table(
				Table::create()
					.table(Alias::new("registry_requests"))
					.col(ColumnDef::new(Alias::new("key")).uuid().primary_key())
					.col(
						ColumnDef::new(Alias::new("request"))
							.json_binary()
							.not_null(),
					)
					.col(ColumnDef::new(Alias::new("entity_id")).text().not_null())
					.to_owned(),
			)
			.await?;
		// SeaQuery has no PostgreSQL CREATE TRIGGER builder.
		manager.get_connection().execute_unprepared("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON registry_requests FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()").await?;
		Ok(())
	}
	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.drop_table(
				Table::drop()
					.table(Alias::new("registry_requests"))
					.to_owned(),
			)
			.await
	}
}
