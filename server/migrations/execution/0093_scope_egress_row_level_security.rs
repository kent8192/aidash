// reinhardt-migration-source: 1
// Reinhardt migrations do not express row-level policies.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0093_scope_egress_row_level_security", "execution")
		.add_dependency("execution", "0092_scope_egress")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0093_scope_egress_row_level_security.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0093_scope_egress_row_level_security.sql").into(),
			),
		})
}
