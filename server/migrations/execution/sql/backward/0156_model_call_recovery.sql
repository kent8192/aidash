ALTER TABLE public.model_calls DROP CONSTRAINT model_calls_purpose;
ALTER TABLE public.model_calls ADD CONSTRAINT model_calls_purpose
    CHECK (purpose IN ('compaction_summary','memory_role','workbench_test','media_check','mcp_sampling'));
