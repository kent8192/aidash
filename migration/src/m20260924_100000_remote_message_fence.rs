use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_table(
				Table::create()
					.table(Alias::new("remote_run_message_fences"))
					.col(ColumnDef::new(Alias::new("task_id")).uuid().not_null())
					.col(ColumnDef::new(Alias::new("run_id")).uuid().not_null())
					.col(
						ColumnDef::new(Alias::new("idempotency_key"))
							.text()
							.not_null(),
					)
					.col(ColumnDef::new(Alias::new("content")).text().not_null())
					.col(ColumnDef::new(Alias::new("input_seq")).big_integer().null())
					.col(
						ColumnDef::new(Alias::new("expires_at"))
							.timestamp_with_time_zone()
							.null(),
					)
					.col(
						ColumnDef::new(Alias::new("consumed"))
							.boolean()
							.not_null()
							.default(false),
					)
					.primary_key(
						Index::create()
							.col(Alias::new("task_id"))
							.col(Alias::new("idempotency_key")),
					)
					.foreign_key(
						ForeignKey::create()
							.from_tbl(Alias::new("remote_run_message_fences"))
							.from_col(Alias::new("task_id"))
							.to_tbl(Alias::new("tasks"))
							.to_col(Alias::new("id"))
							.on_delete(ForeignKeyAction::Cascade),
					)
					.to_owned(),
			)
			.await?;
		// SeaQuery cannot define PostgreSQL trigger functions. The task-row
		// lock in reservation serializes remote admission with terminal updates.
		// Matching run rows are locked too, so a legacy worker cannot complete a
		// task while an unobserved ledger input is admitted or backfilled.
		manager
			.get_connection()
			.execute_unprepared(
				r#"
CREATE FUNCTION gate_remote_task_terminal() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.status IN ('COMPLETED', 'FAILED', 'CANCELLED', 'ABANDONED')
        AND NEW.status IS DISTINCT FROM OLD.status THEN
        PERFORM id FROM runs WHERE task_id = OLD.id FOR UPDATE;
        IF EXISTS (
                SELECT 1 FROM remote_run_message_fences
                WHERE task_id = OLD.id
                  AND NOT consumed
                  AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP)
            ) OR EXISTS (
                SELECT 1 FROM runs AS r
                WHERE r.task_id = OLD.id
                  AND r.lease_owner IS NOT NULL
                  AND NOT r.ledger_worker_ready
                  AND EXISTS (
                      SELECT 1 FROM run_inputs AS i
                      WHERE i.run_id = r.id AND i.seq > r.observed_input_seq
                  )
            ) THEN
            RAISE EXCEPTION USING ERRCODE = 'A3301',
                MESSAGE = 'run messages await inference';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER gate_remote_task_terminal BEFORE UPDATE OF status ON tasks
FOR EACH ROW EXECUTE FUNCTION gate_remote_task_terminal();
CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON remote_run_message_fences FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();
"#,
			)
			.await?;
		// CREATE TRIGGER above holds the task-table DDL lock until this migration
		// commits. Backfill while that lock is held so legacy task termination
		// cannot slip between the history snapshot and activation of the gate.
		let prefix = "(SUBSTRING(m.sender FROM 7) || ':' || t.id::text || ':')";
		let input_key = format!("SUBSTRING(m.idempotency_key FROM LENGTH({prefix}) + 1)");
		let uuid = "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}";
		let valid_key = format!("{input_key} ~ '^(human:|subject-human:.+:){uuid}:{uuid}$'");
		let history = Query::select()
			.expr(Expr::cust("t.id"))
			.expr(Expr::cust(format!(
				"CASE WHEN {valid_key} THEN LEFT(RIGHT({input_key}, 73), 36)::uuid END"
			)))
			.expr(Expr::cust(&input_key))
			.expr(Expr::cust("m.content"))
			.expr(Expr::cust("NULL"))
			.expr(Expr::value(false))
			.from_as(Alias::new("messages"), Alias::new("m"))
			.join_as(
				JoinType::InnerJoin,
				Alias::new("tasks"),
				Alias::new("t"),
				Expr::cust("m.workspace_id = t.workspace_id"),
			)
			.and_where(Expr::cust(
				"t.status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED', 'ABANDONED')",
			))
			.and_where(Expr::cust("LEFT(m.sender, 6) = 'human@'"))
			.and_where(Expr::cust(format!(
				"LEFT(m.idempotency_key, LENGTH({prefix})) = {prefix}"
			)))
			.and_where(Expr::cust(valid_key))
			.to_owned();
		let backfill = Query::insert()
			.into_table(Alias::new("remote_run_message_fences"))
			.columns(
				[
					"task_id",
					"run_id",
					"idempotency_key",
					"content",
					"expires_at",
					"consumed",
				]
				.map(Alias::new),
			)
			.select_from(history)
			.map_err(|error| DbErr::Custom(error.to_string()))?
			.to_owned();
		let db = manager.get_connection();
		db.execute(db.get_database_backend().build(&backfill))
			.await?;
		manager
			.create_index(
				Index::create()
					.name("remote_run_message_fences_sequence")
					.table(Alias::new("remote_run_message_fences"))
					.col(Alias::new("task_id"))
					.col(Alias::new("run_id"))
					.col(Alias::new("input_seq"))
					.to_owned(),
			)
			.await?;

		Ok(())
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.get_connection()
			.execute_unprepared(
				"DROP TRIGGER gate_remote_task_terminal ON tasks; DROP FUNCTION gate_remote_task_terminal()",
			)
			.await?;
		manager
			.drop_table(
				Table::drop()
					.table(Alias::new("remote_run_message_fences"))
					.to_owned(),
			)
			.await?;
		Ok(())
	}
}
