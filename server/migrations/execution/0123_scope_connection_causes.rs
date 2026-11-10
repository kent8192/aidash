// reinhardt-migration-source: 1
// Why a Host connection did not complete, and how long it waited (#845).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0123_scope_connection_causes", "execution")
		.add_dependency("execution", "0122_invocation_refused_state")
		.add_operation(Operation::AddColumn {
			table: "egress_scope_connections".into(),
			column: ColumnDefinition::new("error", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "egress_scope_connections".into(),
			column: ColumnDefinition::new("wait_ms", FieldType::BigInteger),
			mysql_options: None,
		})
		.atomic(true)
}
