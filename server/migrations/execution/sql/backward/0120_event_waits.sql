SET LOCAL search_path = public, pg_catalog;
DROP TRIGGER aidash_delegated_task_activation ON public.authorization_run_outputs;
DROP FUNCTION public.aidash_delegated_task_activation();
DROP TRIGGER aidash_workspace_task_activation ON public.tasks;
DROP FUNCTION public.aidash_workspace_task_activation();
DROP TRIGGER aidash_operation_finished_activation ON public.core_operations;
DROP FUNCTION public.aidash_operation_finished_activation();
