use sea_orm_migration::prelude::*;
#[derive(DeriveMigrationName)]
pub struct Migration;
fn col(name: &str) -> ColumnDef {
	ColumnDef::new(Alias::new(name))
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_table(
				Table::create()
					.table(Alias::new("authorization_remote_grant_reads"))
					.col(col("grant_id").uuid().not_null())
					.col(col("workspace_id").uuid().not_null())
					.col(col("resource_kind").text().not_null())
					.col(col("resource_id").uuid().not_null())
					.primary_key(
						Index::create()
							.col(Alias::new("grant_id"))
							.col(Alias::new("workspace_id"))
							.col(Alias::new("resource_kind"))
							.col(Alias::new("resource_id")),
					)
					.foreign_key(
						ForeignKey::create()
							.from(
								Alias::new("authorization_remote_grant_reads"),
								Alias::new("grant_id"),
							)
							.to(Alias::new("authorization_remote_grants"), Alias::new("id"))
							.on_delete(ForeignKeyAction::Cascade),
					)
					.to_owned(),
			)
			.await?;
		// SeaQuery has no PostgreSQL CREATE TRIGGER builder.
		manager.get_connection().execute_unprepared("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON authorization_remote_grant_reads FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()").await?;
		Ok(())
	}
	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.drop_table(
				Table::drop()
					.table(Alias::new("authorization_remote_grant_reads"))
					.to_owned(),
			)
			.await
	}
}
