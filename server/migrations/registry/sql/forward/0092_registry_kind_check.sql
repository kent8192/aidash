ALTER TABLE public.registry ADD CONSTRAINT registry_active_kind
 CHECK (kind IN ('agent', 'bundle', 'embedding', 'external_service', 'model', 'skill', 'source', 'tool')) NOT VALID;
