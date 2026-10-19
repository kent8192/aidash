DO $$
DECLARE
    definition text := pg_get_functiondef('public.aidash_activation_trigger()'::regprocedure);
BEGIN
    IF position('IF NEW.kind IN (''outbound'', ''disclosure'', ''tool_call'') THEN' IN definition) > 0 THEN
        RETURN;
    END IF;
    IF position('IF NEW.kind = ''approval'' THEN' IN definition) = 0 THEN
        RAISE EXCEPTION 'unknown approval activation trigger definition';
    END IF;
    EXECUTE replace(definition, 'IF NEW.kind = ''approval'' THEN', 'IF NEW.kind IN (''outbound'', ''disclosure'', ''tool_call'') THEN');
END;
$$;
