// reinhardt-migration-source: 1
// ORM state of the Journal proofs without research; 0056 changed the database.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0057_journal_proofs_without_research_state", "knowledge")
		.add_dependency("knowledge", "0056_journal_proofs_without_research")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::DropColumn {
			table: "memory_journal_proofs".into(),
			column: "research".into(),
			old_definition: Some(
				ColumnDefinition::new("research", FieldType::Jsonb).with_not_null(true),
			),
		})
}
