// reinhardt-migration-source: 1
// Duty Triggers gain the `node_event` kind: an event another Node sends
// (#1013, ADR 0142). Only that kind may name a `from_node`. The check is
// database-only, as 0035 created it.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0044_duty_trigger_node_event_check", "workspaces")
		.add_dependency("workspaces", "0043_duty_trigger_from_node")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "duty_triggers".into(),
			constraint: Constraint::Check {
				name: "duty_triggers_kind_check".into(),
				expression: "kind IN ('webhook', 'duty_completed') AND (kind = 'duty_completed') = (source_duty_id IS NOT NULL) AND source_duty_id IS DISTINCT FROM duty_id".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "duty_triggers".into(),
			constraint: Constraint::Check {
				name: "duty_triggers_kind_check".into(),
				expression: "kind IN ('webhook', 'duty_completed', 'node_event') AND (kind = 'duty_completed') = (source_duty_id IS NOT NULL) AND source_duty_id IS DISTINCT FROM duty_id AND (from_node IS NULL OR kind = 'node_event')".into(),
			},
		})
}
