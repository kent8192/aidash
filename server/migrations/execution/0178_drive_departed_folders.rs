// reinhardt-migration-source: 1
//! Mark private file versions once when their owner leaves the Tenant.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0178_drive_departed_folders", "execution")
		.add_dependency("execution", "0177_drive_model_state")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "drive_entries".into(),
			column: ColumnDefinition::new("owner_departed_at", FieldType::TimestampTz),
			mysql_options: None,
		})
}
