// reinhardt-migration-source: 1
// An authorized manual retry opens a new Retry Cycle of the same memory
// operation (#291, ADR 0057). Each attempt records the cycle it was charged
// in, so the retry ledger counts only the current cycle. Existing attempts
// belong to cycle 0, the cycle every operation starts in.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0032_memory_attempt_retry_cycle", "knowledge")
		.add_dependency("knowledge", "0031_embedding_provider_keys")
		.add_operation(Operation::AddColumn {
			table: "memory_model_attempts".into(),
			column: ColumnDefinition::new("cycle", FieldType::Integer)
				.with_not_null(true)
				.with_default(Some("0".into())),
			mysql_options: None,
		})
}
