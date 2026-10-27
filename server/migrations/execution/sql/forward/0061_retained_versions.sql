CREATE OR REPLACE FUNCTION public.aidash_release_message_retention() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM public.core_retention_holds
     WHERE holder_kind = 'message'
       AND holder_id = OLD.id::text
       AND tenant = OLD.tenant;
    RETURN OLD;
END;
$$;
DROP TRIGGER IF EXISTS messages_release_retention ON public.messages;
CREATE TRIGGER messages_release_retention AFTER DELETE ON public.messages
    FOR EACH ROW EXECUTE FUNCTION public.aidash_release_message_retention();
