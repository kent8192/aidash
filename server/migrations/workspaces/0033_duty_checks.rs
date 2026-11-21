// reinhardt-migration-source: 1
// Duty checks (#1011): a Duty whose Requester left is `requester_left`, the
// minimum-interval stop is gone (#1012, its Duties are paused instead), the
// time trigger may be absent and the failure pause is 0 (never) to 1000.
// Rewriting the stored halts is data, not schema: its assets are Reinhardt
// Query output from `workspaces::services::duty_halt_backfill`, checked by
// `manage migrationseeds`. The Tenant limits table carries a forced row-level
// policy that a typed DropTable cannot drop or restore, so dropping it is
// handwritten SQL; 0034 records the model state.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0033_duty_checks", "workspaces")
		.add_dependency("workspaces", "0032_duties")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "duties".into(),
			constraint: Constraint::Check {
				name: "schedules_successor_check".into(),
				expression: "successor IS NULL OR halt = 'registrant_left'".into(),
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "duties".into(),
			constraint: Constraint::Check {
				name: "schedules_halt_check".into(),
				expression: "halt IS NULL OR halt IN ('paused', 'failures', 'registrant_left', 'agent_version_unusable', 'thread_deleted', 'tenant_deleted', 'interval_too_short')".into(),
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "duties".into(),
			constraint: Constraint::Check {
				name: "schedules_trigger_check".into(),
				expression: "jsonb_typeof(trigger) = 'object' AND trigger->>'kind' IN ('cron', 'interval')".into(),
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0033_duty_halts.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0033_duty_halts.sql").into()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "duties".into(),
			constraint: Constraint::Check {
				name: "duties_halt_check".into(),
				expression: "halt IS NULL OR halt IN ('paused', 'failures', 'requester_left', 'agent_version_unusable', 'thread_deleted', 'tenant_deleted')".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "duties".into(),
			constraint: Constraint::Check {
				name: "duties_successor_check".into(),
				expression: "successor IS NULL OR halt = 'requester_left'".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "duties".into(),
			constraint: Constraint::Check {
				name: "duties_trigger_check".into(),
				expression: "trigger IS NULL OR (jsonb_typeof(trigger) = 'object' AND trigger->>'kind' IN ('cron', 'interval'))".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "duties".into(),
			constraint: Constraint::Check {
				name: "duties_pause_after_failures_check".into(),
				expression: "pause_after_failures BETWEEN 0 AND 1000".into(),
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0033_duty_limits.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0033_duty_limits.sql").into()),
		})
}
