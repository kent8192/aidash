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
						ColumnDef::new(Alias::new("ledger_worker_ready"))
							.boolean()
							.not_null()
							.default(false),
					)
					.to_owned(),
			)
			.await?;
		// SeaQuery cannot define a PostgreSQL trigger function. An old worker
		// does not set this transaction-local marker when it acquires or renews a
		// lease. Fence it once ledger input exists or an upgraded worker has
		// claimed the run, including across a subsequent lease expiry.
		manager
			.get_connection()
			.execute_unprepared(
				r#"
CREATE FUNCTION gate_legacy_run_worker() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    -- A worker that already held a lease when this migration ran must not
    -- commit an answer against inputs that were accepted beforehand.
    IF OLD.lease_owner IS NOT NULL AND NOT OLD.ledger_worker_ready
        AND EXISTS (SELECT 1 FROM run_inputs WHERE run_id = OLD.id)
        AND current_setting('aidash.input_ledger_worker', true) IS DISTINCT FROM 'true'
        AND (NEW.phase IS DISTINCT FROM OLD.phase
            OR NEW.pending IS DISTINCT FROM OLD.pending
            OR NEW.context IS DISTINCT FROM OLD.context
            OR NEW.step IS DISTINCT FROM OLD.step
            OR NEW.observed_input_seq IS DISTINCT FROM OLD.observed_input_seq) THEN
        RAISE EXCEPTION 'run input ledger requires an upgraded worker';
    END IF;
    IF NEW.lease_owner IS NOT NULL
        AND (NEW.lease_owner IS DISTINCT FROM OLD.lease_owner
            OR NEW.lease_until > OLD.lease_until)
        AND (OLD.ledger_worker_ready OR EXISTS
            (SELECT 1 FROM run_inputs WHERE run_id = OLD.id))
        AND current_setting('aidash.input_ledger_worker', true) IS DISTINCT FROM 'true' THEN
        RAISE EXCEPTION 'run input ledger requires an upgraded worker';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER gate_legacy_run_worker BEFORE UPDATE ON runs
FOR EACH ROW EXECUTE FUNCTION gate_legacy_run_worker();
"#,
			)
			.await?;
		Ok(())
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.get_connection()
			.execute_unprepared(
				"DROP TRIGGER gate_legacy_run_worker ON runs; DROP FUNCTION gate_legacy_run_worker()",
			)
			.await?;
		manager
			.alter_table(
				Table::alter()
					.table(Alias::new("runs"))
					.drop_column(Alias::new("ledger_worker_ready"))
					.to_owned(),
			)
			.await?;
		Ok(())
	}
}
