// reinhardt-migration-source: 1
// PostgreSQL NOT VALID constraint DDL has no typed migration operation.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0092_registry_kind_check", "registry")
		.add_dependency("registry", "0091_thread_target_kinds")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0092_registry_kind_check.sql").into(),
			reverse_sql: None,
		})
}
