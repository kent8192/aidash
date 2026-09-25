use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		// SeaQuery cannot define a PostgreSQL trigger function. Old replicas
		// write the message first and cannot apply the model-aware input budget;
		// reject those writes until every serving replica uses ledger admission.
		// Unkeyed messages cannot identify a particular run, so fail closed while
		// a workspace has active runs rather than reporting a false acceptance.
		manager.get_connection().execute_unprepared(r#"
CREATE FUNCTION gate_legacy_run_message() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    uuid_pattern CONSTANT text := '[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}';
    target_run uuid;
BEGIN
    IF NEW.idempotency_key IS NULL THEN
        IF EXISTS (SELECT 1 FROM runs WHERE workspace_id = NEW.workspace_id
            AND phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')) THEN
            RAISE EXCEPTION 'unkeyed messages require upgraded run admission while a run is active';
        END IF;
        RETURN NEW;
    END IF;

    IF NEW.idempotency_key ~ ('^human:' || uuid_pattern || ':' || uuid_pattern || '$') THEN
        target_run := split_part(NEW.idempotency_key, ':', 2)::uuid;
    ELSIF NEW.idempotency_key LIKE 'subject-human:%'
        AND right(NEW.idempotency_key, 74) ~ ('^:' || uuid_pattern || ':' || uuid_pattern || '$') THEN
        target_run := split_part(right(NEW.idempotency_key, 74), ':', 2)::uuid;
    ELSE
        RETURN NEW;
    END IF;

    IF EXISTS (SELECT 1 FROM runs WHERE id = target_run AND workspace_id = NEW.workspace_id)
        AND NOT EXISTS (SELECT 1 FROM run_inputs
            WHERE run_id = target_run AND idempotency_key = NEW.idempotency_key) THEN
        RAISE EXCEPTION 'run-directed messages require upgraded ledger admission';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER gate_legacy_run_message BEFORE INSERT ON messages
FOR EACH ROW EXECUTE FUNCTION gate_legacy_run_message();
"#).await?;
		Ok(())
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.get_connection()
			.execute_unprepared(
				"DROP TRIGGER gate_legacy_run_message ON messages; DROP FUNCTION gate_legacy_run_message()",
			)
			.await?;
		Ok(())
	}
}
