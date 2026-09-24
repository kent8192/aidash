use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.alter_table(
				Table::alter()
					.table(Alias::new("runs"))
					.add_column(
						ColumnDef::new(Alias::new("observed_input_seq"))
							.big_integer()
							.not_null()
							.default(0),
					)
					.to_owned(),
			)
			.await?;
		manager
			.create_table(
				Table::create()
					.table(Alias::new("run_inputs"))
					.col(
						ColumnDef::new(Alias::new("seq"))
							.big_integer()
							.auto_increment()
							.primary_key(),
					)
					.col(ColumnDef::new(Alias::new("run_id")).uuid().not_null())
					.col(ColumnDef::new(Alias::new("sender")).text().not_null())
					.col(ColumnDef::new(Alias::new("content")).text().not_null())
					.col(
						ColumnDef::new(Alias::new("idempotency_key"))
							.text()
							.not_null(),
					)
					.index(
						Index::create()
							.name("run_inputs_run_key")
							.unique()
							.col(Alias::new("run_id"))
							.col(Alias::new("idempotency_key")),
					)
					.foreign_key(
						ForeignKey::create()
							.from_tbl(Alias::new("run_inputs"))
							.from_col(Alias::new("run_id"))
							.to_tbl(Alias::new("runs"))
							.to_col(Alias::new("id"))
							.on_delete(ForeignKeyAction::Cascade),
					)
					.to_owned(),
			)
			.await?;
		// SeaQuery has no PostgreSQL trigger builder.
		manager
			.get_connection()
			.execute_unprepared(
				"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON run_inputs FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()",
			)
			.await?;
		Ok(())
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.drop_table(Table::drop().table(Alias::new("run_inputs")).to_owned())
			.await?;
		manager
			.alter_table(
				Table::alter()
					.table(Alias::new("runs"))
					.drop_column(Alias::new("observed_input_seq"))
					.to_owned(),
			)
			.await?;
		Ok(())
	}
}
