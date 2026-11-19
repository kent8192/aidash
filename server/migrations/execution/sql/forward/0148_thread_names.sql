-- PostgreSQL renames constraints and indexes only with ALTER ... RENAME, which
-- typed Reinhardt operations cannot express. Names follow the Thread (#1008).
ALTER TABLE public.runs RENAME CONSTRAINT runs_tenant_id_workspace_id_key TO runs_tenant_id_thread_id_key;
ALTER TABLE public.events RENAME CONSTRAINT events_workspace_id_fkey TO events_thread_id_fkey;
ALTER TABLE public.generation_embedding_usage RENAME CONSTRAINT generation_embedding_usage_workspace_id_fkey TO generation_embedding_usage_thread_id_fkey;
ALTER TABLE public.generation_requests RENAME CONSTRAINT generation_requests_local_workspace_id_fkey TO generation_requests_local_thread_id_fkey;
ALTER TABLE public.generation_requests RENAME CONSTRAINT generation_requests_task_workspace TO generation_requests_task_thread;
ALTER TABLE public.human_requests RENAME CONSTRAINT human_requests_run_workspace TO human_requests_run_thread;
ALTER TABLE public.subnodes RENAME CONSTRAINT subnodes_workspace_fkey TO subnodes_thread_fkey;
ALTER TABLE public.subnodes RENAME CONSTRAINT subnodes_workspace_key TO subnodes_thread_key;
ALTER INDEX public.runs_id_workspace_unique RENAME TO runs_id_thread_unique;
ALTER INDEX public.events_workspace RENAME TO events_thread;
