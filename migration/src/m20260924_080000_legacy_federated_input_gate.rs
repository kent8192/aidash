use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		// SeaQuery cannot define a PostgreSQL trigger function. A pre-upgrade
		// executor writes corrections directly to the home node, without first
		// recording them in its own run-input ledger. Refuse that legacy path so
		// it cannot return success while a new executor finalizes stale output.
		manager.get_connection().execute_unprepared(r#"
CREATE FUNCTION gate_legacy_federated_run_message() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    uuid_pattern CONSTANT text := '[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}';
BEGIN
    IF NEW.idempotency_key ~ ('^.+:' || uuid_pattern || ':human:' || uuid_pattern || ':' || uuid_pattern || '$')
        OR NEW.idempotency_key ~ ('^.+:' || uuid_pattern || ':subject-human:.+:' || uuid_pattern || ':' || uuid_pattern || '$') THEN
        IF current_setting('aidash.run_message_delivery', true) IS DISTINCT FROM 'true' THEN
            IF NOT EXISTS (SELECT 1 FROM messages AS existing
                WHERE existing.idempotency_key = NEW.idempotency_key
                    AND existing.workspace_id = NEW.workspace_id
                    AND existing.sender = NEW.sender
                    AND existing.content = NEW.content) THEN
                RAISE EXCEPTION 'federated run messages require upgraded ledger delivery';
            END IF;
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER gate_legacy_federated_run_message BEFORE INSERT ON messages
FOR EACH ROW EXECUTE FUNCTION gate_legacy_federated_run_message();
"#).await?;
		Ok(())
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.get_connection()
			.execute_unprepared("DROP TRIGGER gate_legacy_federated_run_message ON messages; DROP FUNCTION gate_legacy_federated_run_message()")
			.await?;
		Ok(())
	}
}
