// reinhardt-migration-source: 1
// Router and Watcher scripts of a Duty (#1011, ADR 0142): a Duty keeps its
// current Router and Watcher and when the Watcher runs next, every saved
// script version with its author and time, and the Tasks it started to run a
// script without a model. An event records what the Router decided, and its
// outcome may be `routing`, `dropped` or `acted`. The new tables are keyed on
// the Duty or the Task and go with the Duty. Keys, checks and indexes are
// database-only; 0041 records the model state.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0039_duty_scripts", "workspaces")
		.add_dependency("workspaces", "0038_task_duty")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "duties".into(),
			column: ColumnDefinition::new("router", FieldType::Jsonb),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "duties".into(),
			column: ColumnDefinition::new("watcher", FieldType::Jsonb),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "duties".into(),
			column: ColumnDefinition::new("watcher_next_at", FieldType::TimestampTz),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "duties".into(),
			constraint: Constraint::Check {
				name: "duties_router_check".into(),
				expression: "router IS NULL OR (jsonb_typeof(router) = 'object' AND router->>'language' IN ('python', 'shell') AND jsonb_typeof(router->'source') = 'string')".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "duties".into(),
			constraint: Constraint::Check {
				name: "duties_watcher_check".into(),
				expression: "watcher IS NULL OR (jsonb_typeof(watcher) = 'object' AND watcher->'script'->>'language' IN ('python', 'shell') AND jsonb_typeof(watcher->'script'->'source') = 'string' AND jsonb_typeof(watcher->'every_seconds') = 'number')".into(),
			},
		})
		.add_operation(Operation::AddColumn {
			table: "duty_events".into(),
			column: ColumnDefinition::new("router", FieldType::Jsonb),
			mysql_options: None,
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "duty_events".into(),
			constraint: Constraint::Check {
				name: "duty_events_outcome_check".into(),
				expression: "outcome IN ('started', 'delivered', 'skipped', 'failed')".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "duty_events".into(),
			constraint: Constraint::Check {
				name: "duty_events_outcome_check".into(),
				expression: "outcome IN ('started', 'delivered', 'skipped', 'failed', 'routing', 'dropped', 'acted')".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "duty_events".into(),
			constraint: Constraint::Check {
				name: "duty_events_router_check".into(),
				expression: "router IS NULL OR (jsonb_typeof(router) = 'object' AND router->>'action' IN ('drop', 'act', 'task', 'deliver'))".into(),
			},
		})
		.add_operation(Operation::CreateTable {
			name: "duty_script_versions".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("duty_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("role", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("language", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("source", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("every_seconds", FieldType::BigInteger),
				ColumnDefinition::new("author", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "duty_script_versions_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "duty_script_versions_duty_fkey".into(),
					columns: vec!["tenant".into(), "duty_id".into()],
					referenced_table: "duties".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Check {
					name: "duty_script_versions_check".into(),
					expression: "role IN ('router', 'watcher') AND language IN ('python', 'shell') AND octet_length(source) <= 65536 AND (role = 'watcher') = (every_seconds IS NOT NULL)".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "duty_script_runs".into(),
			columns: vec![
				ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("duty_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("role", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("event_id", FieldType::Uuid),
				ColumnDefinition::new("language", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("source", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("stdin", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("settled_at", FieldType::TimestampTz),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "duty_script_runs_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "duty_script_runs_duty_fkey".into(),
					columns: vec!["tenant".into(), "duty_id".into()],
					referenced_table: "duties".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Check {
					name: "duty_script_runs_check".into(),
					expression: "role IN ('router', 'watcher') AND language IN ('python', 'shell') AND octet_length(source) <= 65536 AND (role = 'router') = (event_id IS NOT NULL)".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "duty_script_versions".into(),
			name: "duty_script_versions_history".into(),
			columns: vec!["duty_id".into(), "created_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "duty_script_runs".into(),
			name: "duty_script_runs_open".into(),
			columns: vec!["duty_id".into(), "role".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("settled_at IS NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "duties".into(),
			name: "duties_watcher_due".into(),
			columns: vec!["watcher_next_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("watcher_next_at IS NOT NULL AND halt IS NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
