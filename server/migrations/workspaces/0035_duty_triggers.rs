// reinhardt-migration-source: 1
// Duty Triggers other than the clock, the events they bring and the
// bookings a Task makes for the next event of a trigger (#1011, ADR 0142).
// Keys, checks and indexes are database-only; 0037 records the model state.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0035_duty_triggers", "workspaces")
		.add_dependency("workspaces", "0034_duties_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "duty_triggers".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("duty_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("source_duty_id", FieldType::Uuid),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("created_by", FieldType::Text).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "duty_triggers_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "duty_triggers_workspace_fkey".into(),
					columns: vec!["tenant".into(), "workspace_id".into()],
					referenced_table: "workspaces".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "duty_triggers_duty_fkey".into(),
					columns: vec!["tenant".into(), "duty_id".into()],
					referenced_table: "duties".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "duty_triggers_source_fkey".into(),
					columns: vec!["tenant".into(), "source_duty_id".into()],
					referenced_table: "duties".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Check {
					name: "duty_triggers_kind_check".into(),
					expression: "kind IN ('webhook', 'duty_completed') AND (kind = 'duty_completed') = (source_duty_id IS NOT NULL) AND source_duty_id IS DISTINCT FROM duty_id".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "duty_events".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("duty_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("trigger_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("received_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("payload", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("outcome", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("reason", FieldType::Text),
				ColumnDefinition::new("task_id", FieldType::Uuid),
				ColumnDefinition::new("run_id", FieldType::Uuid),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "duty_events_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "duty_events_workspace_fkey".into(),
					columns: vec!["tenant".into(), "workspace_id".into()],
					referenced_table: "workspaces".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "duty_events_duty_fkey".into(),
					columns: vec!["tenant".into(), "duty_id".into()],
					referenced_table: "duties".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Check {
					name: "duty_events_outcome_check".into(),
					expression: "outcome IN ('started', 'delivered', 'skipped', 'failed')".into(),
				},
				Constraint::Check {
					name: "duty_events_payload_check".into(),
					expression: "octet_length(payload::text) <= 1048576".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "duty_bookings".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("trigger_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "duty_bookings_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "duty_bookings_workspace_fkey".into(),
					columns: vec!["tenant".into(), "workspace_id".into()],
					referenced_table: "workspaces".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "duty_bookings_trigger_fkey".into(),
					columns: vec!["trigger_id".into()],
					referenced_table: "duty_triggers".into(),
					referenced_columns: vec!["id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "duty_triggers".into(),
			name: "duty_triggers_duty".into(),
			columns: vec!["tenant".into(), "duty_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "duty_triggers".into(),
			name: "duty_triggers_source".into(),
			columns: vec!["tenant".into(), "source_duty_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("source_duty_id IS NOT NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "duty_events".into(),
			name: "duty_events_history".into(),
			columns: vec!["duty_id".into(), "received_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "duty_bookings".into(),
			name: "duty_bookings_key".into(),
			columns: vec!["trigger_id".into(), "run_id".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
