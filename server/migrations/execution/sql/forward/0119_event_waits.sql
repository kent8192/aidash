SET LOCAL search_path = public, pg_catalog;

CREATE FUNCTION public.aidash_operation_finished_activation() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.state IN ('prepared', 'submitted', 'running', 'cancelling')
       AND NEW.state NOT IN ('prepared', 'submitted', 'running', 'cancelling') THEN
        PERFORM aidash_request_activation(NEW.run_id, 'operation_finished');
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER aidash_operation_finished_activation
AFTER UPDATE OF state ON public.core_operations
FOR EACH ROW EXECUTE FUNCTION public.aidash_operation_finished_activation();

CREATE FUNCTION public.aidash_workspace_task_activation() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE target uuid;
BEGIN
    IF NEW.status IS NOT DISTINCT FROM OLD.status
       OR NEW.status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED', 'ABANDONED', 'BLOCKED') THEN
        RETURN NEW;
    END IF;
    FOR target IN
	    SELECT o.source_run_id FROM authorization_task_origins o
	      JOIN runs r ON r.id = o.source_run_id AND r.tenant = o.tenant
	     WHERE o.task_id = NEW.id AND o.tenant = NEW.tenant
	       AND r.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
        UNION
	    SELECT o.run_id FROM authorization_run_outputs o
	      JOIN runs r ON r.id = o.run_id AND r.tenant = o.tenant
	     WHERE o.resource_kind = 'delegated_task' AND o.resource_id = NEW.id
	       AND o.tenant = NEW.tenant AND r.phase NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
        ORDER BY 1
    LOOP
        -- A dedicated Run-keyed advisory lock orders producer commits without
        -- taking the Run row or reversing cancellation's row-lock order.
        PERFORM pg_advisory_xact_lock(hashtextextended('aidash:workspace-wait:' || target::text, 71500644));
        PERFORM aidash_request_activation(target, 'workspace_task:' || NEW.id::text || ':' || NEW.status);
    END LOOP;
    RETURN NEW;
END;
$$;

CREATE TRIGGER aidash_workspace_task_activation
AFTER UPDATE OF status ON public.tasks
FOR EACH ROW EXECUTE FUNCTION public.aidash_workspace_task_activation();

CREATE FUNCTION public.aidash_delegated_task_activation() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE task_status text;
BEGIN
    IF NEW.resource_kind <> 'delegated_task' THEN
        RETURN NEW;
    END IF;
    SELECT status INTO task_status FROM tasks
     WHERE id = NEW.resource_id AND tenant = NEW.tenant;
    IF task_status IN ('COMPLETED', 'FAILED', 'CANCELLED', 'ABANDONED', 'BLOCKED') THEN
        PERFORM pg_advisory_xact_lock(hashtextextended('aidash:workspace-wait:' || NEW.run_id::text, 71500644));
        PERFORM aidash_request_activation(NEW.run_id, 'workspace_task:' || NEW.resource_id::text || ':' || task_status);
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER aidash_delegated_task_activation
AFTER INSERT ON public.authorization_run_outputs
FOR EACH ROW EXECUTE FUNCTION public.aidash_delegated_task_activation();
