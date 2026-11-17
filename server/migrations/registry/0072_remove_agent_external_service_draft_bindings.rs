// reinhardt-migration-source: 1
// An Agent no longer binds an External service (#738): External services are
// made available to a Scratch Space only. Drafts keep no such Binding.
// Reinhardt has no data operation for rewriting a JSON array inside a column,
// so this one-time data change is SQL. Registered Agent versions are immutable
// and digest-pinned: they keep theirs, and Run start leaves it out.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new(
		"0072_remove_agent_external_service_draft_bindings",
		"registry",
	)
	.add_dependency("registry", "0071_execution_pod_tools")
	.database_only(true)
	.atomic(true)
	.add_operation(Operation::RunSQL {
		sql: include_str!("sql/forward/0072_remove_agent_external_service_draft_bindings.sql")
			.into(),
		reverse_sql: Some(
			include_str!("sql/backward/0072_remove_agent_external_service_draft_bindings.sql")
				.into(),
		),
	})
}
