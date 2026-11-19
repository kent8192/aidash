// reinhardt-migration-source: 1
// An Agent no longer binds an External service (#738): External services are
// made available to a Scratch Space only. Drafts keep no such Binding.
// This one-time data change is SQL because Reinhardt Query cannot build it:
// its FROM sources are tables and subqueries only, so it cannot read a JSON
// array with `jsonb_array_elements(...) WITH ORDINALITY`, and its function
// calls cannot carry the `ORDER BY` that `jsonb_agg` needs to keep Binding
// order. Registered Agent versions are immutable and digest-pinned: they keep
// theirs, and Run start leaves it out.
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
