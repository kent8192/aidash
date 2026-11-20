CREATE OR REPLACE FUNCTION public.aidash_stint_transition() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    taken_over boolean := false;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        -- Every claim takes a new token and a holder releases by clearing it,
        -- so a token replacing another is a takeover: a Lease Recovery.
        taken_over := OLD.lease_owner IS NOT NULL AND NEW.lease_owner IS NOT NULL
            AND OLD.lease_owner <> NEW.lease_owner;
    END IF;
    IF NEW.control = 'PAUSED' THEN
        -- A person, a Budget set to stop or a Safety Stop paused it: the Agent
        -- cannot move it forward itself.
        UPDATE public.stints
        SET ended_at = greatest(clock_timestamp(), started_at), end_reason = 'paused'
        WHERE run_id = NEW.id AND ended_at IS NULL;
        RETURN NULL;
    END IF;
    -- A cancelled execution is finished without the Agent, so only an active
    -- one opens a Stint; one already open ends with the terminal phase.
    IF NEW.control = 'ACTIVE' AND NEW.phase IN ('READY', 'THINKING', 'TOOL_CALL') THEN
        INSERT INTO public.stints (id, tenant, run_id, agent_id, agent_version, node, started_at, lease_owner)
        SELECT gen_random_uuid(), NEW.tenant, NEW.id, NEW.agent_id, NEW.agent_version, NEW.node, clock_timestamp(), NEW.lease_owner
        WHERE NOT EXISTS (SELECT 1 FROM public.stints WHERE run_id = NEW.id AND ended_at IS NULL);
    END IF;
    -- The last holder stays recorded after it releases the lease.
    IF NEW.lease_owner IS NOT NULL THEN
        UPDATE public.stints
        SET lease_owner = NEW.lease_owner,
            lease_recoveries = lease_recoveries + CASE WHEN taken_over THEN 1 ELSE 0 END
        WHERE run_id = NEW.id AND ended_at IS NULL
            AND (taken_over OR lease_owner IS DISTINCT FROM NEW.lease_owner);
    END IF;
    IF NEW.phase IN ('COMPLETED', 'FAILED', 'CANCELLED')
        OR (NEW.phase = 'WAITING' AND COALESCE(NEW.pending #>> '{data,reason}', '') <> 'failure_delivery') THEN
        UPDATE public.stints
        SET ended_at = greatest(clock_timestamp(), started_at),
            end_reason = CASE WHEN NEW.phase = 'WAITING'
                THEN COALESCE(NEW.pending #>> '{data,reason}', 'waiting')
                ELSE lower(NEW.phase) END
        WHERE run_id = NEW.id AND ended_at IS NULL;
    END IF;
    RETURN NULL;
END;
$$;
DROP TRIGGER aidash_stint_transition ON public.task_executions;
CREATE TRIGGER aidash_stint_transition AFTER INSERT OR UPDATE OF phase, pending, control, lease_owner ON public.task_executions FOR EACH ROW EXECUTE FUNCTION public.aidash_stint_transition();
