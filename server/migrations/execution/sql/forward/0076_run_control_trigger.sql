CREATE FUNCTION public.aidash_run_control_provenance() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.control IS DISTINCT FROM OLD.control
        AND NEW.control_revision = OLD.control_revision THEN
        NEW.control_revision := OLD.control_revision + 1;
        IF NEW.control_source = OLD.control_source THEN
            NEW.control_source := 'system';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER aidash_run_control_provenance BEFORE UPDATE OF control ON public.runs FOR EACH ROW EXECUTE FUNCTION public.aidash_run_control_provenance();
