// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0073_scratch_wipe_receipts", "workspaces")
		.add_dependency("workspaces", "0072_node_scratch_spaces")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "scratch_spaces".into(),
			column: ColumnDefinition::new("wipe_completed_generation", FieldType::BigInteger)
				.with_not_null(true)
				.with_default(Some("0".into())),
			mysql_options: None,
		})
}
