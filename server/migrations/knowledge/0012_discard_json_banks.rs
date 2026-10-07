// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0012_discard_json_banks", "knowledge")
		.add_dependency("knowledge", "0011_memory_jobs")
		.add_dependency("execution", "0007_model_state")
		.add_operation(Operation::DropConstraintDefinition {
			table: "semantic_agent_memory".into(),
			constraint: Constraint::ForeignKey {
				name: "semantic_agent_memory_entry_id_fkey".into(),
				columns: vec!["entry_id".into()],
				referenced_table: "semantic_entries".into(),
				referenced_columns: vec!["id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		// PostgreSQL statement triggers are unsupported by typed schema operations.
		// Remove explicitly so reversal reinstalls the guard after table recreation.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0012_discard_json_banks.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0012_discard_json_banks.sql").to_owned()),
		})
		.add_operation(Operation::DropTable {
			name: "semantic_agent_memory".into(),
		})
		.add_operation(Operation::DropTable {
			name: "memory".into(),
		})
		.atomic(true)
}
