use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		// SeaQuery cannot define a PostgreSQL trigger function. Install the
		// bridge before the catch-up backfill so old replicas cannot write into
		// the gap between the one-time backfill and trigger activation.
		manager.get_connection().execute_unprepared(r#"
CREATE FUNCTION legacy_run_input_bridge() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    uuid_pattern CONSTANT text := '[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}';
    target_run uuid;
    current_run record;
    existing_content text;
BEGIN
    IF NEW.idempotency_key ~ ('^human:' || uuid_pattern || ':' || uuid_pattern || '$') THEN
        target_run := split_part(NEW.idempotency_key, ':', 2)::uuid;
    ELSIF NEW.idempotency_key LIKE 'subject-human:%'
        AND right(NEW.idempotency_key, 74) ~ ('^:' || uuid_pattern || ':' || uuid_pattern || '$') THEN
        target_run := split_part(right(NEW.idempotency_key, 74), ':', 2)::uuid;
    ELSE
        RETURN NEW;
    END IF;

    SELECT r.phase, r.control, r.pending INTO current_run
    FROM runs AS r WHERE r.id = target_run AND r.workspace_id = NEW.workspace_id FOR UPDATE;
    IF NOT FOUND THEN
        RETURN NEW;
    END IF;

    SELECT content INTO existing_content FROM run_inputs
    WHERE run_id = target_run AND idempotency_key = NEW.idempotency_key;
    IF FOUND THEN
        IF existing_content <> NEW.content THEN
            RAISE EXCEPTION 'run message idempotency key reused';
        END IF;
        UPDATE run_inputs SET message_id = NEW.id
        WHERE run_id = target_run AND idempotency_key = NEW.idempotency_key
            AND (message_id IS NULL OR message_id = NEW.id);
        IF NOT FOUND THEN
            RAISE EXCEPTION 'run input message binding changed';
        END IF;
        RETURN NEW;
    END IF;

    IF current_run.pending->>'finalizing' = 'true'
        OR current_run.pending ? 'terminal_transition'
        OR current_run.control = 'CANCELLED'
        OR current_run.phase IN ('COMPLETED', 'FAILED', 'CANCELLED') THEN
        RAISE EXCEPTION 'run message arrived after finalization';
    END IF;
    INSERT INTO run_inputs (run_id, sender, content, idempotency_key, message_id)
    VALUES (target_run, NEW.sender, NEW.content, NEW.idempotency_key, NEW.id);
    RETURN NEW;
END;
$$;
CREATE TRIGGER legacy_run_input_bridge AFTER INSERT ON messages
FOR EACH ROW EXECUTE FUNCTION legacy_run_input_bridge();
"#).await?;

		let historical = Query::select()
			.columns([
				(Alias::new("r"), Alias::new("id")),
				(Alias::new("m"), Alias::new("sender")),
				(Alias::new("m"), Alias::new("content")),
				(Alias::new("m"), Alias::new("idempotency_key")),
				(Alias::new("m"), Alias::new("id")),
			])
			.from_as(Alias::new("messages"), Alias::new("m"))
			.join_as(
				JoinType::InnerJoin,
				Alias::new("runs"),
				Alias::new("r"),
				Expr::cust("m.workspace_id = r.workspace_id"),
			)
			.and_where(Expr::cust("m.idempotency_key IS NOT NULL"))
			.and_where(Expr::cust("(m.idempotency_key LIKE 'human:' || r.id::text || ':%' OR (m.idempotency_key LIKE 'subject-human:%' AND RIGHT(m.idempotency_key, 74) LIKE ':' || r.id::text || ':%'))"))
			.order_by((Alias::new("m"), Alias::new("created_at")), Order::Asc)
			.order_by((Alias::new("m"), Alias::new("id")), Order::Asc)
			.to_owned();
		let mut backfill = Query::insert();
		backfill.into_table(Alias::new("run_inputs")).columns([
			Alias::new("run_id"),
			Alias::new("sender"),
			Alias::new("content"),
			Alias::new("idempotency_key"),
			Alias::new("message_id"),
		]);
		backfill
			.select_from(historical)
			.map_err(|error| DbErr::Custom(error.to_string()))?
			.on_conflict(
				OnConflict::columns([Alias::new("run_id"), Alias::new("idempotency_key")])
					.do_nothing()
					.to_owned(),
			);
		manager
			.get_connection()
			.execute(manager.get_database_backend().build(&backfill))
			.await?;
		Ok(())
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.get_connection()
			.execute_unprepared(
				"DROP TRIGGER legacy_run_input_bridge ON messages; DROP FUNCTION legacy_run_input_bridge()",
			)
			.await?;
		Ok(())
	}
}
