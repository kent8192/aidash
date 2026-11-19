-- PostgreSQL function bodies cannot be expressed by typed Reinhardt operations.
-- The Task dependency sync writes workspace_id again (#1008).
CREATE OR REPLACE FUNCTION public.sync_task_dependencies()
 RETURNS trigger
 LANGUAGE plpgsql
AS $function$
BEGIN
    IF TG_OP = 'UPDATE' THEN
        DELETE FROM task_dependencies WHERE task_id = OLD.id;
    END IF;
    INSERT INTO task_dependencies(task_id, tenant, workspace_id, dependency_id)
        SELECT DISTINCT NEW.id, NEW.tenant, NEW.workspace_id, unnest(NEW.dependencies);
    RETURN NEW;
END $function$;
