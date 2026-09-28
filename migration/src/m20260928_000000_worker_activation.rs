use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;
fn a(name: &str) -> Alias {
	Alias::new(name)
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
		m.create_table(
			Table::create()
				.table(a("run_activations"))
				.col(
					ColumnDef::new(a("generation"))
						.big_integer()
						.auto_increment()
						.primary_key(),
				)
				.col(
					ColumnDef::new(a("id"))
						.uuid()
						.not_null()
						.unique_key()
						.default(Expr::cust("gen_random_uuid()")),
				)
				.col(ColumnDef::new(a("run_id")).uuid().not_null())
				.col(ColumnDef::new(a("run_revision")).big_integer().not_null())
				.col(ColumnDef::new(a("reason")).text().not_null())
				.col(
					ColumnDef::new(a("state"))
						.text()
						.not_null()
						.default("pending")
						.check(Expr::cust(
							"state IN ('pending','claimed','deferred','settled')",
						)),
				)
				.col(
					ColumnDef::new(a("due_at"))
						.timestamp_with_time_zone()
						.default(Expr::cust("CURRENT_TIMESTAMP")),
				)
				.col(
					ColumnDef::new(a("publication_epoch"))
						.big_integer()
						.not_null()
						.default(0),
				)
				.col(ColumnDef::new(a("publish_token")).uuid())
				.col(ColumnDef::new(a("publish_until")).timestamp_with_time_zone())
				.col(ColumnDef::new(a("published_at")).timestamp_with_time_zone())
				.col(ColumnDef::new(a("lease_token")).uuid())
				.col(ColumnDef::new(a("claimed_at")).timestamp_with_time_zone())
				.col(ColumnDef::new(a("claim_source")).text())
				.col(ColumnDef::new(a("worker_pid")).big_integer())
				.col(ColumnDef::new(a("disposition")).text())
				.col(
					ColumnDef::new(a("created_at"))
						.timestamp_with_time_zone()
						.not_null()
						.default(Expr::cust("CURRENT_TIMESTAMP")),
				)
				.to_owned(),
		)
		.await?;
		// Deliberately no Run FK: retained dispositions survive Run deletion, and
		// cross-Run unblock triggers must not acquire FK locks out of Run order.
		m.create_index(
			Index::create()
				.name("activation_due")
				.table(a("run_activations"))
				.col(a("due_at"))
				.col(a("generation"))
				.and_where(Expr::col(a("state")).ne("settled"))
				.to_owned(),
		)
		.await?;
		m.create_index(
			Index::create()
				.name("activation_run_generation")
				.table(a("run_activations"))
				.col(a("run_id"))
				.col(a("generation"))
				.to_owned(),
		)
		.await?;
		m.create_table(
			Table::create()
				.table(a("activation_quarantine"))
				.col(ColumnDef::new(a("digest")).text().primary_key())
				.col(ColumnDef::new(a("reason")).text().not_null())
				.col(ColumnDef::new(a("stream_sequence")).big_integer())
				.col(
					ColumnDef::new(a("created_at"))
						.timestamp_with_time_zone()
						.not_null()
						.default(Expr::cust("CURRENT_TIMESTAMP")),
				)
				.to_owned(),
		)
		.await?;
		// SeaQuery cannot express PostgreSQL trigger/function DDL. These triggers
		// make the shared handoff atomic even for compatible old writers. They
		// append obligations; they never update/lock another Run or publish I/O.
		m.get_connection()
			.execute_unprepared(include_str!("m20260928_000000_worker_activation.sql"))
			.await?;
		Ok(())
	}
	async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
		// Trigger/function DDL is not expressible in SeaQuery.
		for table in [
			"runs",
			"tasks",
			"run_inputs",
			"human_requests",
			"core_records",
			"core_runs",
			"core_areas",
		] {
			m.get_connection()
				.execute_unprepared(&format!("DROP TRIGGER aidash_activation ON {table}"))
				.await?;
		}
		m.get_connection().execute_unprepared("DROP FUNCTION aidash_activation_trigger(); DROP FUNCTION aidash_request_activation(UUID, TEXT)").await?;
		for table in ["activation_quarantine", "run_activations"] {
			m.drop_table(Table::drop().table(a(table)).to_owned())
				.await?;
		}
		Ok(())
	}
}
