// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0052_node_cancellation_jobs_state", "execution")
		.add_dependency("execution", "0051_node_cancellation_jobs")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "node_cancellation_jobs".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("deletion_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("work_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("binding", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("retry_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
