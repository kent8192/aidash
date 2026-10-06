SET LOCAL search_path = public, pg_catalog;

DO $fresh$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
        WHERE n.nspname=current_schema() AND c.relkind IN ('r','p','v','m','f')
          AND c.relname <> 'reinhardt_migrations') THEN
        RAISE EXCEPTION 'Aidash workspace baseline requires an empty database; legacy or unrelated schemas are not adopted';
    END IF;
END
$fresh$;
