// reinhardt-migration-source: 1
// Snapshot stop obligations in the Node deletion transaction. Restoration does
// not remove them, and work created after restoration never enters this snapshot.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0053_node_cancellation_jobs", "execution")
		.add_dependency("execution", "0052_runner_allocation_journals")
		.add_dependency("identity", "0044_node_administration_target_state")
		.add_dependency("federation", "0018_remote_admission_cancellation_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddConstraintDefinition {
			table: "node_deletions".into(),
			constraint: Constraint::Unique {
				name: "node_deletions_cancellation_owner".into(),
				columns: vec!["id".into(), "tenant".into(), "node".into()],
			},
		})
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
			constraints: vec![
				Constraint::ForeignKey {
					name: "node_cancellation_jobs_deletion_owner_fkey".into(),
					columns: vec!["deletion_id".into(), "tenant".into(), "node".into()],
					referenced_table: "node_deletions".into(),
					referenced_columns: vec!["id".into(), "tenant".into(), "node".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "node_cancellation_jobs_kind".into(),
					expression:
						"kind IN ('run','admission','generation','grant','intent','task','operation','python')"
							.into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "node_cancellation_jobs".into(),
			name: "node_cancellation_jobs_due".into(),
			columns: vec!["retry_at".into(), "id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
