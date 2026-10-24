// reinhardt-migration-source: 1
// Preserve dispatch lineage and reservations apart from settled policy tariffs.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0036_memory_attempt_evidence", "knowledge")
		.add_dependency("knowledge", "0035_foreign_key_relations_state")
		.add_operation(Operation::AddColumn {
			table: "memory_model_attempts".into(),
			column: ColumnDefinition::new("source_run_ids", FieldType::Jsonb),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "memory_model_attempts".into(),
			column: ColumnDefinition::new("reserved_tokens", FieldType::BigInteger),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "memory_model_attempts".into(),
			column: ColumnDefinition::new("reserved_cost_micros", FieldType::BigInteger),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "memory_model_attempts".into(),
			column: ColumnDefinition::new("provider_usage", FieldType::Jsonb),
			mysql_options: None,
		})
		.atomic(true)
}
