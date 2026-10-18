// reinhardt-migration-source: 1
// A Run's Retry Cycle (#291, ADR 0057). An authorized manual retry of a
// semantic pause increments it, and native-memory model operations count only
// the current cycle's attempts against their retry ledger. It never decreases,
// because an Ordered Run keeps one memory operation across steps. Existing
// Runs start in cycle 0.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0034_run_semantic_retry_cycle", "execution")
		.add_dependency("execution", "0033_research_event_run_index")
		.add_operation(Operation::AddColumn {
			table: "runs".into(),
			column: ColumnDefinition::new("semantic_retry_cycle", FieldType::Integer)
				.with_not_null(true)
				.with_default(Some("0".into())),
			mysql_options: None,
		})
}
