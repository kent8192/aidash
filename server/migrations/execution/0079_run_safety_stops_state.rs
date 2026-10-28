// reinhardt-migration-source: 1
// Logical model state; 0077 owns the physical integrity constraints and indexes.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0079_run_safety_stops_state", "execution")
		.add_dependency("execution", "0078_run_safety_stops_row_level_security")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "run_safety_stops".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("action", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("expected_control_revision", FieldType::BigInteger)
					.with_not_null(true),
				ColumnDefinition::new("actor_kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("identity_id", FieldType::Uuid),
				ColumnDefinition::new("reason", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("operation_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("caller_key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("fingerprint", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("outcome", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
