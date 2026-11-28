CREATE VIEW public.core_run_areas WITH (security_invoker = true) AS
SELECT binding.run_id, binding.tenant, binding.initialized, area.id AS area_id
  FROM public.core_runs AS binding
  JOIN public.task_executions AS run
    ON run.id = binding.run_id AND run.tenant = binding.tenant
  LEFT JOIN public.core_areas AS area
    ON area.tenant = binding.tenant
   AND area.home_node = run.home_node
   AND area.thread_id = run.thread_id
   AND area.agent_id = run.agent_id
   AND area.owner = binding.scope_subject;
