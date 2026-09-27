use sea_orm_migration::prelude::*;
#[derive(DeriveMigrationName)]
pub struct Migration;
fn c(name: &str) -> ColumnDef {
	ColumnDef::new(Alias::new(name))
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_table(
				Table::create()
					.table(Alias::new("authorization_remote_execution"))
					.col(c("grant_id").uuid().not_null().primary_key())
					.col(c("admission_id").uuid().not_null().unique_key())
					.col(c("task_id").uuid().not_null().unique_key())
					.col(c("task_revision").big_integer().not_null())
					.col(c("initial_task").json_binary().not_null())
					.col(
						c("created_at")
							.timestamp_with_time_zone()
							.not_null()
							.default(Expr::current_timestamp()),
					)
					.to_owned(),
			)
			.await?;
		manager
			.create_table(
				Table::create()
					.table(Alias::new("authorization_remote_commands"))
					.col(c("grant_id").uuid().not_null())
					.col(c("request_key").text().not_null())
					.col(c("digest").text().not_null())
					.col(c("result").json_binary().not_null())
					.primary_key(
						Index::create()
							.col(Alias::new("grant_id"))
							.col(Alias::new("request_key")),
					)
					.to_owned(),
			)
			.await?;
		manager
			.create_table(
				Table::create()
					.table(Alias::new("authorization_remote_outputs"))
					.col(c("grant_id").uuid().not_null())
					.col(c("workspace_id").uuid().not_null())
					.col(c("resource_kind").text().not_null())
					.col(c("resource_id").uuid().not_null())
					.primary_key(
						Index::create()
							.col(Alias::new("grant_id"))
							.col(Alias::new("workspace_id"))
							.col(Alias::new("resource_kind"))
							.col(Alias::new("resource_id")),
					)
					.to_owned(),
			)
			.await?;
		for table in [
			"authorization_remote_outputs",
			"authorization_remote_execution",
			"authorization_remote_commands",
		] {
			// PostgreSQL statement-trigger DDL has no SeaQuery representation.
			manager.get_connection().execute_unprepared(&format!("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON {table} FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()")).await?;
		}
		Ok(())
	}
	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		for table in [
			"authorization_remote_outputs",
			"authorization_remote_commands",
			"authorization_remote_execution",
		] {
			manager
				.drop_table(Table::drop().table(Alias::new(table)).to_owned())
				.await?;
		}
		Ok(())
	}
}
