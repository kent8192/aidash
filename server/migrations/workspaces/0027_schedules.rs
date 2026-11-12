// reinhardt-migration-source: 1
// Schedules start Tasks as their Registrant (#557, ADR 0134). Each belongs to
// one Workspace of its Tenant; its ticks keep the history and the key that lets
// a scheduled time start at most one Task.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0027_schedules", "workspaces")
		.add_dependency("workspaces", "0026_scratch_binding_indexes")
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "schedules".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("registrant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("credential_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("title", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("request", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("trigger", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("thread_id", FieldType::Uuid),
				ColumnDefinition::new("approval_mode", FieldType::Text),
				ColumnDefinition::new("max_lateness_seconds", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("anchor", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("next_scheduled_at", FieldType::TimestampTz),
				ColumnDefinition::new("next_fire_at", FieldType::TimestampTz),
				ColumnDefinition::new("halt", FieldType::Text),
				ColumnDefinition::new("halted_at", FieldType::TimestampTz),
				ColumnDefinition::new("successor", FieldType::Text),
				ColumnDefinition::new("consecutive_failures", FieldType::Integer).with_not_null(true).with_default(Some("0".into())),
				ColumnDefinition::new("created_by_run_id", FieldType::Uuid),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true).with_default(Some("0".into())),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "schedules_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Unique {
					name: "schedules_tenant_id_key".into(),
					columns: vec!["tenant".into(), "id".into()],
				},
				Constraint::ForeignKey {
					name: "schedules_workspace_fkey".into(),
					columns: vec!["tenant".into(), "workspace_id".into()],
					referenced_table: "workspaces".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Check {
					name: "schedules_halt_check".into(),
					expression: "halt IS NULL OR halt IN ('paused', 'failures', 'registrant_left', 'agent_version_unusable', 'thread_deleted', 'tenant_deleted')".into(),
				},
				Constraint::Check {
					name: "schedules_halted_at_check".into(),
					expression: "(halt IS NULL) = (halted_at IS NULL)".into(),
				},
				Constraint::Check {
					name: "schedules_successor_check".into(),
					expression: "successor IS NULL OR halt = 'registrant_left'".into(),
				},
				Constraint::Check {
					name: "schedules_approval_mode_check".into(),
					expression: "approval_mode IS NULL OR approval_mode IN ('manual', 'auto', 'dont_ask', 'bypass')".into(),
				},
				Constraint::Check {
					name: "schedules_lateness_check".into(),
					expression: "max_lateness_seconds BETWEEN 60 AND 604800".into(),
				},
				Constraint::Check {
					name: "schedules_failures_check".into(),
					expression: "consecutive_failures >= 0".into(),
				},
				Constraint::Check {
					name: "schedules_fire_check".into(),
					expression: "(next_scheduled_at IS NULL) = (next_fire_at IS NULL) AND (next_fire_at IS NULL OR next_fire_at >= next_scheduled_at)".into(),
				},
				Constraint::Check {
					name: "schedules_trigger_check".into(),
					expression: "jsonb_typeof(trigger) = 'object' AND trigger->>'kind' IN ('cron', 'interval')".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "schedule_ticks".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("schedule_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("scheduled_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("recorded_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("status", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("reason", FieldType::Text),
				ColumnDefinition::new("task_id", FieldType::Uuid),
				ColumnDefinition::new("thread_id", FieldType::Uuid),
				ColumnDefinition::new("started_by", FieldType::Text),
				ColumnDefinition::new("settled_at", FieldType::TimestampTz),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "schedule_ticks_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "schedule_ticks_schedule_fkey".into(),
					columns: vec!["tenant".into(), "schedule_id".into()],
					referenced_table: "schedules".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "schedule_ticks_workspace_fkey".into(),
					columns: vec!["tenant".into(), "workspace_id".into()],
					referenced_table: "workspaces".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Check {
					name: "schedule_ticks_status_check".into(),
					expression: "status IN ('started', 'completed', 'failed', 'cancelled', 'skipped')".into(),
				},
				Constraint::Check {
					name: "schedule_ticks_task_check".into(),
					expression: "(status = 'skipped' AND task_id IS NULL) OR status <> 'skipped'".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "schedule_limits".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("max_schedules", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("max_per_person", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("min_interval_seconds", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true).with_default(Some("0".into())),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("updated_by", FieldType::Text).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "schedule_limits_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Check {
					name: "schedule_limits_values_check".into(),
					expression: "max_schedules >= 1 AND max_per_person >= 1 AND min_interval_seconds >= 60".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "schedules".into(),
			name: "schedules_due".into(),
			columns: vec!["next_fire_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("halt IS NULL AND next_fire_at IS NOT NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "schedules".into(),
			name: "schedules_workspace".into(),
			columns: vec!["tenant".into(), "workspace_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "schedules".into(),
			name: "schedules_registrant".into(),
			columns: vec!["tenant".into(), "registrant".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "schedule_ticks".into(),
			name: "schedule_ticks_slot_key".into(),
			columns: vec!["schedule_id".into(), "scheduled_at".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("started_by IS NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "schedule_ticks".into(),
			name: "schedule_ticks_history".into(),
			columns: vec!["schedule_id".into(), "scheduled_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "schedule_ticks".into(),
			name: "schedule_ticks_open".into(),
			columns: vec!["tenant".into(), "status".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("status = 'started'".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "schedule_ticks".into(),
			name: "schedule_ticks_task".into(),
			columns: vec!["task_id".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("task_id IS NOT NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
