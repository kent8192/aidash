// reinhardt-migration-source: 1
// Logical model state; 0050 owns the physical constraints.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0051_research_journal_results_state", "execution")
		.add_dependency("execution", "0050_research_journal_results")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "research_journal_results".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("run_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("invocation_key", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("digest", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("reference", FieldType::Jsonb)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("bytes", FieldType::Binary)
					.with_not_null(false)
					.with_primary_key(false),
				ColumnDefinition::new("purge_reason", FieldType::Text)
					.with_not_null(false)
					.with_primary_key(false),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_primary_key(false),
			],
			constraints: vec![Constraint::ForeignKey {
				name: "research_journal_results_run".into(),
				columns: vec!["tenant".into(), "run_id".into()],
				referenced_table: "runs".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
