-- Reinhardt cannot describe NOT VALID. Keep earlier local-validation rows
-- readable for rollback while excluding them from the current public views.
ALTER TABLE public.model_calls DROP CONSTRAINT model_calls_purpose;
ALTER TABLE public.model_calls ADD CONSTRAINT model_calls_purpose
    CHECK (purpose IN ('compaction_summary','memory_role','workbench_test','mcp_sampling')) NOT VALID;
