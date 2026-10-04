// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// SQL preserves generated columns, composite keys, CHECKs and procedural guards.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
    Migration::new("0001_functions", "workspaces")
        .add_dependency("registry", "0001_functions")
        .add_operation(Operation::RunSQL {
            sql: r#"SET LOCAL check_function_bodies = false;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.guard_task_dependencies() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF pg_trigger_depth() < 2 THEN
        RAISE EXCEPTION 'task_dependencies is maintained by tasks'
            USING ERRCODE = '23514', CONSTRAINT = 'tasks_dependencies_managed';
    END IF;
    RETURN NULL;
END $$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.guard_task_dependency_cycle() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF EXISTS (
        WITH RECURSIVE edges(source_id, target_id) AS (
            SELECT parent_id, id FROM tasks WHERE parent_id IS NOT NULL
            UNION ALL
            SELECT task_id, dependency_id FROM task_dependencies
        ), reachable(current_id) AS (
            SELECT target_id FROM edges WHERE source_id = NEW.id
            UNION
            SELECT edge.target_id
            FROM reachable r
            JOIN edges edge ON edge.source_id = r.current_id
        )
        SELECT 1 FROM reachable WHERE current_id = NEW.id
    ) THEN
        RAISE EXCEPTION 'task dependency graph contains a cycle'
            USING ERRCODE = '23514', CONSTRAINT = 'tasks_dependency_cycle';
    END IF;
    RETURN NEW;
END $$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.guard_task_parent_cycle() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    -- The statement trigger takes the shared lock before deferred-check
    -- snapshots are established; keep this acquisition as a defensive guard.
    -- Scope serialization to this schema's task hierarchy. Isolated tenant/test
    -- schemas share a database but must not block one another's graph writes.
    PERFORM pg_advisory_xact_lock(70721021, hashtext(TG_TABLE_SCHEMA));
    IF EXISTS (
        WITH RECURSIVE walk(current_id, parent_id, path, cycle) AS (
            SELECT id, parent_id, ARRAY[id], false
            FROM tasks
            WHERE id = NEW.id
            UNION ALL
            SELECT t.id, t.parent_id, w.path || t.id, t.id = ANY(w.path)
            FROM walk w
            JOIN tasks t ON t.id = w.parent_id
            WHERE NOT w.cycle
        )
        SELECT 1 FROM walk WHERE cycle
    ) THEN
        RAISE EXCEPTION 'task parent hierarchy contains a cycle'
            USING ERRCODE = '23514', CONSTRAINT = 'tasks_parent_cycle';
    END IF;
    RETURN NEW;
END $$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.lock_task_hierarchy_before_change() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(70721021, hashtext(TG_TABLE_SCHEMA));
    RETURN NULL;
END $$;"#.to_string(),
            reverse_sql: None,
        })
        .add_operation(Operation::RunSQL {
            sql: r#"CREATE FUNCTION public.sync_task_dependencies() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'UPDATE' THEN
        DELETE FROM task_dependencies WHERE task_id = OLD.id;
    END IF;
    INSERT INTO task_dependencies(task_id, workspace_id, dependency_id)
        SELECT DISTINCT NEW.id, NEW.workspace_id, unnest(NEW.dependencies);
    RETURN NEW;
END $$;"#.to_string(),
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
