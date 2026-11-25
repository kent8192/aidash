// reinhardt-migration-source: 1
//! Optional Tenant Background time limit below the Deployment ceiling.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0164_background_time_limit", "execution")
		.add_dependency("execution", "0163_scope_host_debit_state")
		.add_operation(Operation::AddColumn {
			table: "core_preview_limits".into(),
			column: ColumnDefinition::new("background_maximum_seconds", FieldType::BigInteger)
				.with_not_null(false),
			mysql_options: None,
		})
}
