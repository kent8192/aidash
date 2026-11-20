// reinhardt-migration-source: 1
// An outbox for Watcher output and Duty completion events. Reinhardt has no
// typed operation for the Tenant row policy, so that DDL uses SQL assets.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0065_duty_pending_events", "workspaces")
		.add_dependency("workspaces", "0064_duty_deliveries_row_level_security")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "duty_pending_events".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("duty_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("trigger_id", FieldType::Uuid),
				ColumnDefinition::new("received_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("payload", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "duty_pending_events_duty_fkey".into(),
					columns: vec!["tenant".into(), "duty_id".into()],
					referenced_table: "duties".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "duty_pending_events_trigger_fkey".into(),
					columns: vec!["tenant".into(), "trigger_id".into()],
					referenced_table: "duty_triggers".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
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
			table: "duty_pending_events".into(),
			name: "duty_pending_events_by_duty".into(),
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
			table: "duty_pending_events".into(),
			name: "duty_pending_events_by_trigger".into(),
			columns: vec!["tenant".into(), "trigger_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("trigger_id IS NOT NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0065_duty_pending_events.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0065_duty_pending_events.sql").to_owned()),
		})
}
