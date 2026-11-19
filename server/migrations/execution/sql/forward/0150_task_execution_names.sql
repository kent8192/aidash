-- PostgreSQL renames constraints and indexes only with ALTER ... RENAME, which
-- typed Reinhardt operations cannot express. Names follow the Task's execution
-- record (#1009, ADR 0141).
ALTER TABLE public.task_executions RENAME CONSTRAINT runs_pkey TO task_executions_pkey;
ALTER TABLE public.task_executions RENAME CONSTRAINT runs_home_node_task_id_key TO task_executions_home_node_task_id_key;
ALTER TABLE public.task_executions RENAME CONSTRAINT runs_tenant_id_key TO task_executions_tenant_id_key;
ALTER TABLE public.task_executions RENAME CONSTRAINT runs_tenant_id_thread_id_key TO task_executions_tenant_id_thread_id_key;
ALTER TABLE public.task_executions RENAME CONSTRAINT runs_human_request_ref TO task_executions_human_request_ref;
ALTER TABLE public.task_executions RENAME CONSTRAINT runs_node_fkey TO task_executions_node_fkey;
ALTER TABLE public.task_executions RENAME CONSTRAINT runs_tenant_root_fkey TO task_executions_tenant_root_fkey;
ALTER TABLE public.task_executions RENAME CONSTRAINT runs_control_check TO task_executions_control_check;
ALTER TABLE public.task_executions RENAME CONSTRAINT runs_counters TO task_executions_counters;
ALTER TABLE public.task_executions RENAME CONSTRAINT runs_lease TO task_executions_lease;
ALTER TABLE public.task_executions RENAME CONSTRAINT runs_phase_check TO task_executions_phase_check;
ALTER TABLE public.task_executions RENAME CONSTRAINT runs_control_provenance TO task_executions_control_provenance;
ALTER INDEX public.runs_id_thread_unique RENAME TO task_executions_id_thread_unique;
ALTER INDEX public.runs_ready RENAME TO task_executions_ready;
ALTER INDEX public.runs_task_created_page RENAME TO task_executions_task_created_page;
ALTER INDEX public.activation_dependency_runs RENAME TO activation_dependency_task_executions;
ALTER INDEX public.dashboard_status_waiting_runs RENAME TO dashboard_status_waiting_task_executions;
