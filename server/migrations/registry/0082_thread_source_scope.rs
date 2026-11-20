// reinhardt-migration-source: 1
// Raw SQL is DDL only: PostgreSQL validation function bodies have no typed operation.
// A native memory Source is scoped to `thread` since the Workspace became the
// Thread (#1008, ADR 0140), but the validator from
// `registry/0067_optional_memory_roles` admitted only `participant` and
// `workspace`. Nothing enforces it on stored rows (registry 0015 skips native
// configurations and 0026 dropped `packages`), so this only keeps the
// function consistent: it admits `thread` too and keeps `workspace`, which
// stored versions carry and Rust still reads. The reverse restores the 0067
// body.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0082_thread_source_scope", "registry")
		.add_dependency("registry", "0081_thread_tools")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0082_thread_source_scope.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0082_thread_source_scope.sql").into()),
		})
}
