// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0036_memory_recovery_eligibility", "knowledge")
		.add_dependency("knowledge", "0035_foreign_key_relations_state")
		.add_operation(Operation::AddColumn {
			table: "memory_bank_settings".into(),
			column: ColumnDefinition::new("recovery_eligible_at", FieldType::TimestampTz)
				.with_not_null(true)
				.with_default(Some("CURRENT_TIMESTAMP".into())),
			mysql_options: None,
		})
		.atomic(true)
}
