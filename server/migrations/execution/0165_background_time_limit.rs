// reinhardt-migration-source: 1
//! Optional Tenant Background time limit below the Deployment ceiling.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0165_background_time_limit", "execution")
		.add_dependency("execution", "0164_model_call_recovery")
		.add_operation(Operation::AddColumn {
			table: "core_preview_limits".into(),
			column: ColumnDefinition::new("background_maximum_seconds", FieldType::BigInteger)
				.with_not_null(false),
			mysql_options: None,
		})
}
