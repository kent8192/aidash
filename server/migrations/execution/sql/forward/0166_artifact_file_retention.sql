CREATE FUNCTION public.aidash_release_artifact_retention() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM public.core_retention_holds
     WHERE holder_kind = 'artifact'
       AND holder_id = OLD.id::text
       AND tenant = OLD.tenant;
    RETURN OLD;
END;
$$;
CREATE TRIGGER artifacts_release_retention AFTER DELETE ON public.artifacts
    FOR EACH ROW EXECUTE FUNCTION public.aidash_release_artifact_retention();
