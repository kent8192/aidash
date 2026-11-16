ALTER TABLE public.registry ADD CONSTRAINT registry_no_compaction_decider
    CHECK (kind <> 'decider' OR metadata #>> '{config,hook}' IS DISTINCT FROM 'compaction') NOT VALID;
