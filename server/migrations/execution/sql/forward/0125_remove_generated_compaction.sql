-- Statement triggers and row-level security have no typed migration operation.
DROP TRIGGER atomic_write_guard ON public.generation_compaction_usage;
DROP POLICY tenant_isolation ON public.generation_compaction_usage;
