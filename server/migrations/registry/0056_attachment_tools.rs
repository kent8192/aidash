// reinhardt-migration-source: 1
// PostgreSQL function bodies have no typed migration operation. Extend the
// existing descriptor and Agent default validators for attachment_copy.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0056_attachment_tools", "registry")
		.add_dependency("registry", "0055_remove_structured_harness_contracts")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0056_attachment_tools.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0056_attachment_tools.sql").into()),
		})
}
