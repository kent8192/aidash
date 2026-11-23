-- Reinhardt has no policy/trigger operations. Its PostgreSQL AlterColumn
-- renderer also does not emit changes to nullability.
ALTER TABLE public.inference_usage ALTER COLUMN run_id DROP NOT NULL;

CREATE FUNCTION public.aidash_inference_owner() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'INSERT' AND NEW.stint_id IS NULL AND NEW.model_call_id IS NULL THEN
        NEW.stint_id := (SELECT id FROM public.stints
            WHERE tenant = NEW.tenant AND run_id = NEW.run_id AND ended_at IS NULL);
    END IF;
    IF (NEW.stint_id IS NULL) = (NEW.model_call_id IS NULL) THEN
        RAISE EXCEPTION 'an inference attempt requires exactly one owner'
            USING ERRCODE = '23514';
    END IF;
    IF NEW.model_call_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM public.model_calls
        WHERE tenant = NEW.tenant AND id = NEW.model_call_id
            AND run_id IS NOT DISTINCT FROM NEW.run_id
    ) THEN
        RAISE EXCEPTION 'inference attempt and model call have different execution attribution'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
DROP TRIGGER aidash_inference_attempt_stint ON public.inference_attempts;
CREATE TRIGGER aidash_inference_attempt_owner
    BEFORE INSERT OR UPDATE OF stint_id, model_call_id, run_id ON public.inference_attempts
    FOR EACH ROW EXECUTE FUNCTION public.aidash_inference_owner();
CREATE TRIGGER aidash_inference_usage_owner
    BEFORE INSERT OR UPDATE OF stint_id, model_call_id, run_id ON public.inference_usage
    FOR EACH ROW EXECUTE FUNCTION public.aidash_inference_owner();
