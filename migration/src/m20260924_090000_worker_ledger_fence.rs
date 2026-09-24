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
            OR NEW.observed_input_seq IS DISTINCT FROM OLD.observed_input_seq
            OR NEW.revision IS DISTINCT FROM OLD.revision) THEN
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

-- Legacy workers publish model output through messages before saving the run.
-- Serialize that effect with run-input admission, then reject output if an
-- admitted correction is already present. Fenced upgraded publication sets
-- the transaction-local marker only after checking the worker lease and input
-- sequence.
CREATE FUNCTION gate_legacy_run_output() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    uuid_pattern CONSTANT text := '[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}';
    target_run uuid;
    target_task uuid;
    peer_key_parts text[];
BEGIN
    IF NEW.idempotency_key ~ ('^' || uuid_pattern || ':[0-9]+:output$') THEN
        target_run := split_part(NEW.idempotency_key, ':', 1)::uuid;
    ELSIF NEW.idempotency_key ~ ('^.*:' || uuid_pattern || ':' || uuid_pattern || ':[0-9]+:output$') THEN
        peer_key_parts := regexp_match(
            NEW.idempotency_key,
            '^.*:(' || uuid_pattern || '):(' || uuid_pattern || '):[0-9]+:output$'
        );
        target_task := peer_key_parts[1]::uuid;
        target_run := peer_key_parts[2]::uuid;
    END IF;

    IF target_run IS NOT NULL THEN
        PERFORM id FROM runs
        WHERE id = target_run
          AND workspace_id = NEW.workspace_id
          AND (target_task IS NULL OR task_id = target_task)
        FOR UPDATE;
        IF FOUND
            AND current_setting('aidash.input_ledger_worker', true) IS DISTINCT FROM 'true'
            AND EXISTS (SELECT 1 FROM run_inputs WHERE run_id = target_run) THEN
            RAISE EXCEPTION 'run input ledger requires fenced response publication';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER gate_legacy_run_output BEFORE INSERT ON messages
FOR EACH ROW EXECUTE FUNCTION gate_legacy_run_output();
"#,
			)
			.await?;
		Ok(())
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.get_connection()
			.execute_unprepared(
				"DROP TRIGGER gate_legacy_run_output ON messages; DROP FUNCTION gate_legacy_run_output(); DROP TRIGGER gate_legacy_run_worker ON runs; DROP FUNCTION gate_legacy_run_worker()",
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
