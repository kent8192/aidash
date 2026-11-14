// reinhardt-migration-source: 1
// Logical ORM state: the keys, checks and indexes 0027 created stay enforced in
// the database. The models declare none of them, so this state-only checkpoint
// removes them from the logical state and never executes SQL.

use reinhardt::db::migrations::FieldType;
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0030_schedules_model_state", "workspaces")
        .add_operation(Operation::DropNamedIndex {
            table: "schedule_ticks".to_string(),
            name: "schedule_ticks_history".to_string(),
            columns: vec!["schedule_id".to_string(), "scheduled_at".to_string()],
            unique: false,
            index_type: None,
            where_clause: None,
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .add_operation(Operation::DropNamedIndex {
            table: "schedule_ticks".to_string(),
            name: "schedule_ticks_open".to_string(),
            columns: vec!["tenant".to_string(), "status".to_string()],
            unique: false,
            index_type: None,
            where_clause: Some("status = 'started'".to_string()),
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .add_operation(Operation::DropNamedIndex {
            table: "schedule_ticks".to_string(),
            name: "schedule_ticks_slot_key".to_string(),
            columns: vec!["schedule_id".to_string(), "scheduled_at".to_string()],
            unique: true,
            index_type: None,
            where_clause: Some("started_by IS NULL".to_string()),
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .add_operation(Operation::DropNamedIndex {
            table: "schedule_ticks".to_string(),
            name: "schedule_ticks_task".to_string(),
            columns: vec!["task_id".to_string()],
            unique: true,
            index_type: None,
            where_clause: Some("task_id IS NOT NULL".to_string()),
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .add_operation(Operation::DropNamedIndex {
            table: "schedules".to_string(),
            name: "schedules_creation_key".to_string(),
            columns: vec!["created_by_run_id".to_string(), "creation_key".to_string()],
            unique: true,
            index_type: None,
            where_clause: Some("creation_key IS NOT NULL".to_string()),
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .add_operation(Operation::DropNamedIndex {
            table: "schedules".to_string(),
            name: "schedules_due".to_string(),
            columns: vec!["next_fire_at".to_string()],
            unique: false,
            index_type: None,
            where_clause: Some("halt IS NULL AND next_fire_at IS NOT NULL".to_string()),
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .add_operation(Operation::DropNamedIndex {
            table: "schedules".to_string(),
            name: "schedules_registrant".to_string(),
            columns: vec!["tenant".to_string(), "registrant".to_string()],
            unique: false,
            index_type: None,
            where_clause: None,
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .add_operation(Operation::DropNamedIndex {
            table: "schedules".to_string(),
            name: "schedules_workspace".to_string(),
            columns: vec!["tenant".to_string(), "workspace_id".to_string()],
            unique: false,
            index_type: None,
            where_clause: None,
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedule_limits".to_string(),
            constraint: Constraint::ForeignKey {
                name: "schedule_limits_tenant_fkey".to_string(),
                columns: vec!["tenant".to_string()],
                referenced_table: "tenants".to_string(),
                referenced_columns: vec!["name".to_string()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::Restrict,
                deferrable: None,
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedule_limits".to_string(),
            constraint: Constraint::Check {
                name: "schedule_limits_values_check".to_string(),
                expression: "max_schedules >= 1 AND max_per_person >= 1 AND min_interval_seconds >= 60"
                    .to_string(),
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedule_ticks".to_string(),
            constraint: Constraint::ForeignKey {
                name: "schedule_ticks_schedule_fkey".to_string(),
                columns: vec!["tenant".to_string(), "schedule_id".to_string()],
                referenced_table: "schedules".to_string(),
                referenced_columns: vec!["tenant".to_string(), "id".to_string()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::Restrict,
                deferrable: None,
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedule_ticks".to_string(),
            constraint: Constraint::Check {
                name: "schedule_ticks_status_check".to_string(),
                expression: "status IN ('started', 'completed', 'failed', 'cancelled', 'skipped')"
                    .to_string(),
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedule_ticks".to_string(),
            constraint: Constraint::Check {
                name: "schedule_ticks_task_check".to_string(),
                expression: "(status = 'skipped' AND task_id IS NULL) OR status <> 'skipped'"
                    .to_string(),
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedule_ticks".to_string(),
            constraint: Constraint::ForeignKey {
                name: "schedule_ticks_tenant_fkey".to_string(),
                columns: vec!["tenant".to_string()],
                referenced_table: "tenants".to_string(),
                referenced_columns: vec!["name".to_string()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::Restrict,
                deferrable: None,
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedule_ticks".to_string(),
            constraint: Constraint::ForeignKey {
                name: "schedule_ticks_workspace_fkey".to_string(),
                columns: vec!["tenant".to_string(), "workspace_id".to_string()],
                referenced_table: "workspaces".to_string(),
                referenced_columns: vec!["tenant".to_string(), "id".to_string()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::Restrict,
                deferrable: None,
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedules".to_string(),
            constraint: Constraint::Check {
                name: "schedules_approval_mode_check".to_string(),
                expression: "approval_mode IS NULL OR approval_mode IN ('manual', 'auto', 'dont_ask', 'bypass')"
                    .to_string(),
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedules".to_string(),
            constraint: Constraint::Check {
                name: "schedules_failures_check".to_string(),
                expression: "consecutive_failures >= 0".to_string(),
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedules".to_string(),
            constraint: Constraint::Check {
                name: "schedules_fire_check".to_string(),
                expression: "(next_scheduled_at IS NULL) = (next_fire_at IS NULL) AND (next_fire_at IS NULL OR next_fire_at >= next_scheduled_at)"
                    .to_string(),
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedules".to_string(),
            constraint: Constraint::Check {
                name: "schedules_halt_check".to_string(),
                expression: "halt IS NULL OR halt IN ('paused', 'failures', 'registrant_left', 'agent_version_unusable', 'thread_deleted', 'tenant_deleted', 'interval_too_short')"
                    .to_string(),
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedules".to_string(),
            constraint: Constraint::Check {
                name: "schedules_halted_at_check".to_string(),
                expression: "(halt IS NULL) = (halted_at IS NULL)".to_string(),
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedules".to_string(),
            constraint: Constraint::Check {
                name: "schedules_lateness_check".to_string(),
                expression: "max_lateness_seconds BETWEEN 60 AND 604800".to_string(),
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedules".to_string(),
            constraint: Constraint::Check {
                name: "schedules_successor_check".to_string(),
                expression: "successor IS NULL OR halt = 'registrant_left'".to_string(),
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedules".to_string(),
            constraint: Constraint::ForeignKey {
                name: "schedules_tenant_fkey".to_string(),
                columns: vec!["tenant".to_string()],
                referenced_table: "tenants".to_string(),
                referenced_columns: vec!["name".to_string()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::Restrict,
                deferrable: None,
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedules".to_string(),
            constraint: Constraint::Unique {
                name: "schedules_tenant_id_key".to_string(),
                columns: vec!["tenant".to_string(), "id".to_string()],
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedules".to_string(),
            constraint: Constraint::Check {
                name: "schedules_trigger_check".to_string(),
                expression: "jsonb_typeof(trigger) = 'object' AND trigger->>'kind' IN ('cron', 'interval')"
                    .to_string(),
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "schedules".to_string(),
            constraint: Constraint::ForeignKey {
                name: "schedules_workspace_fkey".to_string(),
                columns: vec!["tenant".to_string(), "workspace_id".to_string()],
                referenced_table: "workspaces".to_string(),
                referenced_columns: vec!["tenant".to_string(), "id".to_string()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::Restrict,
                deferrable: None,
            },
        })
        .add_dependency("workspaces", "0029_schedules_row_level_security")
        .atomic(true)
        .with_initial(None)
        .state_only(true)
        .database_only(false)
}
