use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_table(
				Table::create()
					.table(Alias::new("authorization_graph_operator_grants"))
					.col(ColumnDef::new(Alias::new("source_node")).text().not_null())
					.col(
						ColumnDef::new(Alias::new("source_operator"))
							.uuid()
							.not_null(),
					)
					.col(ColumnDef::new(Alias::new("tenant")).text().not_null())
					.col(ColumnDef::new(Alias::new("enabled")).boolean().not_null())
					.col(
						ColumnDef::new(Alias::new("revision"))
							.big_integer()
							.not_null()
							.check(Expr::col(Alias::new("revision")).gt(0)),
					)
					.col(
						ColumnDef::new(Alias::new("updated_at"))
							.timestamp_with_time_zone()
							.not_null()
							.default(Expr::cust("clock_timestamp()")),
					)
					.primary_key(
						Index::create()
							.col(Alias::new("source_node"))
							.col(Alias::new("source_operator"))
							.col(Alias::new("tenant")),
					)
					.foreign_key(
						ForeignKey::create()
							.from(
								Alias::new("authorization_graph_operator_grants"),
								Alias::new("tenant"),
							)
							.to(Alias::new("authorization_bundles"), Alias::new("tenant")),
					)
					.to_owned(),
			)
			.await?;
		// SeaQuery does not express PostgreSQL trigger attachment.
		manager.get_connection().execute_unprepared("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON authorization_graph_operator_grants FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()").await?;
		Ok(())
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.drop_table(
				Table::drop()
					.table(Alias::new("authorization_graph_operator_grants"))
					.to_owned(),
			)
			.await?;
		Ok(())
	}
}
