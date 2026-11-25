SET LOCAL aidash.scope = 'deployment';
UPDATE "events" SET "data" = "data" - 'started_by_duty' #- '{task,started_by_duty}' WHERE "kind" IN ('task.created', 'task.claimed', 'task.updated', 'task.completed', 'task.abandoned') AND ("data" ? 'started_by_duty' OR "data" #> '{task,started_by_duty}' IS NOT NULL);
