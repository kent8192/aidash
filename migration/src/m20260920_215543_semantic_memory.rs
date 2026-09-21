// SeaQuery 0.32 cannot express PostgreSQL trigger functions, triggers, or ALTER CHECK constraints.
// Those DDL operations intentionally use SeaORM execution; ordinary queries use builders.
use sea_orm_migration::prelude::*;
#[derive(DeriveMigrationName)]
pub struct Migration;
fn col(name: &str) -> ColumnDef {
	ColumnDef::new(Alias::new(name))
}
fn now(name: &str) -> ColumnDef {
	col(name)
		.timestamp_with_time_zone()
		.not_null()
		.default(Expr::current_timestamp())
		.to_owned()
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
		m.create_table(
			Table::create()
				.table(Alias::new("semantic_indexes"))
				.col(col("workspace_id").uuid().not_null().primary_key())
				.col(col("tenant").text().not_null())
				.col(col("revision").big_integer().not_null())
				.col(col("spec").json_binary().not_null())
				.col(col("collection").text().not_null().unique_key())
				.col(now("updated_at"))
				.foreign_key(
					ForeignKey::create()
						.from(Alias::new("semantic_indexes"), Alias::new("workspace_id"))
						.to(Alias::new("workspaces"), Alias::new("id")),
				)
				.to_owned(),
		)
		.await?;
		m.create_table(
			Table::create()
				.table(Alias::new("semantic_collections"))
				.col(col("collection").text().not_null().primary_key())
				.col(col("workspace_id").uuid().not_null())
				.col(col("vector").json_binary().not_null())
				.col(col("retired").boolean().not_null().default(false))
				.col(col("last_error").text())
				.col(col("cleaned_at").timestamp_with_time_zone())
				.col(now("next_attempt"))
				.foreign_key(
					ForeignKey::create()
						.from(
							Alias::new("semantic_collections"),
							Alias::new("workspace_id"),
						)
						.to(Alias::new("semantic_indexes"), Alias::new("workspace_id")),
				)
				.to_owned(),
		)
		.await?;
		m.create_table(
			Table::create()
				.table(Alias::new("semantic_entries"))
				.col(col("id").uuid().not_null().primary_key())
				.col(col("workspace_id").uuid().not_null())
				.col(col("key").text().not_null())
				.col(col("source").json_binary().not_null())
				.col(col("agent").text())
				.col(col("metadata").json_binary().not_null())
				.col(col("revision").big_integer().not_null())
				.col(col("point_id").uuid().not_null())
				.col(col("index_revision").big_integer().not_null())
				.col(col("deleted").boolean().not_null().default(false))
				.col(
					col("state").text().not_null().check(
						Expr::col(Alias::new("state"))
							.is_in(["PENDING", "READY", "ERROR", "REVOKED", "DELETED"]),
					),
				)
				.col(col("attempts").integer().not_null().default(0))
				.col(col("last_error").text())
				.col(col("created_by").text().not_null())
				.col(col("authority").json_binary().not_null())
				.col(now("updated_at"))
				.col(now("next_attempt"))
				.index(
					Index::create()
						.unique()
						.col(Alias::new("workspace_id"))
						.col(Alias::new("key")),
				)
				.foreign_key(
					ForeignKey::create()
						.from(Alias::new("semantic_entries"), Alias::new("workspace_id"))
						.to(Alias::new("semantic_indexes"), Alias::new("workspace_id")),
				)
				.to_owned(),
		)
		.await?;
		m.create_table(
			Table::create()
				.table(Alias::new("semantic_points"))
				.col(col("id").uuid().not_null().primary_key())
				.col(col("entry_id").uuid().not_null())
				.col(col("content_digest").text())
				.col(col("collection").text().not_null())
				.col(col("retired").boolean().not_null().default(false))
				.col(col("last_error").text())
				.col(col("cleaned_at").timestamp_with_time_zone())
				.col(now("next_attempt"))
				.foreign_key(
					ForeignKey::create()
						.from(Alias::new("semantic_points"), Alias::new("entry_id"))
						.to(Alias::new("semantic_entries"), Alias::new("id")),
				)
				.foreign_key(
					ForeignKey::create()
						.from(Alias::new("semantic_points"), Alias::new("collection"))
						.to(Alias::new("semantic_collections"), Alias::new("collection")),
				)
				.to_owned(),
		)
		.await?;
		m.create_table(
			Table::create()
				.table(Alias::new("semantic_history"))
				.col(
					col("sequence")
						.big_integer()
						.not_null()
						.auto_increment()
						.primary_key(),
				)
				.col(col("workspace_id").uuid().not_null())
				.col(col("entry_id").uuid())
				.col(col("revision").big_integer().not_null())
				.col(col("state").text().not_null())
				.col(col("detail").text().not_null())
				.col(now("created_at"))
				.to_owned(),
		)
		.await?;
		m.create_table(
			Table::create()
				.table(Alias::new("semantic_run_reads"))
				.col(col("run_id").uuid().not_null())
				.col(col("entry_id").uuid().not_null())
				.col(col("revision").big_integer().not_null())
				.primary_key(
					Index::create()
						.col(Alias::new("run_id"))
						.col(Alias::new("entry_id"))
						.col(Alias::new("revision")),
				)
				.foreign_key(
					ForeignKey::create()
						.from(Alias::new("semantic_run_reads"), Alias::new("run_id"))
						.to(Alias::new("runs"), Alias::new("id")),
				)
				.foreign_key(
					ForeignKey::create()
						.from(Alias::new("semantic_run_reads"), Alias::new("entry_id"))
						.to(Alias::new("semantic_entries"), Alias::new("id")),
				)
				.to_owned(),
		)
		.await?;
		m.create_table(
			Table::create()
				.table(Alias::new("semantic_agent_memory"))
				.col(col("entry_id").uuid().not_null().primary_key())
				.col(col("workspace_id").uuid().not_null())
				.col(col("agent_id").text().not_null())
				.col(col("agent_version").text().not_null())
				.col(col("home_node").text().not_null())
				.foreign_key(
					ForeignKey::create()
						.from(Alias::new("semantic_agent_memory"), Alias::new("entry_id"))
						.to(Alias::new("semantic_entries"), Alias::new("id")),
				)
				.to_owned(),
		)
		.await?;
		for table in [
			"semantic_indexes",
			"semantic_collections",
			"semantic_entries",
			"semantic_points",
			"semantic_history",
			"semantic_run_reads",
			"semantic_agent_memory",
		] {
			// PostgreSQL statement trigger syntax has no SeaQuery equivalent.
			m.get_connection().execute_unprepared(&format!("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON {table} FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()")).await?;
		}
		Ok(())
	}
	async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
		for table in [
			"semantic_agent_memory",
			"semantic_run_reads",
			"semantic_history",
			"semantic_points",
			"semantic_entries",
			"semantic_collections",
			"semantic_indexes",
		] {
			m.drop_table(Table::drop().table(Alias::new(table)).to_owned())
				.await?;
		}
		Ok(())
	}
}
