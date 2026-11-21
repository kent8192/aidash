// reinhardt-migration-source: 1
// Stints (#1009, ADR 0141): one row per Agent's awake stretch on a Task. A Stint
// opens when its Run becomes ready or starts working and none is open, and ends
// when the Run enters a wait the Agent cannot move forward itself (every wait
// but failure delivery) or reaches a terminal phase. Every writer of a Run goes
// through `runs`, so a trigger records Stints for all of them; Reinhardt has no
// trigger operation, so the function and trigger are the documented raw-SQL
// exception. Inference attempts record the Stint they ran in.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0145_stints", "execution")
		.add_dependency("execution", "0144_subnode_caps_removed_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "stints".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("started_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("ended_at", FieldType::TimestampTz),
				ColumnDefinition::new("end_reason", FieldType::Text),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "stints_tenant_root_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "stints_run_fkey".into(),
					columns: vec!["tenant".into(), "run_id".into()],
					referenced_table: "runs".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "stints_end".into(),
					expression: "(ended_at IS NULL) = (end_reason IS NULL) AND (ended_at IS NULL OR ended_at >= started_at)".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateIndex {
			table: "stints".into(),
			columns: vec!["run_id".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("ended_at IS NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateIndex {
			table: "stints".into(),
			columns: vec!["run_id".into(), "started_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::AddColumn {
			table: "inference_attempts".into(),
			column: ColumnDefinition::new("stint_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0145_stints.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0145_stints.sql").into()),
		})
}
