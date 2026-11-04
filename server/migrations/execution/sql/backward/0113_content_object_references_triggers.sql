-- The prior UUID objects and their owning rows were deliberately discarded.
-- Refuse reversal instead of constructing an incomplete legacy schema.
DO $$ BEGIN
  RAISE EXCEPTION 'execution 0113_content_object_references is irreversible; restore a pre-cutover backup'
    USING ERRCODE = '55000';
END $$;
DROP TRIGGER atomic_write_guard ON public.core_quotas;
DROP TRIGGER atomic_write_guard ON public.core_objects;
