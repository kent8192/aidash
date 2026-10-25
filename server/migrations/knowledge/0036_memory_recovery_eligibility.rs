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
		// Seed existing rows, then require application writers to name eligibility.
		.add_operation(Operation::AlterColumn {
			table: "memory_bank_settings".into(),
			column: "recovery_eligible_at".into(),
			old_definition: Some(
				ColumnDefinition::new("recovery_eligible_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".into())),
			),
			new_definition: ColumnDefinition::new("recovery_eligible_at", FieldType::TimestampTz)
				.with_not_null(true)
				.with_default(None),
			mysql_options: None,
		})
		.atomic(true)
}
