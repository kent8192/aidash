// reinhardt-migration-source: 1
// PostgreSQL function bodies have no typed migration operation. DDL only;
// historical descriptors remain unchanged and retain strict lifecycle checks.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0012_host_lifecycle_expression", "registry")
		.database_only(true)
		.add_dependency("registry", "0011_binding_contract")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0012_host_lifecycle_expression.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0012_host_lifecycle_expression.sql").into(),
			),
		})
}
