use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;
fn a(s: &str) -> Alias {
	Alias::new(s)
}
async fn replace_agent_checks(m: &SchemaManager<'_>, core: bool) -> Result<(), DbErr> {
	for (table, name, expression) in
		super::m20260921_071045_record_constraints::checks_extended(true, core)
	{
		if matches!(name, "registry_agent_config" | "packages_identity") {
			// SeaQuery cannot replace CHECK constraints on an existing table.
			m.get_connection().execute_unprepared(&format!("ALTER TABLE \"{table}\" DROP CONSTRAINT \"{name}\", ADD CONSTRAINT \"{name}\" CHECK (COALESCE(({expression}), false))")).await?;
		}
	}
	Ok(())
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
		replace_agent_checks(m, true).await?;
		let mut areas = Table::create();
		areas
			.table(a("core_areas"))
			.col(ColumnDef::new(a("id")).uuid().primary_key());
		for column in ["tenant", "home_node", "agent_id", "owner"] {
			areas.col(ColumnDef::new(a(column)).text().not_null());
		}
		for column in ["workspace_id", "thread_id"] {
			areas.col(ColumnDef::new(a(column)).uuid().not_null());
		}
		for column in ["generation", "revision", "epoch", "next_sequence"] {
			areas.col(
				ColumnDef::new(a(column))
					.big_integer()
					.not_null()
					.default(1),
			);
		}
		areas.col(
			ColumnDef::new(a("state"))
				.text()
				.not_null()
				.default("active"),
		);
		areas.col(
			ColumnDef::new(a("manifest"))
				.json_binary()
				.not_null()
				.default("[]"),
		);
		areas.col(
			ColumnDef::new(a("constraints"))
				.json_binary()
				.not_null()
				.default("[]"),
		);
		areas.index(
			Index::create()
				.name("core_session_identity")
				.unique()
				.col(a("tenant"))
				.col(a("home_node"))
				.col(a("workspace_id"))
				.col(a("thread_id"))
				.col(a("agent_id"))
				.col(a("owner")),
		);
		// No thread cascade: retained files keep their original ownership tombstone.
		m.create_table(areas.to_owned()).await?;
		m.create_table(
			Table::create()
				.table(a("core_runs"))
				.col(ColumnDef::new(a("run_id")).uuid().primary_key())
				.col(ColumnDef::new(a("area_id")).uuid().not_null())
				.col(ColumnDef::new(a("sequence")).big_integer().not_null())
				.col(
					ColumnDef::new(a("initialized"))
						.boolean()
						.not_null()
						.default(false),
				)
				.col(
					ColumnDef::new(a("generation"))
						.big_integer()
						.not_null()
						.default(1),
				)
				.foreign_key(
					ForeignKey::create()
						.from(a("core_runs"), a("run_id"))
						.to(a("runs"), a("id")),
				)
				.foreign_key(
					ForeignKey::create()
						.from(a("core_runs"), a("area_id"))
						.to(a("core_areas"), a("id")),
				)
				.index(
					Index::create()
						.name("core_run_order")
						.unique()
						.col(a("area_id"))
						.col(a("sequence")),
				)
				.to_owned(),
		)
		.await?;
		m.create_table(
			Table::create()
				.table(a("core_task_sessions"))
				.col(ColumnDef::new(a("task_id")).uuid().primary_key())
				.col(ColumnDef::new(a("thread_id")).uuid().not_null())
				.to_owned(),
		)
		.await?;
		m.create_table(
			Table::create()
				.table(a("core_quotas"))
				.col(ColumnDef::new(a("tenant")).text().primary_key())
				.col(
					ColumnDef::new(a("used_bytes"))
						.big_integer()
						.not_null()
						.default(0),
				)
				.to_owned(),
		)
		.await?;
		m.create_table(
			Table::create()
				.table(a("core_objects"))
				.col(ColumnDef::new(a("id")).uuid().primary_key())
				.col(ColumnDef::new(a("tenant")).text().not_null())
				.col(ColumnDef::new(a("area_id")).uuid())
				.col(ColumnDef::new(a("kind")).text().not_null())
				.col(ColumnDef::new(a("digest")).text().not_null())
				.col(ColumnDef::new(a("size")).big_integer().not_null())
				.col(
					ColumnDef::new(a("created_at"))
						.timestamp_with_time_zone()
						.not_null()
						.default(Expr::current_timestamp()),
				)
				.to_owned(),
		)
		.await?;
		m.create_table(
			Table::create()
				.table(a("core_requests"))
				.col(ColumnDef::new(a("tenant")).text().not_null())
				.col(ColumnDef::new(a("principal")).text().not_null())
				.col(ColumnDef::new(a("key")).uuid().not_null())
				.col(ColumnDef::new(a("digest")).text().not_null())
				.col(ColumnDef::new(a("result")).json_binary().not_null())
				.primary_key(
					Index::create()
						.col(a("tenant"))
						.col(a("principal"))
						.col(a("key")),
				)
				.to_owned(),
		)
		.await?;
		m.create_table(
			Table::create()
				.table(a("core_records"))
				.col(ColumnDef::new(a("id")).uuid().primary_key())
				.col(ColumnDef::new(a("tenant")).text().not_null())
				.col(ColumnDef::new(a("owner")).text().not_null())
				.col(ColumnDef::new(a("area_id")).uuid())
				.col(ColumnDef::new(a("kind")).text().not_null())
				.col(ColumnDef::new(a("state")).text().not_null())
				.col(
					ColumnDef::new(a("revision"))
						.big_integer()
						.not_null()
						.default(1),
				)
				.col(ColumnDef::new(a("data")).json_binary().not_null())
				.col(ColumnDef::new(a("expires_at")).timestamp_with_time_zone())
				.to_owned(),
		)
		.await?;
		// PostgreSQL trigger DDL is not expressible using SeaQuery.
		let mut operations = Table::create();
		operations
			.table(a("core_operations"))
			.col(ColumnDef::new(a("id")).uuid().primary_key());
		for column in ["area_id", "run_id", "credential_id"] {
			operations.col(ColumnDef::new(a(column)).uuid().not_null());
		}
		for column in [
			"tenant",
			"principal",
			"request_key",
			"digest",
			"kind",
			"state",
		] {
			operations.col(ColumnDef::new(a(column)).text().not_null());
		}
		for column in ["epoch", "generation", "revision", "policy_revision"] {
			operations.col(ColumnDef::new(a(column)).big_integer().not_null());
		}
		for column in ["subjects", "input", "result"] {
			operations.col(ColumnDef::new(a(column)).json_binary().not_null());
		}
		operations.col(ColumnDef::new(a("runner_instance")).text());
		for column in ["created_at", "updated_at"] {
			operations.col(
				ColumnDef::new(a(column))
					.timestamp_with_time_zone()
					.not_null()
					.default(Expr::current_timestamp()),
			);
		}
		operations.index(
			Index::create()
				.name("core_operation_key")
				.unique()
				.col(a("tenant"))
				.col(a("principal"))
				.col(a("request_key")),
		);
		m.create_table(operations.to_owned()).await?;
		for table in [
			"core_records",
			"core_operations",
			"core_areas",
			"core_runs",
			"core_quotas",
			"core_objects",
			"core_requests",
			"core_task_sessions",
		] {
			m.get_connection().execute_unprepared(&format!("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON {table} FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()")).await?;
		}
		Ok(())
	}
	async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
		// Refuse destructive rollback after users have created retained data.
		for retained in [
			"core_objects",
			"core_areas",
			"core_records",
			"core_operations",
			"core_requests",
		] {
			let count = m
				.get_connection()
				.query_one(
					m.get_database_backend().build(
						&Query::select()
							.expr(Expr::col(Asterisk).count())
							.from(a(retained))
							.to_owned(),
					),
				)
				.await?
				.ok_or_else(|| DbErr::Custom("cannot inspect capability objects".into()))?
				.try_get_by_index::<i64>(0)?;
			if count != 0 {
				return Err(DbErr::Custom(
					"disable capability admission instead of deleting retained objects".into(),
				));
			}
		}
		// This also rejects rollback while immutable Agent versions use new fields.
		replace_agent_checks(m, false).await?;
		for table in [
			"core_records",
			"core_operations",
			"core_requests",
			"core_objects",
			"core_quotas",
			"core_runs",
			"core_task_sessions",
			"core_areas",
		] {
			m.drop_table(Table::drop().table(a(table)).to_owned())
				.await?;
		}
		Ok(())
	}
}
