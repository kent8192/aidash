-- PostgreSQL renames constraints and indexes only with ALTER ... RENAME, which
-- typed Reinhardt operations cannot express. Names follow the Thread (#1008).
ALTER INDEX public.events_thread RENAME TO events_workspace;
ALTER INDEX public.runs_id_thread_unique RENAME TO runs_id_workspace_unique;
ALTER TABLE public.subnodes RENAME CONSTRAINT subnodes_thread_key TO subnodes_workspace_key;
ALTER TABLE public.subnodes RENAME CONSTRAINT subnodes_thread_fkey TO subnodes_workspace_fkey;
ALTER TABLE public.human_requests RENAME CONSTRAINT human_requests_run_thread TO human_requests_run_workspace;
ALTER TABLE public.generation_requests RENAME CONSTRAINT generation_requests_task_thread TO generation_requests_task_workspace;
ALTER TABLE public.generation_requests RENAME CONSTRAINT generation_requests_local_thread_id_fkey TO generation_requests_local_workspace_id_fkey;
ALTER TABLE public.generation_embedding_usage RENAME CONSTRAINT generation_embedding_usage_thread_id_fkey TO generation_embedding_usage_workspace_id_fkey;
ALTER TABLE public.events RENAME CONSTRAINT events_thread_id_fkey TO events_workspace_id_fkey;
ALTER TABLE public.runs RENAME CONSTRAINT runs_tenant_id_thread_id_key TO runs_tenant_id_workspace_id_key;
