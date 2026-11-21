-- A Task's Working Area session is its Thread (#1008, ADR 0140), so the Task
-- to reply-thread map goes. The table carries a forced row-level policy and a
-- write guard that typed DropTable cannot drop or restore.
DROP TABLE public.core_task_sessions;
