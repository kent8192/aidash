-- PostgreSQL renames constraints and indexes only with ALTER ... RENAME, which
-- typed Reinhardt operations cannot express. The names follow `runs` again,
-- before the table takes its old name back (#1009).
ALTER INDEX public.dashboard_status_waiting_task_executions RENAME TO dashboard_status_waiting_runs;
ALTER INDEX public.activation_dependency_task_executions RENAME TO activation_dependency_runs;
ALTER INDEX public.task_executions_task_created_page RENAME TO runs_task_created_page;
ALTER INDEX public.task_executions_ready RENAME TO runs_ready;
ALTER INDEX public.task_executions_id_thread_unique RENAME TO runs_id_thread_unique;
ALTER TABLE public.task_executions RENAME CONSTRAINT task_executions_control_provenance TO runs_control_provenance;
ALTER TABLE public.task_executions RENAME CONSTRAINT task_executions_phase_check TO runs_phase_check;
ALTER TABLE public.task_executions RENAME CONSTRAINT task_executions_lease TO runs_lease;
ALTER TABLE public.task_executions RENAME CONSTRAINT task_executions_counters TO runs_counters;
ALTER TABLE public.task_executions RENAME CONSTRAINT task_executions_control_check TO runs_control_check;
ALTER TABLE public.task_executions RENAME CONSTRAINT task_executions_tenant_root_fkey TO runs_tenant_root_fkey;
ALTER TABLE public.task_executions RENAME CONSTRAINT task_executions_node_fkey TO runs_node_fkey;
ALTER TABLE public.task_executions RENAME CONSTRAINT task_executions_human_request_ref TO runs_human_request_ref;
ALTER TABLE public.task_executions RENAME CONSTRAINT task_executions_tenant_id_thread_id_key TO runs_tenant_id_thread_id_key;
ALTER TABLE public.task_executions RENAME CONSTRAINT task_executions_tenant_id_key TO runs_tenant_id_key;
ALTER TABLE public.task_executions RENAME CONSTRAINT task_executions_home_node_task_id_key TO runs_home_node_task_id_key;
ALTER TABLE public.task_executions RENAME CONSTRAINT task_executions_pkey TO runs_pkey;
