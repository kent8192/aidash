use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;
fn a(name: &str) -> Alias {
	Alias::new(name)
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
		// DDL exception: SeaQuery cannot replace existing CHECK constraints.
		// Historical helpers retain their original strict flag set; only this
		// forward migration admits the three new, optional Boolean properties.
		for (table, name, expression) in super::m20260921_071045_record_constraints::web_checks() {
			if matches!(name, "registry_agent_config" | "packages_identity") {
				m.get_connection().execute_unprepared(&format!("ALTER TABLE \"{table}\" DROP CONSTRAINT \"{name}\", ADD CONSTRAINT \"{name}\" CHECK (COALESCE(({expression}), false))")).await?;
			}
		}
		m.create_table(
			Table::create()
				.table(a("web_runs"))
				.col(ColumnDef::new(a("run_id")).uuid().primary_key())
				.col(ColumnDef::new(a("tenant")).text().not_null())
				.col(ColumnDef::new(a("owner")).text().not_null())
				.col(ColumnDef::new(a("data")).json_binary().not_null())
				.foreign_key(
					ForeignKey::create()
						.from(a("web_runs"), a("run_id"))
						.to(a("runs"), a("id")),
				)
				.to_owned(),
		)
		.await?;
		m.create_table(
			Table::create()
				.table(a("web_invocations"))
				.col(ColumnDef::new(a("run_id")).uuid().not_null())
				.col(ColumnDef::new(a("invocation_key")).text().not_null())
				.col(
					ColumnDef::new(a("operation_id"))
						.uuid()
						.not_null()
						.unique_key(),
				)
				.primary_key(Index::create().col(a("run_id")).col(a("invocation_key")))
				.foreign_key(
					ForeignKey::create()
						.from(a("web_invocations"), a("run_id"))
						.to(a("runs"), a("id")),
				)
				.foreign_key(
					ForeignKey::create()
						.from(a("web_invocations"), a("operation_id"))
						.to(a("core_records"), a("id")),
				)
				.to_owned(),
		)
		.await?;
		m.create_table(
			Table::create()
				.table(a("web_months"))
				.col(ColumnDef::new(a("node_id")).text().not_null())
				.col(ColumnDef::new(a("account_id")).text().not_null())
				.col(ColumnDef::new(a("month")).text().not_null())
				.col(ColumnDef::new(a("used_micro_usd")).big_integer().not_null())
				.col(ColumnDef::new(a("base_micro_usd")).big_integer().not_null())
				.primary_key(
					Index::create()
						.col(a("node_id"))
						.col(a("account_id"))
						.col(a("month")),
				)
				.to_owned(),
		)
		.await?;
		m.create_table(
			Table::create()
				.table(a("web_nodes"))
				.col(ColumnDef::new(a("node_id")).text().primary_key())
				.col(ColumnDef::new(a("data")).json_binary().not_null())
				.to_owned(),
		)
		.await?;
		// DDL exception: SeaQuery 0.32 cannot express a JSON expression index.
		// This bounds per-Run evidence scans without indexing retained text.
		m.get_connection()
			.execute_unprepared(
				"CREATE INDEX web_record_run ON core_records (tenant, kind, (data->>'run_id'), id)",
			)
			.await?;
		Ok(())
	}
	async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
		for (table, name, expression) in
			super::m20260921_071045_record_constraints::workbench_checks()
		{
			if matches!(name, "registry_agent_config" | "packages_identity") {
				// Existing immutable versions with Web flags deliberately block
				// schema rollback. Operators can disable admission without deleting
				// published versions or retained observations.
				m.get_connection().execute_unprepared(&format!("ALTER TABLE \"{table}\" DROP CONSTRAINT \"{name}\", ADD CONSTRAINT \"{name}\" CHECK (COALESCE(({expression}), false))")).await?;
			}
		}
		m.drop_index(
			Index::drop()
				.name("web_record_run")
				.table(a("core_records"))
				.to_owned(),
		)
		.await?;
		for name in ["web_invocations", "web_runs", "web_months", "web_nodes"] {
			m.drop_table(Table::drop().table(a(name)).to_owned())
				.await?;
		}
		Ok(())
	}
}
