// reinhardt-migration-source: 1
// Deployment configuration has no Tenant or Node foreign key and survives their purge.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0050_runner_allocation_journals", "execution")
		.add_dependency("execution", "0049_foreign_key_relations_state")
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "runner_allocation_journals".into(),
			columns: vec![
				ColumnDefinition::new("deployment", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("runner_instance", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("pinned_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
