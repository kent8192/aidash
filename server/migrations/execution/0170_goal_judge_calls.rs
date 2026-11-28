// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0170_goal_judge_calls", "execution")
		.add_dependency("execution", "0166_scope_connection_debits")
		.atomic(true)
		// Reinhardt cannot replace an existing named PostgreSQL check in place.
		.add_operation(Operation::RunSQL {
			sql: "ALTER TABLE public.model_calls DROP CONSTRAINT model_calls_purpose; ALTER TABLE public.model_calls ADD CONSTRAINT model_calls_purpose CHECK (purpose IN ('compaction_summary','memory_role','workbench_test','mcp_sampling','goal_judge'));".into(),
			reverse_sql: Some("ALTER TABLE public.model_calls DROP CONSTRAINT model_calls_purpose; ALTER TABLE public.model_calls ADD CONSTRAINT model_calls_purpose CHECK (purpose IN ('compaction_summary','memory_role','workbench_test','mcp_sampling'));".into()),
		})
}
