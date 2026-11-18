// reinhardt-migration-source: 1
// Research evidence is gone (#987, ADR 0138), so a memory's Journal proof is
// the admitted range alone. The rows stay; the `research` column and the part
// of the check that names it go. 0057 records the model state.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0056_journal_proofs_without_research", "knowledge")
		.add_dependency("knowledge", "0055_attachment_images")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "memory_journal_proofs".into(),
			constraint: Constraint::Check {
				name: "memory_journal_proofs_valid".into(),
				expression: "from_seq > 0 AND through_seq >= from_seq AND jsonb_typeof(research) = 'array'".into(),
			},
		})
		// Restoring the column on rollback needs a value for existing rows; a
		// proof without research evidence held an empty array.
		.add_operation(Operation::DropColumn {
			table: "memory_journal_proofs".into(),
			column: "research".into(),
			old_definition: Some(
				ColumnDefinition::new("research", FieldType::Jsonb)
					.with_not_null(true)
					.with_default(Some("'[]'::jsonb".to_owned())),
			),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory_journal_proofs".into(),
			constraint: Constraint::Check {
				name: "memory_journal_proofs_valid".into(),
				expression: "from_seq > 0 AND through_seq >= from_seq".into(),
			},
		})
}
