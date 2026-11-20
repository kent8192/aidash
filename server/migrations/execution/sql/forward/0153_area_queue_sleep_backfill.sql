SET LOCAL aidash.scope = 'deployment';
UPDATE "core_runs" SET "asleep" = TRUE WHERE EXISTS (SELECT 1 FROM "task_executions" WHERE "task_executions"."id" = "core_runs"."run_id" AND "task_executions"."tenant" = "core_runs"."tenant" AND aidash_area_queue_should_sleep("task_executions"."phase", "task_executions"."control", "task_executions"."pending", "task_executions"."lease_owner"));
