// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0001_functions", "federation")
        .add_dependency("execution", "0001_functions")
        .add_operation(Operation::RunSQL {
            sql: r#"SET LOCAL check_function_bodies = false;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.atomic_immutable_decision() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.id<>OLD.id OR NEW.digest<>OLD.digest OR NEW.manifest<>OLD.manifest
       OR (OLD.decision IS NOT NULL AND NEW.decision IS DISTINCT FROM OLD.decision)
       OR (OLD.visible AND NOT NEW.visible) OR (OLD.complete AND NOT NEW.complete) THEN
        RAISE EXCEPTION 'atomic transaction identity and decisions are immutable';
    END IF;
    RETURN NEW;
END;
$$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.atomic_write_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE pending uuid;
BEGIN
    IF current_setting('aidash.transaction_control',true) = 'authority'
       AND TG_TABLE_NAME IN ('authorization_bundles','authorization_revisions',
           'authorization_credentials','authorization_decisions',
           'authorization_peer_mappings','authorization_peer_mapping_history',
           'authorization_graph_operator_grants','peers') THEN
        RETURN NULL;
    END IF;
    SELECT transaction_id INTO pending FROM atomic_gate WHERE singleton FOR SHARE NOWAIT;
    IF pending IS NOT NULL AND current_setting('aidash.atomic_transaction',true) IS DISTINCT FROM pending::text THEN
        RAISE EXCEPTION 'atomic transaction visibility pending' USING ERRCODE='55P03';
    END IF;
    RETURN NULL;
END;
$$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.gate_remote_task_terminal() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
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
$$;"#.to_string(),
            // Refuse irreversible baseline rollback before the native ledger changes.
            reverse_sql: Some(r#"DO $aidash_baseline$
BEGIN
    RAISE EXCEPTION 'Aidash frozen baseline is forward-only; restore a backup to roll back';
END
$aidash_baseline$;"#.to_string()),
        })
        .atomic(true)
        .database_only(true)
}
