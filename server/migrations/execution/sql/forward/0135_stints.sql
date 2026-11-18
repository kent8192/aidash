CREATE FUNCTION public.aidash_stint_transition() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.phase IN ('READY', 'THINKING', 'TOOL_CALL') THEN
        INSERT INTO public.stints (id, tenant, run_id, agent_id, agent_version, node, started_at)
        SELECT gen_random_uuid(), NEW.tenant, NEW.id, NEW.agent_id, NEW.agent_version, NEW.node, clock_timestamp()
        WHERE NOT EXISTS (SELECT 1 FROM public.stints WHERE run_id = NEW.id AND ended_at IS NULL);
    ELSIF NEW.phase IN ('COMPLETED', 'FAILED', 'CANCELLED')
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
CREATE TRIGGER aidash_stint_transition AFTER INSERT OR UPDATE OF phase, pending ON public.runs FOR EACH ROW EXECUTE FUNCTION public.aidash_stint_transition();
CREATE FUNCTION public.aidash_inference_attempt_stint() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.stint_id IS NULL THEN
        NEW.stint_id := (SELECT id FROM public.stints WHERE run_id = NEW.run_id AND ended_at IS NULL);
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER aidash_inference_attempt_stint BEFORE INSERT ON public.inference_attempts FOR EACH ROW EXECUTE FUNCTION public.aidash_inference_attempt_stint();
