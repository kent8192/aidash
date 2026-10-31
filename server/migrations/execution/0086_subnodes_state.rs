// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0086_subnodes_state", "execution")
		.add_dependency("execution", "0085_subnodes_row_level_security")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "subnodes".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("creator_run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("creator_task_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("creator_agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("creator_agent_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("leader_agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("leader_agent_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("allowance", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("title", FieldType::Text),
				ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("ended_at", FieldType::TimestampTz),
				ColumnDefinition::new("ended_reason", FieldType::Text),
				ColumnDefinition::new("creation_key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("creation_fingerprint", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "subnode_members".into(),
			columns: vec![
				ColumnDefinition::new("subnode_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("agent_id", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("added_by_run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("added_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "subnode_operation_receipts".into(),
			columns: vec![
				ColumnDefinition::new("run_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("call_key", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("fingerprint", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("result", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "subnode_limits".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("members_per_subnode", FieldType::Integer)
					.with_not_null(true),
				ColumnDefinition::new("subnodes_per_run", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("remote_runs_may_create", FieldType::Boolean)
					.with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("updated_by", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
