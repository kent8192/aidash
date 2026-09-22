use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;
#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_table(
				Table::create()
					.table(Alias::new("agent_knowledge"))
					.col(ColumnDef::new(Alias::new("agent_id")).text().not_null())
					.col(
						ColumnDef::new(Alias::new("agent_version"))
							.text()
							.not_null(),
					)
					.col(
						ColumnDef::new(Alias::new("documents"))
							.json_binary()
							.not_null(),
					)
					.primary_key(
						Index::create()
							.col(Alias::new("agent_id"))
							.col(Alias::new("agent_version")),
					)
					.foreign_key(
						ForeignKey::create()
							.from_tbl(Alias::new("agent_knowledge"))
							.from_col(Alias::new("agent_id"))
							.from_col(Alias::new("agent_version"))
							.to_tbl(Alias::new("registry"))
							.to_col(Alias::new("id"))
							.to_col(Alias::new("version")),
					)
					.to_owned(),
			)
			.await?;
		// SeaQuery has no PostgreSQL CREATE TRIGGER builder.
		manager.get_connection().execute_unprepared("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON agent_knowledge FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()").await?;
		Ok(())
	}
	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.drop_table(
				Table::drop()
					.table(Alias::new("agent_knowledge"))
					.to_owned(),
			)
			.await
	}
}
